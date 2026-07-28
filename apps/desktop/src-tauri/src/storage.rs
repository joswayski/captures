use std::{
    fs::{self, File},
    io::Write,
    path::{Component, Path, PathBuf},
};

use chrono::{DateTime, Duration, Local, Utc};
use image::{
    ExtendedColorType, ImageEncoder, RgbaImage,
    codecs::png::{CompressionType, FilterType, PngEncoder},
};
use uuid::Uuid;

use crate::{
    AppError,
    models::{
        AppSettings, ArtifactKind, CaptureArtifact, HISTORY_RETENTION_DAYS, HistoryEntry,
        RecordingArtifactData,
    },
};

const HISTORY_IMAGE_FILE: &str = "capture.png";
const HISTORY_PREVIEW_FILE: &str = "preview.png";
const HISTORY_METADATA_FILE: &str = "metadata.json";
const DRAG_EXPORT_DIRECTORY: &str = ".drag-exports";
const DRAG_ICON_FILE: &str = "drag-preview.png";
const DRAG_ICON_WIDTH: u32 = 284;
const DRAG_ICON_HEIGHT: u32 = 160;

pub struct ArtifactDragFiles {
    pub path: PathBuf,
    pub icon_path: PathBuf,
}

pub fn load_settings() -> AppSettings {
    let path = crate::models::settings_path();
    let mut settings = fs::read_to_string(&path)
        .ok()
        .and_then(|contents| serde_json::from_str(&contents).ok())
        .unwrap_or_default();
    crate::models::migrate_legacy_output_directory(&mut settings);
    if crate::models::migrate_settings(&mut settings)
        && let Err(error) = save_settings_to(&path, &settings)
    {
        eprintln!("failed to persist migrated settings: {error}");
    }
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

pub fn prepare_artifact_drag(artifact: &CaptureArtifact) -> Result<ArtifactDragFiles, AppError> {
    prepare_artifact_drag_in(&crate::models::history_directory(), artifact)
}

pub fn clear_drag_exports() -> Result<(), AppError> {
    clear_drag_exports_in(&crate::models::history_directory())
}

fn prepare_artifact_drag_in(
    history_root: &Path,
    artifact: &CaptureArtifact,
) -> Result<ArtifactDragFiles, AppError> {
    let artifact_id = Uuid::parse_str(&artifact.id).map_err(|_| AppError::HistoryUnavailable)?;
    let history_entry = history_root.join(artifact_id.to_string());
    let history_image = history_entry.join(HISTORY_IMAGE_FILE);
    let history_preview = history_entry.join(HISTORY_PREVIEW_FILE);
    let history_available = history_image.is_file() && history_preview.is_file();
    let fallback_directory = history_root
        .join(DRAG_EXPORT_DIRECTORY)
        .join(artifact_id.to_string());
    let drag_directory = if history_available {
        &history_entry
    } else {
        &fallback_directory
    };
    fs::create_dir_all(drag_directory)?;

    let path = artifact
        .path
        .as_ref()
        .map(PathBuf::from)
        .filter(|path| path.is_file())
        .map(fs::canonicalize)
        .transpose()?
        .unwrap_or_else(|| drag_directory.join(drag_file_name(&artifact.created_at)));
    if !path.is_file() {
        let linked = history_available && fs::hard_link(&history_image, &path).is_ok();
        if !linked {
            write_drag_file(&path, &artifact.image_png)?;
        }
    }

    let icon_path = drag_directory.join(DRAG_ICON_FILE);
    let icon_png = encode_drag_icon_png(&artifact.preview_png)?;
    write_drag_file(&icon_path, &icon_png)?;

    Ok(ArtifactDragFiles {
        path: fs::canonicalize(path)?,
        icon_path: fs::canonicalize(icon_path)?,
    })
}

fn drag_file_name(created_at: &str) -> String {
    let created_at = DateTime::parse_from_rfc3339(created_at)
        .map(|created_at| created_at.with_timezone(&Local))
        .unwrap_or_else(|_| Local::now());
    format!(
        "Captures_{}.png",
        created_at.format("%Y-%m-%d_%H-%M-%S_%3f")
    )
}

fn write_drag_file(path: &Path, bytes: &[u8]) -> Result<(), AppError> {
    if path.metadata().is_ok_and(|metadata| {
        metadata.is_file() && metadata.len() == u64::try_from(bytes.len()).unwrap_or(u64::MAX)
    }) {
        return Ok(());
    }
    let parent = path.parent().ok_or_else(|| {
        AppError::Io(std::io::Error::other(
            "drag file path has no parent directory",
        ))
    })?;
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".captures-drag-{}.tmp", Uuid::new_v4()));
    let result = (|| {
        fs::write(&temporary, bytes)?;
        if path.exists() {
            fs::remove_file(path)?;
        }
        fs::rename(&temporary, path)?;
        Ok::<(), AppError>(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn clear_drag_exports_in(history_root: &Path) -> Result<(), AppError> {
    match fs::remove_dir_all(history_root.join(DRAG_EXPORT_DIRECTORY)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub fn unique_media_path(directory: &Path, extension: &str) -> Result<PathBuf, AppError> {
    fs::create_dir_all(directory)?;
    let stem = format!("Captures_{}", Local::now().format("%Y-%m-%d_%H-%M-%S_%3f"));
    let initial = directory.join(format!("{stem}.{extension}"));
    if !initial.exists() {
        return Ok(initial);
    }
    Ok((1_u32..)
        .map(|suffix| directory.join(format!("{stem}-{suffix}.{extension}")))
        .find(|candidate| !candidate.exists())
        .unwrap_or_else(|| directory.join(format!("{stem}-{}.{}", Uuid::new_v4(), extension))))
}

pub fn recording_destination_path(
    source: &Path,
    file_stem: &str,
    extension: &str,
) -> Result<PathBuf, AppError> {
    recording_destination_path_in(source, None, file_stem, extension)
}

pub fn recording_destination_path_in(
    source: &Path,
    selected_directory: Option<&Path>,
    file_stem: &str,
    extension: &str,
) -> Result<PathBuf, AppError> {
    let stem = file_stem.trim();
    let reserved = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let portable_name = !stem.is_empty()
        && stem != "."
        && stem != ".."
        && stem == file_stem
        && !stem.ends_with('.')
        && !stem.ends_with(' ')
        && !stem
            .chars()
            .any(|character| character.is_control() || r#"<>:"/\|?*"#.contains(character))
        && !reserved.iter().any(|name| {
            stem.split('.')
                .next()
                .is_some_and(|base| base.eq_ignore_ascii_case(name))
        });
    let single_component = {
        let mut components = Path::new(stem).components();
        matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
    };
    if !portable_name || !single_component {
        return Err(AppError::Task(
            "enter a filename without folders or reserved characters".to_owned(),
        ));
    }
    if extension.is_empty()
        || extension
            .chars()
            .any(|character| !character.is_ascii_alphanumeric())
    {
        return Err(AppError::Task("the selected format is invalid".to_owned()));
    }
    let directory =
        selected_directory.unwrap_or_else(|| source.parent().unwrap_or_else(|| Path::new(".")));
    if !directory.is_dir() {
        return Err(AppError::Task(
            "the selected save folder is unavailable".to_owned(),
        ));
    }
    let destination = directory.join(format!("{stem}.{extension}"));
    if destination == source || destination.exists() {
        return Err(AppError::Task(format!(
            "“{stem}.{extension}” already exists; choose another filename"
        )));
    }
    Ok(destination)
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

pub fn save_history_recording(entry: &HistoryEntry, poster_png: &[u8]) -> Result<(), AppError> {
    if entry.kind == ArtifactKind::Screenshot || entry.recording_artifact().is_none() {
        return Err(AppError::Task(
            "recording history metadata is incomplete".to_owned(),
        ));
    }
    let directory = crate::models::history_directory();
    save_history_entry_in(&directory, entry, None, poster_png)?;
    let _ = load_capture_history_from(&directory, Utc::now())?;
    Ok(())
}

pub fn load_recording_artifact(entry: &HistoryEntry) -> Option<RecordingArtifactData> {
    let summary = entry.recording_artifact()?;
    let poster_png = load_history_image(&entry.id, true).ok()?;
    Some(RecordingArtifactData {
        summary,
        poster_png,
    })
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
    save_history_entry_in(root, entry, Some(image_png), preview_png)
}

fn save_history_entry_in(
    root: &Path,
    entry: &HistoryEntry,
    image_png: Option<&[u8]>,
    preview_png: &[u8],
) -> Result<(), AppError> {
    fs::create_dir_all(root)?;
    let destination = history_entry_directory(root, &entry.id)?;
    let temporary = root.join(format!(".{}.{}.tmp", entry.id, Uuid::new_v4()));
    fs::create_dir(&temporary)?;

    let result = (|| {
        if let Some(image_png) = image_png {
            fs::write(temporary.join(HISTORY_IMAGE_FILE), image_png)?;
        }
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
        if history_entry.kind == ArtifactKind::Screenshot && created_at < cutoff {
            let _ = fs::remove_dir_all(path);
            continue;
        }
        let files_are_valid = path.join(HISTORY_PREVIEW_FILE).is_file()
            && match history_entry.kind {
                ArtifactKind::Screenshot => {
                    history_entry.mode.is_some() && path.join(HISTORY_IMAGE_FILE).is_file()
                }
                ArtifactKind::Video | ArtifactKind::Gif => {
                    history_entry.recording_artifact().is_some()
                }
            };
        if !files_are_valid {
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

/// Downscale a full-resolution capture to logical display pixels (tests / legacy).
#[allow(dead_code)]
pub fn encode_preview_png(image: &RgbaImage, scale_factor: f64) -> Result<Vec<u8>, AppError> {
    let scale = scale_factor.max(1.0);
    let width = (f64::from(image.width()) / scale).round().max(1.0) as u32;
    let height = (f64::from(image.height()) / scale).round().max(1.0) as u32;
    if width < image.width() || height < image.height() {
        let preview = image::imageops::resize(
            image,
            width,
            height,
            image::imageops::FilterType::CatmullRom,
        );
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

fn encode_drag_icon_png(preview_png: &[u8]) -> Result<Vec<u8>, AppError> {
    let preview =
        image::load_from_memory(preview_png).map_err(|error| AppError::Image(error.to_string()))?;
    let icon = preview
        .resize_to_fill(
            DRAG_ICON_WIDTH,
            DRAG_ICON_HEIGHT,
            image::imageops::FilterType::Triangle,
        )
        .to_rgba8();
    encode_png_with_filter(&icon, FilterType::Sub)
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
    use captures_recording::{RecordingKind, RecordingTarget};
    use chrono::{Duration, TimeZone, Utc};
    use image::{Rgba, RgbaImage};
    use tempfile::tempdir;

    use super::{
        DRAG_EXPORT_DIRECTORY, DRAG_ICON_FILE, DRAG_ICON_HEIGHT, DRAG_ICON_WIDTH,
        HISTORY_IMAGE_FILE, HISTORY_PREVIEW_FILE, clear_drag_exports_in, encode_drag_icon_png,
        encode_png, encode_preview_png, encode_thumbnail_png, load_capture_history_from,
        prepare_artifact_drag_in, recording_destination_path, recording_destination_path_in,
        save_encoded_capture, save_history_capture_in, save_history_entry_in, save_settings_to,
        unique_path,
    };
    use crate::models::{
        AppSettings, ArtifactKind, CaptureArtifact, ClipboardCopyStatus, HistoryEntry,
        RecordingArtifact, history_full_url, history_preview_url,
    };

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
    fn recording_destination_requires_a_safe_single_basename() {
        let directory = tempdir().expect("temporary directory");
        let source = directory.path().join("source.mp4");

        let destination =
            recording_destination_path(&source, "Demo recording", "mp4").expect("safe destination");
        assert_eq!(destination, directory.path().join("Demo recording.mp4"));
        for unsafe_stem in [
            "",
            " ",
            ".",
            "..",
            "../escape",
            "nested/name",
            r"nested\name",
            "bad:name",
            "trailing.",
            "CON",
        ] {
            assert!(
                recording_destination_path(&source, unsafe_stem, "mp4").is_err(),
                "{unsafe_stem:?} must be rejected"
            );
        }
    }

    #[test]
    fn recording_destination_refuses_to_overwrite() {
        let directory = tempdir().expect("temporary directory");
        let source = directory.path().join("source.mp4");
        std::fs::write(&source, b"master").expect("source written");
        std::fs::write(directory.path().join("saved.mp4"), b"existing").expect("collision written");

        assert!(recording_destination_path(&source, "source", "mp4").is_err());
        assert!(recording_destination_path(&source, "saved", "mp4").is_err());
    }

    #[test]
    fn recording_destination_can_use_an_existing_selected_folder() {
        let source_directory = tempdir().expect("source directory");
        let selected_directory = tempdir().expect("selected directory");
        let source = source_directory.path().join("source.mp4");

        let destination =
            recording_destination_path_in(&source, Some(selected_directory.path()), "saved", "mp4")
                .expect("destination in selected folder");

        assert_eq!(destination, selected_directory.path().join("saved.mp4"));
        assert!(
            recording_destination_path_in(
                &source,
                Some(&selected_directory.path().join("missing")),
                "saved",
                "mp4",
            )
            .is_err()
        );
    }

    #[test]
    fn full_resolution_png_round_trips_exact_pixels() {
        let image = RgbaImage::from_fn(3, 2, |x, y| {
            Rgba([
                u8::try_from(x * 31).unwrap(),
                u8::try_from(y * 67).unwrap(),
                u8::try_from((x + y) * 23).unwrap(),
                255,
            ])
        });

        let bytes = encode_png(&image).expect("capture encoded");
        let decoded = image::load_from_memory(&bytes)
            .expect("capture decoded")
            .to_rgba8();

        assert_eq!(decoded.dimensions(), image.dimensions());
        assert_eq!(decoded.as_raw(), image.as_raw());
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
    fn drag_icon_matches_the_logical_preview_card_size() {
        let image = RgbaImage::from_pixel(568, 284, Rgba([1, 2, 3, 255]));
        let preview_png = encode_png(&image).expect("preview encoded");
        let bytes = encode_drag_icon_png(&preview_png).expect("drag icon encoded");
        let icon = image::load_from_memory(&bytes).expect("drag icon readable");

        assert_eq!(
            (icon.width(), icon.height()),
            (DRAG_ICON_WIDTH, DRAG_ICON_HEIGHT)
        );
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

    #[test]
    fn prepares_a_named_full_resolution_file_drag_from_private_history() {
        let directory = tempdir().expect("temporary directory");
        let id = uuid::Uuid::new_v4().to_string();
        let artifact = capture_artifact(&id, None, true);
        let entry = history_entry(&id, artifact.created_at.clone());
        save_history_capture_in(
            directory.path(),
            &entry,
            &artifact.image_png,
            &artifact.preview_png,
        )
        .expect("capture history saved");

        let drag =
            prepare_artifact_drag_in(directory.path(), &artifact).expect("artifact drag prepared");

        assert_eq!(std::fs::read(&drag.path).unwrap(), artifact.image_png);
        assert_eq!(drag.icon_path.file_name().unwrap(), DRAG_ICON_FILE);
        let icon = image::open(&drag.icon_path).expect("drag icon readable");
        assert_eq!(
            (icon.width(), icon.height()),
            (DRAG_ICON_WIDTH, DRAG_ICON_HEIGHT)
        );
        let history_entry = std::fs::canonicalize(directory.path().join(&id)).unwrap();
        assert_eq!(drag.path.parent(), Some(history_entry.as_path()));
        assert!(
            drag.path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("Captures_")
        );
    }

    #[test]
    fn falls_back_to_a_launch_scoped_drag_export_when_history_is_unavailable() {
        let directory = tempdir().expect("temporary directory");
        let id = uuid::Uuid::new_v4().to_string();
        let artifact = capture_artifact(&id, None, false);

        let drag =
            prepare_artifact_drag_in(directory.path(), &artifact).expect("artifact drag prepared");
        let export_root = directory.path().join(DRAG_EXPORT_DIRECTORY);
        let canonical_export_root = std::fs::canonicalize(&export_root).unwrap();

        assert!(drag.path.starts_with(&canonical_export_root));
        assert!(drag.icon_path.starts_with(&canonical_export_root));
        assert_eq!(std::fs::read(&drag.path).unwrap(), artifact.image_png);
        assert_eq!(drag.icon_path.file_name().unwrap(), DRAG_ICON_FILE);
        let icon = image::open(&drag.icon_path).expect("drag icon readable");
        assert_eq!(
            (icon.width(), icon.height()),
            (DRAG_ICON_WIDTH, DRAG_ICON_HEIGHT)
        );

        clear_drag_exports_in(directory.path()).expect("drag exports cleared");
        assert!(!export_root.exists());
    }

    #[test]
    fn recording_history_keeps_metadata_and_poster_without_copying_media() {
        let directory = tempdir().expect("temporary directory");
        let now = Utc.with_ymd_and_hms(2026, 7, 19, 12, 0, 0).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let media_path = directory.path().join("externally-managed.mp4");
        let artifact = RecordingArtifact {
            id: id.clone(),
            kind: RecordingKind::Video,
            path: media_path.to_string_lossy().into_owned(),
            media_url: format!("captures-capture://localhost/media/{id}"),
            poster_url: format!("captures-capture://localhost/poster/{id}"),
            mime_type: "video/mp4".to_owned(),
            duration_ms: 4_200,
            width: 1_920,
            height: 1_080,
            size_bytes: 123_456,
            dropped_frames: 0,
            has_system_audio: true,
            has_microphone_audio: false,
            created_at: (now - Duration::days(90)).to_rfc3339(),
            target: RecordingTarget::Display {
                display_id: "1".to_owned(),
            },
            missing: true,
        };
        let entry = HistoryEntry::from_recording(&artifact);

        save_history_entry_in(directory.path(), &entry, None, b"poster")
            .expect("recording history saved");
        let loaded = load_capture_history_from(directory.path(), now).expect("history loaded");

        assert_eq!(
            loaded.len(),
            1,
            "recording metadata does not expire with screenshot recovery"
        );
        assert_eq!(loaded[0].kind, ArtifactKind::Video);
        assert!(loaded[0].recording_artifact().unwrap().missing);
        assert!(!directory.path().join(&id).join(HISTORY_IMAGE_FILE).exists());
        assert_eq!(
            std::fs::read(directory.path().join(&id).join(HISTORY_PREVIEW_FILE)).unwrap(),
            b"poster",
        );
    }

    fn history_entry(id: &str, created_at: String) -> HistoryEntry {
        HistoryEntry {
            id: id.to_owned(),
            kind: ArtifactKind::Screenshot,
            preview_url: history_preview_url(id),
            full_url: history_full_url(id),
            width: 1_440,
            height: 900,
            size_bytes: 42,
            created_at,
            mode: Some(CaptureMode::Region),
            saved_path: None,
            mime_type: None,
            duration_ms: None,
            target: None,
            has_system_audio: false,
            has_microphone_audio: false,
            dropped_frames: 0,
        }
    }

    fn capture_artifact(id: &str, path: Option<String>, history_saved: bool) -> CaptureArtifact {
        let image = RgbaImage::from_pixel(568, 320, Rgba([16, 32, 48, 255]));
        CaptureArtifact {
            id: id.to_owned(),
            path,
            preview_url: format!("captures-capture://localhost/artifact/{id}"),
            full_url: format!("captures-capture://localhost/artifact-full/{id}"),
            width: 1_440,
            height: 900,
            size_bytes: 12,
            created_at: "2026-07-22T12:34:56.789Z".to_owned(),
            mode: CaptureMode::Region,
            history_saved,
            clipboard_copy_status: ClipboardCopyStatus::Copied,
            image_png: encode_png(&image).expect("capture encoded"),
            preview_png: encode_thumbnail_png(&image).expect("preview encoded"),
        }
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
