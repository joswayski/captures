use std::{
    fs::{self, File},
    io::{self, Write},
    path::{Path, PathBuf},
};

use chrono::Local;
use image::RgbaImage;
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
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let contents = serde_json::to_vec_pretty(settings)?;
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, contents)?;
    fs::rename(temporary, path)?;
    Ok(())
}

pub fn save_capture(
    image: &RgbaImage,
    settings: &AppSettings,
) -> Result<(PathBuf, Vec<u8>), AppError> {
    let directory = PathBuf::from(&settings.output_directory);
    fs::create_dir_all(&directory)?;
    let png = encode_png(image)?;
    let stem = format!("CES_{}", Local::now().format("%Y-%m-%d_%H-%M-%S_%3f"));
    let path = unique_path(&directory, &stem);
    let temporary = directory.join(format!(".ces-{}.tmp", Uuid::new_v4()));

    let mut file = File::create(&temporary)?;
    file.write_all(&png)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&temporary, &path)?;
    Ok((path, png))
}

pub fn encode_png(image: &RgbaImage) -> Result<Vec<u8>, AppError> {
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgba8(image.clone())
        .write_to(&mut io::Cursor::new(&mut bytes), image::ImageFormat::Png)
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

    use super::{save_capture, unique_path};
    use crate::models::AppSettings;

    #[test]
    fn save_capture_writes_a_png_and_avoids_collisions() {
        let directory = tempdir().expect("temporary directory");
        let settings = AppSettings {
            output_directory: directory.path().to_string_lossy().into_owned(),
            ..AppSettings::default()
        };
        let image = RgbaImage::from_pixel(2, 3, Rgba([1, 2, 3, 255]));

        let (path, bytes) = save_capture(&image, &settings).expect("capture saved");
        assert_eq!(
            image::ImageFormat::from_path(&path).unwrap(),
            image::ImageFormat::Png
        );
        assert!(!bytes.is_empty());
        assert!(path.exists());
        assert!(!unique_path(directory.path(), "CES_test").exists());
    }
}
