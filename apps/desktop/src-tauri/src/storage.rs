use std::{
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
};

use chrono::{DateTime, Duration, Local, Utc};
use image::{
    ExtendedColorType, ImageEncoder, RgbaImage,
    codecs::png::{CompressionType, FilterType, PngEncoder},
};
use uuid::Uuid;

use crate::{
    AppError,
    models::{AppSettings, HISTORY_RETENTION_DAYS, HistoryEntry},
};

const HISTORY_IMAGE_FILE: &str = "capture.png";
const HISTORY_PREVIEW_FILE: &str = "preview.png";
const HISTORY_METADATA_FILE: &str = "metadata.json";

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

pub fn load_capture_history() -> Result<Vec<HistoryEntry>, AppError> {
    load_capture_history_from(&crate::models::history_directory(), Utc::now())
}

pub fn save_history_capture(
    entry: &HistoryEntry,
    image_png: &[u8],
    preview_png: &[u8],
) -> Result<(), AppError> {
    let directory = crate::models::history_directory();
    save_history_capture_in(&directory, entry, image_png, preview_png)?;
    let _ = load_capture_history_from(&directory, Utc::now())?;
    Ok(())
}

pub fn load_history_images(entry_id: &str) -> Result<(Vec<u8>, Vec<u8>), AppError> {
    let directory = history_entry_directory(&crate::models::history_directory(), entry_id)?;
    Ok((
        fs::read(directory.join(HISTORY_IMAGE_FILE))?,
        fs::read(directory.join(HISTORY_PREVIEW_FILE))?,
    ))
}

pub fn load_history_image(entry_id: &str, preview: bool) -> Result<Vec<u8>, AppError> {
    let directory = history_entry_directory(&crate::models::history_directory(), entry_id)?;
    fs::read(directory.join(if preview {
        HISTORY_PREVIEW_FILE
    } else {
        HISTORY_IMAGE_FILE
    }))
    .map_err(Into::into)
}

pub fn delete_history_capture(entry_id: &str) -> Result<(), AppError> {
    let directory = history_entry_directory(&crate::models::history_directory(), entry_id)?;
    match fs::remove_dir_all(directory) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn save_history_capture_in(
    root: &Path,
    entry: &HistoryEntry,
    image_png: &[u8],
    preview_png: &[u8],
) -> Result<(), AppError> {
    fs::create_dir_all(root)?;
    let destination = history_entry_directory(root, &entry.id)?;
    let temporary = root.join(format!(".{}.{}.tmp", entry.id, Uuid::new_v4()));
    fs::create_dir(&temporary)?;

    let result = (|| {
        fs::write(temporary.join(HISTORY_IMAGE_FILE), image_png)?;
        fs::write(temporary.join(HISTORY_PREVIEW_FILE), preview_png)?;
        fs::write(
            temporary.join(HISTORY_METADATA_FILE),
            serde_json::to_vec_pretty(entry)?,
        )?;
        fs::rename(&temporary, destination)?;
        Ok::<(), AppError>(())
    })();

    if result.is_err() {
        let _ = fs::remove_dir_all(temporary);
    }
    result
}

fn load_capture_history_from(
    root: &Path,
    now: DateTime<Utc>,
) -> Result<Vec<HistoryEntry>, AppError> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let cutoff = now - Duration::days(HISTORY_RETENTION_DAYS);
    let mut history = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        let directory_name = entry.file_name().to_string_lossy().into_owned();
        if !path.is_dir() || directory_name.starts_with('.') {
            continue;
        }
        let contents = match fs::read(path.join(HISTORY_METADATA_FILE)) {
            Ok(contents) => contents,
            Err(_) => {
                let _ = fs::remove_dir_all(path);
                continue;
            }
        };
        let history_entry = match serde_json::from_slice::<HistoryEntry>(&contents) {
            Ok(history_entry) => history_entry,
            Err(_) => {
                let _ = fs::remove_dir_all(path);
                continue;
            }
        };
        if history_entry.id != directory_name || Uuid::parse_str(&history_entry.id).is_err() {
            let _ = fs::remove_dir_all(path);
            continue;
        }
        let created_at = match DateTime::parse_from_rfc3339(&history_entry.created_at) {
            Ok(created_at) => created_at,
            Err(_) => {
                let _ = fs::remove_dir_all(path);
                continue;
            }
        };
        let created_at = created_at.with_timezone(&Utc);
        if created_at < cutoff {
            let _ = fs::remove_dir_all(path);
            continue;
        }
        if !path.join(HISTORY_IMAGE_FILE).is_file() || !path.join(HISTORY_PREVIEW_FILE).is_file() {
            let _ = fs::remove_dir_all(path);
            continue;
        }
        history.push((created_at, history_entry));
    }

    history.sort_by(|(left, _), (right, _)| right.cmp(left));
    Ok(history.into_iter().map(|(_, entry)| entry).collect())
}

fn history_entry_directory(root: &Path, entry_id: &str) -> Result<PathBuf, AppError> {
    Uuid::parse_str(entry_id).map_err(|_| AppError::HistoryUnavailable)?;
    Ok(root.join(entry_id))
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
    use captures_capture::CaptureMode;
    use chrono::{Duration, TimeZone, Utc};
    use image::{Rgba, RgbaImage};
    use tempfile::tempdir;

    use super::{
        HISTORY_IMAGE_FILE, HISTORY_PREVIEW_FILE, encode_png, encode_preview_png,
        encode_thumbnail_png, load_capture_history_from, save_encoded_capture,
        save_history_capture_in, unique_path,
    };
    use crate::models::{AppSettings, HistoryEntry, history_full_url, history_preview_url};

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
    fn history_round_trips_and_prunes_captures_older_than_thirty_days() {
        let directory = tempdir().expect("temporary directory");
        let now = Utc.with_ymd_and_hms(2026, 7, 19, 12, 0, 0).unwrap();
        let recent_id = uuid::Uuid::new_v4().to_string();
        let expired_id = uuid::Uuid::new_v4().to_string();
        let recent = history_entry(&recent_id, (now - Duration::days(29)).to_rfc3339());
        let expired = history_entry(&expired_id, (now - Duration::days(31)).to_rfc3339());

        save_history_capture_in(directory.path(), &recent, b"recent-full", b"recent-preview")
            .expect("recent history saved");
        save_history_capture_in(
            directory.path(),
            &expired,
            b"expired-full",
            b"expired-preview",
        )
        .expect("expired history saved");

        let loaded = load_capture_history_from(directory.path(), now).expect("history loaded");

        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, recent_id);
        assert!(!directory.path().join(expired_id).exists());
        assert!(directory.path().join(&recent_id).exists());
        assert_eq!(
            std::fs::read(directory.path().join(&recent_id).join(HISTORY_IMAGE_FILE)).unwrap(),
            b"recent-full",
        );
        assert_eq!(
            std::fs::read(directory.path().join(&recent_id).join(HISTORY_PREVIEW_FILE)).unwrap(),
            b"recent-preview",
        );
    }

    fn history_entry(id: &str, created_at: String) -> HistoryEntry {
        HistoryEntry {
            id: id.to_owned(),
            preview_url: history_preview_url(id),
            full_url: history_full_url(id),
            width: 1_440,
            height: 900,
            size_bytes: 42,
            created_at,
            mode: CaptureMode::Region,
        }
    }
}
