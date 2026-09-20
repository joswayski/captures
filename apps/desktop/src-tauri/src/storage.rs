use std::{
    fs::{self, File},
    io::Write,
    path::{Component, Path, PathBuf},
};

use chrono::{DateTime, Local, Utc};
use image::{
    RgbaImage,
    codecs::png::{CompressionType, FilterType},
};
use uuid::Uuid;

use crate::{
    AppError,
    models::{AppSettings, CaptureArtifact, HistoryEntry, RecordingArtifactData},
};

const HISTORY_IMAGE_FILE: &str = "capture.png";
const HISTORY_PREVIEW_FILE: &str = "preview.png";
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
    captures_settings::load_shipping(&path)
}

pub fn save_settings(settings: &AppSettings) -> Result<(), AppError> {
    let path = crate::models::settings_path();
    save_settings_to(&path, settings)
}

fn save_settings_to(path: &Path, settings: &AppSettings) -> Result<(), AppError> {
    captures_settings::write_atomic(path, settings)
        .map_err(|error| AppError::Task(error.to_string()))?;
    Ok(())
}

pub fn save_encoded_capture(
    bytes: &[u8],
    settings: &AppSettings,
    extension: &str,
) -> Result<PathBuf, AppError> {
    let directory = PathBuf::from(&settings.output_directory);
    fs::create_dir_all(&directory)?;
    let stem = format!("Captures_{}", Local::now().format("%Y-%m-%d_%H-%M-%S_%3f"));
    let path = unique_path(&directory, &stem, extension);
    let temporary = directory.join(format!(".captures-{}.tmp", Uuid::new_v4()));

    let mut file = File::create(&temporary)?;
    file.write_all(bytes)?;
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
    recording_destination_path_in_mode(source, selected_directory, file_stem, extension, &[])
}

pub fn recording_replacement_destination_path_in(
    source: &Path,
    selected_directory: Option<&Path>,
    file_stem: &str,
    extension: &str,
) -> Result<PathBuf, AppError> {
    recording_replacement_destination_path_in_with_replaceable(
        source,
        selected_directory,
        file_stem,
        extension,
        &[],
    )
}

/// Like [`recording_replacement_destination_path_in`], but also allows replacing an
/// existing permanent Captures-folder save (distinct from private recovery media).
pub fn recording_replacement_destination_path_in_with_replaceable(
    source: &Path,
    selected_directory: Option<&Path>,
    file_stem: &str,
    extension: &str,
    replaceable: &[&Path],
) -> Result<PathBuf, AppError> {
    let mut paths = Vec::with_capacity(replaceable.len() + 1);
    paths.push(source);
    paths.extend_from_slice(replaceable);
    recording_destination_path_in_mode(source, selected_directory, file_stem, extension, &paths)
}

fn recording_destination_path_in_mode(
    source: &Path,
    selected_directory: Option<&Path>,
    file_stem: &str,
    extension: &str,
    replaceable: &[&Path],
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
    let can_replace = replaceable.contains(&destination.as_path());
    if destination.exists() && !can_replace {
        return Err(AppError::Task(format!(
            "“{stem}.{extension}” already exists; choose another filename"
        )));
    }
    Ok(destination)
}

pub fn load_capture_history() -> Result<Vec<HistoryEntry>, AppError> {
    captures_history::load(&crate::models::history_directory(), Utc::now()).map_err(Into::into)
}

pub fn save_history_capture(
    entry: &HistoryEntry,
    image_png: &[u8],
    preview_png: &[u8],
) -> Result<(), AppError> {
    let directory = crate::models::history_directory();
    captures_history::save_capture(&directory, entry, image_png, preview_png)?;
    // Expired entries are pruned when history is loaded at launch, not after
    // every save, so capture latency does not grow with history size.
    Ok(())
}

/// Save a recording into private capture history.
///
/// Copies `media_source` into the history entry as recovery media (retained for
/// [`captures_history::HISTORY_RETENTION_DAYS`]). `entry.saved_path` should only be set when the
/// user has also permanently saved a Captures-folder copy.
pub fn save_history_recording(
    entry: &HistoryEntry,
    poster_png: &[u8],
    media_source: &Path,
) -> Result<PathBuf, AppError> {
    let directory = crate::models::history_directory();
    captures_history::save_recording(&directory, entry, poster_png, media_source)
        .map_err(Into::into)
}

/// Record an opened recording in capture history without copying the media file.
/// Playback stays on `entry.saved_path`.
pub fn save_history_recording_reference(
    entry: &HistoryEntry,
    poster_png: &[u8],
) -> Result<(), AppError> {
    let directory = crate::models::history_directory();
    captures_history::save_recording_reference(&directory, entry, poster_png).map_err(Into::into)
}

/// Rewrite history metadata in place (for example after a permanent save) without
/// replacing recovery media already stored in the entry directory.
pub fn update_history_entry_metadata(entry: &HistoryEntry) -> Result<(), AppError> {
    captures_history::update_metadata(&crate::models::history_directory(), entry)
        .map_err(Into::into)
}

pub fn load_recording_artifact(entry: &HistoryEntry) -> Option<RecordingArtifactData> {
    let summary = crate::models::history_recording_artifact(entry)?;
    let poster_png = load_history_image(&entry.id, true).ok()?;
    Some(RecordingArtifactData {
        summary,
        poster_png,
    })
}

pub fn load_history_images(entry_id: &str) -> Result<(Vec<u8>, Vec<u8>), AppError> {
    captures_history::read_images(&crate::models::history_directory(), entry_id).map_err(Into::into)
}

pub fn load_history_image(entry_id: &str, preview: bool) -> Result<Vec<u8>, AppError> {
    captures_history::read_image(&crate::models::history_directory(), entry_id, preview)
        .map_err(Into::into)
}

pub fn delete_history_capture(entry_id: &str) -> Result<(), AppError> {
    captures_history::delete(&crate::models::history_directory(), entry_id).map_err(Into::into)
}

#[cfg(test)]
fn save_history_capture_in(
    root: &Path,
    entry: &HistoryEntry,
    image_png: &[u8],
    preview_png: &[u8],
) -> Result<(), AppError> {
    save_history_entry_in(root, entry, Some(image_png), preview_png, None).map(|_| ())
}

#[cfg(test)]
fn save_history_entry_in(
    root: &Path,
    entry: &HistoryEntry,
    image_png: Option<&[u8]>,
    preview_png: &[u8],
    media: Option<(&Path, &str)>,
) -> Result<Option<PathBuf>, AppError> {
    captures_history::save_entry(root, entry, image_png, preview_png, media).map_err(Into::into)
}

#[cfg(test)]
fn load_capture_history_from(
    root: &Path,
    now: DateTime<Utc>,
) -> Result<Vec<HistoryEntry>, AppError> {
    captures_history::load(root, now).map_err(Into::into)
}

pub fn encode_png(image: &RgbaImage) -> Result<Vec<u8>, AppError> {
    captures_history::encode_png(image).map_err(Into::into)
}

/// Freeze-frame bytes for the capture overlay / capture menu.
///
/// Crops still come from the uncompressed `RgbaImage`. Screenshot-like images
/// encode faster as PNG `Fast`+`Sub` than JPEG, and that encode sits on the
/// shortcut path before the webview can paint.
pub fn encode_overlay_snapshot(image: &RgbaImage) -> Result<Vec<u8>, AppError> {
    encode_png(image)
}

pub fn overlay_snapshot_mime_type(bytes: &[u8]) -> &'static str {
    if bytes.len() >= 3 && bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes[2] == 0xFF {
        "image/jpeg"
    } else {
        "image/png"
    }
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
    captures_history::encode_thumbnail_png(image).map_err(Into::into)
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
    encode_png_with_quality(image, CompressionType::Fast, filter)
}

fn encode_png_with_quality(
    image: &RgbaImage,
    compression: CompressionType,
    filter: FilterType,
) -> Result<Vec<u8>, AppError> {
    captures_history::encode_png_with_quality(image, compression, filter).map_err(Into::into)
}

fn unique_path(directory: &Path, stem: &str, extension: &str) -> PathBuf {
    let initial = directory.join(format!("{stem}.{extension}"));
    if !initial.exists() {
        return initial;
    }

    (1_u32..)
        .map(|suffix| directory.join(format!("{stem}-{suffix}.{extension}")))
        .find(|candidate| !candidate.exists())
        .unwrap_or_else(|| directory.join(format!("{stem}-{}.{extension}", Uuid::new_v4())))
}

#[cfg(test)]
mod tests {
    use captures_capture::CaptureMode;
    use captures_image::{
        encode_png_export, encode_png_export_dithered, png_palette_colors_for_quality,
    };
    use captures_recording::{RecordingKind, RecordingTarget};
    use chrono::{Duration, TimeZone, Utc};
    use image::{Rgba, RgbaImage};
    use tempfile::tempdir;

    use super::{
        DRAG_EXPORT_DIRECTORY, DRAG_ICON_FILE, DRAG_ICON_HEIGHT, DRAG_ICON_WIDTH,
        HISTORY_IMAGE_FILE, HISTORY_PREVIEW_FILE, clear_drag_exports_in, encode_drag_icon_png,
        encode_overlay_snapshot, encode_png, encode_preview_png, encode_thumbnail_png,
        load_capture_history_from, overlay_snapshot_mime_type, prepare_artifact_drag_in,
        recording_destination_path, recording_destination_path_in,
        recording_replacement_destination_path_in,
        recording_replacement_destination_path_in_with_replaceable, save_encoded_capture,
        save_history_capture_in, save_history_entry_in, save_settings_to, unique_path,
    };
    use crate::models::{
        AppSettings, ArtifactKind, CaptureArtifact, ClipboardCopyStatus, HistoryEntry,
        RecordingArtifact, history_full_url, history_preview_url,
    };

    #[test]
    fn overlay_snapshots_use_the_fast_png_path() {
        let image = RgbaImage::from_pixel(4, 4, Rgba([32, 64, 128, 255]));
        let bytes = encode_overlay_snapshot(&image).expect("overlay snapshot encoded");
        assert_eq!(overlay_snapshot_mime_type(&bytes), "image/png");
        assert_eq!(bytes, encode_png(&image).expect("png"));
        let decoded = image::load_from_memory(&bytes)
            .expect("overlay snapshot decodes")
            .to_rgba8();
        assert_eq!(decoded.dimensions(), (4, 4));
        assert_eq!(decoded.get_pixel(0, 0).0, [32, 64, 128, 255]);
    }

    #[test]
    fn overlay_snapshot_mime_sniffs_png_and_jpeg_magic() {
        assert_eq!(
            overlay_snapshot_mime_type(&[0x89, b'P', b'N', b'G']),
            "image/png"
        );
        assert_eq!(
            overlay_snapshot_mime_type(&[0xFF, 0xD8, 0xFF, 0xE0]),
            "image/jpeg"
        );
    }

    #[test]
    fn save_capture_writes_a_png_and_avoids_collisions() {
        let directory = tempdir().expect("temporary directory");
        let settings = AppSettings {
            output_directory: directory.path().to_string_lossy().into_owned(),
            ..AppSettings::default()
        };
        let image = RgbaImage::from_pixel(2, 3, Rgba([1, 2, 3, 255]));

        let png = encode_png(&image).expect("capture encoded");
        let path = save_encoded_capture(&png, &settings, "png").expect("capture saved");
        let bytes = std::fs::read(&path).expect("saved capture readable");
        assert_eq!(
            image::ImageFormat::from_path(&path).unwrap(),
            image::ImageFormat::Png
        );
        assert!(!bytes.is_empty());
        assert!(path.exists());
        assert!(!unique_path(directory.path(), "Captures_test", "png").exists());
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
    fn recording_replacement_can_keep_or_change_the_source_path_without_overwriting_another_file() {
        let source_directory = tempdir().expect("source directory");
        let selected_directory = tempdir().expect("selected directory");
        let source = source_directory.path().join("source.mp4");
        std::fs::write(&source, b"master").expect("source written");

        assert_eq!(
            recording_replacement_destination_path_in(
                &source,
                Some(source_directory.path()),
                "source",
                "mp4",
            )
            .expect("same source path"),
            source
        );
        assert_eq!(
            recording_replacement_destination_path_in(
                &source,
                Some(selected_directory.path()),
                "renamed",
                "mp4",
            )
            .expect("renamed destination"),
            selected_directory.path().join("renamed.mp4")
        );

        std::fs::write(selected_directory.path().join("existing.mp4"), b"existing")
            .expect("collision written");
        assert!(
            recording_replacement_destination_path_in(
                &source,
                Some(selected_directory.path()),
                "existing",
                "mp4",
            )
            .is_err()
        );
    }

    #[test]
    fn recording_replacement_can_overwrite_a_known_permanent_save() {
        let recovery_directory = tempdir().expect("recovery directory");
        let captures_directory = tempdir().expect("captures directory");
        let recovery = recovery_directory.path().join("media.mp4");
        let permanent = captures_directory.path().join("Captures_clip.mp4");
        std::fs::write(&recovery, b"recovery").expect("recovery written");
        std::fs::write(&permanent, b"permanent").expect("permanent written");

        assert_eq!(
            recording_replacement_destination_path_in_with_replaceable(
                &recovery,
                Some(captures_directory.path()),
                "Captures_clip",
                "mp4",
                &[&permanent],
            )
            .expect("permanent save is replaceable"),
            permanent
        );
        std::fs::write(captures_directory.path().join("other.mp4"), b"other")
            .expect("other permanent written");
        assert!(
            recording_replacement_destination_path_in_with_replaceable(
                &recovery,
                Some(captures_directory.path()),
                "other",
                "mp4",
                &[&permanent],
            )
            .is_err(),
            "unrelated existing files must still be refused"
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
    fn history_save_atomically_replaces_an_existing_entry() {
        let directory = tempdir().expect("temporary directory");
        let id = uuid::Uuid::new_v4().to_string();
        let mut entry = history_entry(&id, Utc::now().to_rfc3339());
        save_history_capture_in(directory.path(), &entry, b"old-full", b"old-preview")
            .expect("initial history saved");

        entry.size_bytes = 42;
        save_history_capture_in(directory.path(), &entry, b"new-full", b"new-preview")
            .expect("history replaced");

        let saved = directory.path().join(&id);
        assert_eq!(
            std::fs::read(saved.join(HISTORY_IMAGE_FILE)).unwrap(),
            b"new-full"
        );
        assert_eq!(
            std::fs::read(saved.join(HISTORY_PREVIEW_FILE)).unwrap(),
            b"new-preview"
        );
        assert!(std::fs::read_dir(directory.path()).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with('.')
        }));
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
    fn recording_history_stores_recovery_media_and_expires_like_screenshots() {
        let directory = tempdir().expect("temporary directory");
        let now = Utc.with_ymd_and_hms(2026, 7, 19, 12, 0, 0).unwrap();
        let recent_id = uuid::Uuid::new_v4().to_string();
        let expired_id = uuid::Uuid::new_v4().to_string();
        let recent_media = directory.path().join("recent-source.mp4");
        let expired_media = directory.path().join("expired-source.mp4");
        std::fs::write(&recent_media, b"recent-bytes").expect("recent media");
        std::fs::write(&expired_media, b"expired-bytes").expect("expired media");

        let recent = crate::models::history_entry_from_recording(&RecordingArtifact {
            id: recent_id.clone(),
            kind: RecordingKind::Video,
            path: recent_media.to_string_lossy().into_owned(),
            saved_path: None,
            media_url: format!("captures-capture://localhost/media/{recent_id}"),
            poster_url: format!("captures-capture://localhost/poster/{recent_id}"),
            mime_type: "video/mp4".to_owned(),
            duration_ms: 4_200,
            width: 1_920,
            height: 1_080,
            size_bytes: 12,
            dropped_frames: 0,
            has_system_audio: true,
            has_microphone_audio: false,
            created_at: (now - Duration::days(10)).to_rfc3339(),
            target: RecordingTarget::Display {
                display_id: "1".to_owned(),
            },
            missing: false,
        });
        let expired = crate::models::history_entry_from_recording(&RecordingArtifact {
            id: expired_id.clone(),
            kind: RecordingKind::Video,
            path: expired_media.to_string_lossy().into_owned(),
            saved_path: None,
            media_url: format!("captures-capture://localhost/media/{expired_id}"),
            poster_url: format!("captures-capture://localhost/poster/{expired_id}"),
            mime_type: "video/mp4".to_owned(),
            duration_ms: 1_000,
            width: 1_280,
            height: 720,
            size_bytes: 12,
            dropped_frames: 0,
            has_system_audio: false,
            has_microphone_audio: false,
            created_at: (now - Duration::days(40)).to_rfc3339(),
            target: RecordingTarget::Display {
                display_id: "1".to_owned(),
            },
            missing: false,
        });

        save_history_entry_in(
            directory.path(),
            &recent,
            None,
            b"recent-poster",
            Some((&recent_media, "media.mp4")),
        )
        .expect("recent recording history saved");
        save_history_entry_in(
            directory.path(),
            &expired,
            None,
            b"expired-poster",
            Some((&expired_media, "media.mp4")),
        )
        .expect("expired recording history saved");

        let loaded = load_capture_history_from(directory.path(), now).expect("history loaded");
        assert_eq!(
            loaded.len(),
            1,
            "recordings expire after the shared recovery window"
        );
        assert_eq!(loaded[0].id, recent_id);
        assert!(
            directory
                .path()
                .join(&recent_id)
                .join("media.mp4")
                .is_file()
        );
        assert!(!directory.path().join(expired_id).exists());
        assert_eq!(
            std::fs::read(directory.path().join(&recent_id).join("media.mp4")).unwrap(),
            b"recent-bytes",
        );
    }

    #[test]
    fn legacy_recording_history_keeps_external_saved_path_rows() {
        let directory = tempdir().expect("temporary directory");
        let now = Utc.with_ymd_and_hms(2026, 7, 19, 12, 0, 0).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let media_path = directory.path().join("externally-managed.mp4");
        std::fs::write(&media_path, b"legacy").expect("legacy media");
        let entry = crate::models::history_entry_from_recording(&RecordingArtifact {
            id: id.clone(),
            kind: RecordingKind::Video,
            path: media_path.to_string_lossy().into_owned(),
            saved_path: Some(media_path.to_string_lossy().into_owned()),
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
            created_at: (now - Duration::days(10)).to_rfc3339(),
            target: RecordingTarget::Display {
                display_id: "1".to_owned(),
            },
            missing: false,
        });

        save_history_entry_in(directory.path(), &entry, None, b"poster", None)
            .expect("legacy recording history saved");
        let loaded = load_capture_history_from(directory.path(), now).expect("history loaded");

        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].kind, ArtifactKind::Video);
        assert_eq!(
            loaded[0].saved_path.as_deref(),
            Some(media_path.to_string_lossy().as_ref())
        );
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

    fn png_color_type(bytes: &[u8]) -> png::ColorType {
        png::Decoder::new(std::io::Cursor::new(bytes))
            .read_info()
            .expect("png header")
            .info()
            .color_type
    }

    #[test]
    fn low_color_png_export_preserves_exact_pixels() {
        let image = RgbaImage::from_fn(257, 129, |x, y| {
            if (x * 13 + y * 7) % 19 < 9 {
                Rgba([24, 48, 96, 255])
            } else {
                Rgba([216, 192, 160, 128])
            }
        });
        let bytes = encode_png_export(&image, true, Some(32)).expect("PNG export");
        assert_eq!(
            image::load_from_memory(&bytes)
                .expect("PNG decodes")
                .to_rgba8(),
            image
        );
        assert!(bytes.len() <= encode_png(&image).expect("preserve PNG").len());
    }

    #[test]
    #[ignore = "manual release benchmark: --release --ignored --nocapture"]
    fn benchmark_flat_png_export() {
        use std::{hint::black_box, time::Instant};

        for (name, width, height, run_length) in [
            ("1080p-solid", 1920, 1080, u32::MAX),
            ("4k-solid", 3840, 2160, u32::MAX),
            ("1080p-runs-64", 1920, 1080, 64),
            ("1080p-runs-8", 1920, 1080, 8),
            ("1080p-alternating", 1920, 1080, 1),
            ("540p-over-budget", 960, 540, 0),
        ] {
            let image = RgbaImage::from_fn(width, height, |x, y| {
                if run_length == 0 {
                    let mixed = x.wrapping_mul(73) ^ y.wrapping_mul(151) ^ (x * y);
                    Rgba([mixed as u8, (mixed >> 5) as u8, (mixed >> 11) as u8, 255])
                } else {
                    let shade = ((y * width + x) / run_length % 4) as u8;
                    Rgba([shade * 61, shade * 37, shade * 19, 255])
                }
            });
            let mut samples = Vec::new();
            for iteration in 0..4 {
                let start = Instant::now();
                let bytes = encode_png_export(black_box(&image), true, Some(256)).unwrap();
                black_box(&bytes);
                let elapsed = start.elapsed().as_secs_f64() * 1000.0;
                if iteration > 0 {
                    samples.push(elapsed);
                } else {
                    let checksum = bytes.iter().fold(0xcbf29ce484222325_u64, |hash, byte| {
                        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
                    });
                    eprintln!("{name}: bytes={} fnv1a={checksum:016x}", bytes.len());
                    if run_length != 0 {
                        assert_eq!(image::load_from_memory(&bytes).unwrap().to_rgba8(), image);
                    }
                }
            }
            eprintln!("{name}: samples_ms={samples:?}");
        }
    }

    #[test]
    #[ignore = "manual release benchmark: --release --ignored --nocapture"]
    fn benchmark_partial_alpha_png_export() {
        use std::{hint::black_box, time::Instant};

        for (name, width, height, colors, partial_alpha, dither) in [
            ("540p-alpha-2", 960, 540, 2, true, true),
            ("540p-alpha-64", 960, 540, 64, true, true),
            ("540p-alpha-256", 960, 540, 256, true, true),
            ("1080p-alpha-256", 1920, 1080, 256, true, true),
            ("540p-alpha-256-no-dither", 960, 540, 256, true, false),
            ("540p-opaque-256-control", 960, 540, 256, false, true),
        ] {
            let image = RgbaImage::from_fn(width, height, |x, y| {
                let mixed = x.wrapping_mul(73) ^ y.wrapping_mul(151) ^ (x * y);
                let alpha = if partial_alpha && x < width / 20 {
                    (x * 255 / (width / 20)) as u8
                } else {
                    255
                };
                Rgba([mixed as u8, (mixed >> 5) as u8, (mixed >> 11) as u8, alpha])
            });
            let mut samples = Vec::new();
            for iteration in 0..4 {
                let start = Instant::now();
                let bytes =
                    encode_png_export_dithered(black_box(&image), true, Some(colors), dither)
                        .unwrap();
                black_box(&bytes);
                let elapsed = start.elapsed().as_secs_f64() * 1000.0;
                if iteration > 0 {
                    samples.push(elapsed);
                }
                if iteration == 0 {
                    let checksum = bytes.iter().fold(0xcbf29ce484222325_u64, |hash, byte| {
                        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
                    });
                    eprintln!("{name}: bytes={} fnv1a={checksum:016x}", bytes.len());
                }
            }
            eprintln!("{name}: samples_ms={samples:?}");
            samples.sort_by(f64::total_cmp);
            eprintln!("{name}: median_ms={:.3}", samples[1]);
        }
    }

    fn png_has_srgb_chunk(bytes: &[u8]) -> bool {
        bytes.windows(4).any(|window| window == b"sRGB")
    }

    fn png_srgb_intent(bytes: &[u8]) -> Option<png::SrgbRenderingIntent> {
        png::Decoder::new(std::io::Cursor::new(bytes))
            .read_info()
            .expect("png header")
            .info()
            .srgb
    }

    fn mean_saturation(image: &RgbaImage) -> f64 {
        let mut total = 0.0_f64;
        let mut count = 0.0_f64;
        for pixel in image.pixels() {
            let red = f64::from(pixel[0]);
            let green = f64::from(pixel[1]);
            let blue = f64::from(pixel[2]);
            let max = red.max(green).max(blue);
            let min = red.min(green).min(blue);
            if max > 0.0 {
                total += (max - min) / max;
                count += 1.0;
            }
        }
        total / count.max(1.0)
    }

    fn mean_horizontal_neighbor_diff(image: &RgbaImage) -> f64 {
        let mut total = 0.0_f64;
        let mut count = 0.0_f64;
        for y in 0..image.height() {
            for x in 0..image.width().saturating_sub(1) {
                let left = image.get_pixel(x, y).0;
                let right = image.get_pixel(x + 1, y).0;
                total += (0..3)
                    .map(|channel| {
                        f64::from(
                            (i16::from(left[channel]) - i16::from(right[channel])).unsigned_abs(),
                        )
                    })
                    .sum::<f64>();
                count += 1.0;
            }
        }
        total / count.max(1.0)
    }

    /// Smooth saturated illustration: orange wash plus a green “eye”.
    /// Enough unique colors to force quantization at Tiny (32), but still a
    /// gradient so dithering shows up as neighbor speckle instead of a hue shift.
    fn saturated_illustration() -> RgbaImage {
        RgbaImage::from_fn(160, 96, |x, y| {
            let dx = i32::try_from(x).unwrap_or(i32::MAX) - 40;
            let dy = i32::try_from(y).unwrap_or(i32::MAX) - 48;
            if dx.saturating_mul(dx) + dy.saturating_mul(dy) < 14 * 14 {
                let tint = (y % 20) as u8;
                Rgba([30, 200, 70 + tint, 255])
            } else {
                let t = f64::from(x) / 159.0;
                Rgba([
                    255,
                    (40.0 + 80.0 * (1.0 - t)) as u8,
                    (12.0 + 20.0 * t) as u8,
                    255,
                ])
            }
        })
    }

    #[test]
    fn quantized_png_stays_indexed_when_a_pixel_is_transparent() {
        let mut image = RgbaImage::from_pixel(64, 48, Rgba([40, 80, 160, 255]));
        image.put_pixel(0, 0, Rgba([0, 0, 0, 0]));
        image.put_pixel(8, 8, Rgba([220, 40, 40, 180]));
        let bytes = encode_png_export(&image, true, Some(32)).expect("quantized");
        assert_eq!(png_color_type(&bytes), png::ColorType::Indexed);
        image::load_from_memory(&bytes).expect("indexed PNG with alpha is readable");
    }

    #[test]
    fn quantized_png_never_exceeds_the_preserve_encode() {
        // Flat, dark UI-like screenshot: mostly one background color with a few
        // solid panels and hairline borders. These deflate extremely well as
        // RGBA, and the old Paeth-filtered indexed encode produced a *larger*
        // "compressed" file than the original.
        let image = RgbaImage::from_fn(640, 400, |x, y| {
            if y % 100 == 0 || x % 160 == 0 {
                Rgba([58, 59, 66, 255])
            } else if x > 480 && y > 300 {
                Rgba([26, 27, 32, 255])
            } else {
                Rgba([16, 17, 20, 255])
            }
        });
        let preserve = encode_png_export(&image, false, None).expect("preserve");
        for colors in [8, 32, 128, 256] {
            let compressed = encode_png_export(&image, true, Some(colors)).expect("compressed");
            assert!(
                compressed.len() <= preserve.len(),
                "compress ({colors} colors) must not exceed preserve (compressed={}, preserve={})",
                compressed.len(),
                preserve.len()
            );
        }
    }

    #[test]
    fn quantized_png_with_alpha_is_smaller_than_32bit_rgba() {
        let image = RgbaImage::from_fn(220, 150, |x, y| {
            let r = (x.wrapping_mul(17).wrapping_add(y.wrapping_mul(3)) % 256) as u8;
            let g = (x.wrapping_mul(5).wrapping_add(y.wrapping_mul(11)) % 256) as u8;
            let b = (x.wrapping_mul(y).wrapping_add(40) % 256) as u8;
            let alpha = if x < 40 { ((x * 255) / 40) as u8 } else { 255 };
            Rgba([r, g, b, alpha])
        });
        let indexed = encode_png_export(&image, true, Some(48)).expect("indexed");
        let rgba = encode_png_export(&image, true, None).expect("rgba");
        assert_eq!(png_color_type(&indexed), png::ColorType::Indexed);
        assert!(
            indexed.len() < rgba.len(),
            "indexed+tRNS should beat RGBA packing (indexed={}, rgba={})",
            indexed.len(),
            rgba.len()
        );
    }

    #[test]
    fn png_palette_tracks_the_shared_quality_notches() {
        assert_eq!(png_palette_colors_for_quality(55), Some(32));
        assert_eq!(png_palette_colors_for_quality(70), Some(64));
        assert_eq!(png_palette_colors_for_quality(85), Some(128));
        assert_eq!(png_palette_colors_for_quality(92), Some(256));
        assert_eq!(png_palette_colors_for_quality(98), None);
    }

    #[test]
    fn exported_pngs_are_tagged_srgb() {
        let image = saturated_illustration();
        let preserve = encode_png_export(&image, false, None).expect("preserve");
        let compact = encode_png_export(&image, true, None).expect("compact");
        let quantized = encode_png_export(&image, true, Some(32)).expect("quantized");
        for (label, bytes) in [
            ("preserve", preserve.as_slice()),
            ("compact", compact.as_slice()),
            ("quantized", quantized.as_slice()),
        ] {
            assert!(
                png_has_srgb_chunk(bytes),
                "{label} PNG must include an sRGB chunk so color-managed viewers match the canvas"
            );
            assert_eq!(
                png_srgb_intent(bytes),
                Some(png::SrgbRenderingIntent::Perceptual),
                "{label} PNG should declare perceptual sRGB"
            );
        }
    }

    #[test]
    fn quantized_png_keeps_saturation_and_adds_dither_speckle() {
        let image = saturated_illustration();
        let original_saturation = mean_saturation(&image);
        let original_speckle = mean_horizontal_neighbor_diff(&image);
        assert!(
            original_saturation > 0.7,
            "fixture should start saturated (sat={original_saturation})"
        );

        let bytes = encode_png_export(&image, true, Some(32)).expect("tiny PNG");
        assert_eq!(png_color_type(&bytes), png::ColorType::Indexed);
        let compressed = image::load_from_memory(&bytes)
            .expect("tiny PNG is readable")
            .to_rgba8();
        let compressed_saturation = mean_saturation(&compressed);
        let compressed_speckle = mean_horizontal_neighbor_diff(&compressed);

        assert!(
            compressed_saturation > original_saturation * 0.85,
            "palette compression should not wash colors (original={original_saturation}, compressed={compressed_saturation})"
        );
        assert!(
            compressed_speckle > original_speckle * 1.4,
            "32-color PNG should show dither speckle instead of a flat remap (original={original_speckle}, compressed={compressed_speckle})"
        );
    }

    #[test]
    fn undithered_indexed_png_can_beat_dither_on_a_smooth_gradient() {
        let image = RgbaImage::from_fn(240, 80, |x, _y| {
            let t = ((x * 255) / 239) as u8;
            Rgba([t, 48, 220_u8.saturating_sub(t / 2), 255])
        });
        let dithered = encode_png_export_dithered(&image, true, Some(8), true).expect("dithered");
        let undithered =
            encode_png_export_dithered(&image, true, Some(8), false).expect("undithered");
        assert!(
            undithered.len() < dithered.len(),
            "posterized 8-color PNG should deflate smaller than a dithered one (undithered={}, dithered={})",
            undithered.len(),
            dithered.len()
        );
        assert_eq!(png_color_type(&undithered), png::ColorType::Indexed);
    }

    #[test]
    fn partial_alpha_dither_uses_two_row_error_and_keeps_saturation() {
        let mut image = saturated_illustration();
        for x in 0..image.width() {
            image.put_pixel(x, 0, Rgba([0, 0, 0, 0]));
            image.put_pixel(x, image.height() - 1, Rgba([40, 80, 160, 120]));
        }
        let original_saturation = mean_saturation(&image);
        let bytes = encode_png_export(&image, true, Some(32)).expect("partial-alpha tiny PNG");
        assert_eq!(png_color_type(&bytes), png::ColorType::Indexed);
        let compressed = image::load_from_memory(&bytes)
            .expect("readable")
            .to_rgba8();
        assert!(
            mean_saturation(&compressed) > original_saturation * 0.8,
            "partial-alpha dither should not wash colors"
        );
    }
}
