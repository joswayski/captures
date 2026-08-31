use std::{
    collections::HashMap,
    fs::{self, File},
    io::Write,
    path::{Component, Path, PathBuf},
};

use chrono::{DateTime, Duration, Local, Utc};
use image::{
    RgbImage, RgbaImage,
    codecs::png::{CompressionType, FilterType},
};
use quantette::{ImageBuf, PaletteSize, Pipeline, dither::FloydSteinberg};
use uuid::Uuid;

use crate::{
    AppError,
    models::{
        AppSettings, ArtifactKind, CaptureArtifact, HISTORY_RETENTION_DAYS, HistoryEntry,
        RecordingArtifactData, find_history_recording_media, history_recording_media_file_name,
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
    load_capture_history_from(&crate::models::history_directory(), Utc::now())
}

pub fn save_history_capture(
    entry: &HistoryEntry,
    image_png: &[u8],
    preview_png: &[u8],
) -> Result<(), AppError> {
    let directory = crate::models::history_directory();
    save_history_capture_in(&directory, entry, image_png, preview_png)?;
    // Expired entries are pruned when history is loaded at launch, not after
    // every save, so capture latency does not grow with history size.
    Ok(())
}

/// Save a recording into private capture history.
///
/// Copies `media_source` into the history entry as recovery media (retained for
/// [`HISTORY_RETENTION_DAYS`]). `entry.saved_path` should only be set when the
/// user has also permanently saved a Captures-folder copy.
pub fn save_history_recording(
    entry: &HistoryEntry,
    poster_png: &[u8],
    media_source: &Path,
) -> Result<PathBuf, AppError> {
    if entry.kind == ArtifactKind::Screenshot || entry.kind.recording_kind().is_none() {
        return Err(AppError::Task(
            "recording history metadata is incomplete".to_owned(),
        ));
    }
    if !media_source.is_file() {
        return Err(AppError::Task(
            "recording media is no longer available".to_owned(),
        ));
    }
    let media_name = history_recording_media_file_name(entry.kind, media_source)
        .ok_or_else(|| AppError::Task("recording history metadata is incomplete".to_owned()))?;
    let directory = crate::models::history_directory();
    let recovery_path = save_history_entry_in(
        &directory,
        entry,
        None,
        poster_png,
        Some((media_source, media_name.as_str())),
    )?;
    recovery_path
        .ok_or_else(|| AppError::Task("recording history media was not written".to_owned()))
}

/// Rewrite history metadata in place (for example after a permanent save) without
/// replacing recovery media already stored in the entry directory.
pub fn update_history_entry_metadata(entry: &HistoryEntry) -> Result<(), AppError> {
    let directory = history_entry_directory(&crate::models::history_directory(), &entry.id)?;
    if !directory.is_dir() {
        return Err(AppError::HistoryUnavailable);
    }
    let temporary = directory.join(format!(".{}.metadata.tmp", Uuid::new_v4()));
    fs::write(&temporary, serde_json::to_vec_pretty(entry)?)?;
    let destination = directory.join(HISTORY_METADATA_FILE);
    fs::rename(temporary, destination)?;
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
    save_history_entry_in(root, entry, Some(image_png), preview_png, None).map(|_| ())
}

fn save_history_entry_in(
    root: &Path,
    entry: &HistoryEntry,
    image_png: Option<&[u8]>,
    preview_png: &[u8],
    media: Option<(&Path, &str)>,
) -> Result<Option<PathBuf>, AppError> {
    fs::create_dir_all(root)?;
    let destination = history_entry_directory(root, &entry.id)?;
    let temporary = root.join(format!(".{}.{}.tmp", entry.id, Uuid::new_v4()));
    let backup = root.join(format!(".{}.{}.bak", entry.id, Uuid::new_v4()));
    fs::create_dir(&temporary)?;

    let result = (|| {
        if let Some(image_png) = image_png {
            fs::write(temporary.join(HISTORY_IMAGE_FILE), image_png)?;
        }
        fs::write(temporary.join(HISTORY_PREVIEW_FILE), preview_png)?;
        let recovery_media = if let Some((media_source, media_name)) = media {
            let media_destination = temporary.join(media_name);
            // Same-path copies are no-ops on some platforms; skip when identical.
            if media_source != media_destination.as_path() {
                fs::copy(media_source, &media_destination)?;
            }
            Some(media_destination)
        } else if let Some(existing) = find_history_recording_media(&destination) {
            // Preserve existing recovery media when only metadata/poster change.
            let media_name = existing
                .file_name()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("media.bin"));
            let preserved = temporary.join(media_name);
            fs::copy(&existing, &preserved)?;
            Some(preserved)
        } else {
            None
        };
        fs::write(
            temporary.join(HISTORY_METADATA_FILE),
            serde_json::to_vec_pretty(entry)?,
        )?;
        if destination.exists() {
            fs::rename(&destination, &backup)?;
            if let Err(error) = fs::rename(&temporary, &destination) {
                let rollback = fs::rename(&backup, &destination);
                return Err(match rollback {
                    Ok(()) => error.into(),
                    Err(rollback_error) => AppError::Task(format!(
                        "capture history could not be replaced ({error}), and the previous entry could not be restored ({rollback_error}); its backup remains at {}",
                        backup.display()
                    )),
                });
            }
            let _ = fs::remove_dir_all(&backup);
        } else {
            fs::rename(&temporary, &destination)?;
        }
        let final_media =
            recovery_media.map(|path| destination.join(path.file_name().expect("media file name")));
        Ok::<_, AppError>(final_media)
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
        // Screenshots and recordings share the same recovery window. Permanent
        // Captures-folder saves (entry.saved_path) are not deleted by this prune.
        if created_at < cutoff {
            let _ = fs::remove_dir_all(path);
            continue;
        }
        let files_are_valid = path.join(HISTORY_PREVIEW_FILE).is_file()
            && match history_entry.kind {
                ArtifactKind::Screenshot => {
                    history_entry.mode.is_some() && path.join(HISTORY_IMAGE_FILE).is_file()
                }
                ArtifactKind::Video | ArtifactKind::Gif => {
                    let has_recovery_media = find_history_recording_media(&path).is_some();
                    let has_permanent_media = history_entry
                        .saved_path
                        .as_ref()
                        .is_some_and(|saved| Path::new(saved).is_file());
                    // Keep legacy metadata rows that still point at an external path
                    // even if that file is currently missing (surface as missing).
                    history_entry.recording_artifact().is_some()
                        && (has_recovery_media
                            || has_permanent_media
                            || history_entry.saved_path.is_some())
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

/// Encode a PNG for user export.
///
/// - Preserve (`compact = false`): fast lossless packing, identical pixels.
/// - Compact without a color budget: stronger lossless packing only.
/// - Compact with `max_colors`: lossy **dithered** color quantization, then an
///   indexed PNG (1 byte/pixel) with optional `tRNS` for alpha. Window shadows
///   and transparent canvas padding stay palettized instead of falling back to
///   32-bit RGBA. Files are tagged sRGB so the compressed preview does not pick
///   up a gamma wash on color-managed displays.
pub fn encode_png_export(
    image: &RgbaImage,
    compact: bool,
    max_colors: Option<u16>,
) -> Result<Vec<u8>, AppError> {
    encode_png_export_dithered(image, compact, max_colors, true)
}

/// Same as [`encode_png_export`], with control over Floyd–Steinberg dithering.
/// Maximum-size search tries the undithered variant when dither noise inflates
/// the deflate stream past the lossless encode.
pub fn encode_png_export_dithered(
    image: &RgbaImage,
    compact: bool,
    max_colors: Option<u16>,
    dither: bool,
) -> Result<Vec<u8>, AppError> {
    if let Some(colors) = max_colors.filter(|count| *count > 0) {
        let quantized = encode_png_quantized(image, colors, dither)?;
        // Quantization is not guaranteed to shrink already-efficient images
        // (flat UI screenshots often deflate better as full RGBA than as a
        // dithered palette). Never let Compress produce a bigger file than the
        // Preserve encode of the same pixels.
        let lossless = encode_png_with_filter(image, FilterType::Sub)?;
        return Ok(if lossless.len() < quantized.len() {
            lossless
        } else {
            quantized
        });
    }
    if compact {
        encode_png_with_quality(image, CompressionType::Best, FilterType::Adaptive)
    } else {
        encode_png_with_filter(image, FilterType::Sub)
    }
}

/// Map the shared compress quality notch (also used for JPEG) to a PNG palette size.
/// Higher quality → more colors kept → larger file. `None` means compact lossless
/// packing only (Highest): same pixels, no color quantization.
pub fn png_palette_colors_for_quality(quality: u8) -> Option<u16> {
    match quality {
        0..=59 => Some(32),   // Tiny (~55)
        60..=77 => Some(64),  // Smaller (~70)
        78..=88 => Some(128), // Balanced (~85)
        89..=94 => Some(256), // High (~92)
        _ => None,            // Highest (~98): tighter packing, no palette
    }
}

/// Palette sizes tried when a hard maximum file size is requested for PNG.
pub const PNG_MAXIMUM_COLOR_STEPS: [u16; 10] = [256, 192, 128, 96, 64, 48, 32, 24, 16, 8];

fn encode_png_quantized(
    image: &RgbaImage,
    max_colors: u16,
    dither: bool,
) -> Result<Vec<u8>, AppError> {
    let width = image.width();
    let height = image.height();
    if width == 0 || height == 0 {
        return encode_png_with_quality(image, CompressionType::Best, FilterType::Adaptive);
    }

    let colors = max_colors.clamp(2, 256);
    let (palette, indices) = match index_image(image, colors, dither) {
        Ok(indexed) => indexed,
        Err(_) => {
            return encode_png_with_quality(image, CompressionType::Best, FilterType::Adaptive);
        }
    };
    if palette.is_empty() || indices.len() != (width as usize) * (height as usize) {
        return encode_png_with_quality(image, CompressionType::Best, FilterType::Adaptive);
    }
    encode_indexed_png(width, height, &palette, &indices)
}

fn index_image(
    image: &RgbaImage,
    max_colors: u16,
    dither: bool,
) -> Result<(Vec<[u8; 4]>, Vec<u8>), AppError> {
    if let Some(exact) = exact_indexed_rgba(image, max_colors) {
        return Ok(exact);
    }

    let has_transparency = image.pixels().any(|pixel| pixel[3] < 255);
    if !has_transparency {
        return quantette_rgb_indexed(image, max_colors, dither);
    }
    let has_partial_alpha = image.pixels().any(|pixel| pixel[3] > 0 && pixel[3] < 255);
    if has_partial_alpha {
        Ok(median_cut_rgba(image, max_colors, dither))
    } else {
        quantette_rgb_binary_alpha(image, max_colors, dither)
    }
}

fn exact_indexed_rgba(image: &RgbaImage, max_colors: u16) -> Option<(Vec<[u8; 4]>, Vec<u8>)> {
    let limit = usize::from(max_colors);
    let mut map = HashMap::new();
    let mut palette = Vec::new();
    let mut indices = Vec::with_capacity(rgba_pixel_count(image));
    for pixel in image.pixels() {
        if let Some(&index) = map.get(&pixel.0) {
            indices.push(index);
            continue;
        }
        if palette.len() >= limit {
            return None;
        }
        let index = u8::try_from(palette.len()).ok()?;
        map.insert(pixel.0, index);
        palette.push(pixel.0);
        indices.push(index);
    }
    Some((palette, indices))
}

fn quantette_rgb_indexed(
    image: &RgbaImage,
    max_colors: u16,
    dither: bool,
) -> Result<(Vec<[u8; 4]>, Vec<u8>), AppError> {
    let indexed = quantette_rgb(image, max_colors, dither)?;
    let palette = indexed
        .palette()
        .iter()
        .map(|color| [color.red, color.green, color.blue, 255])
        .collect();
    Ok((palette, indexed.indices().to_vec()))
}

fn quantette_rgb_binary_alpha(
    image: &RgbaImage,
    max_colors: u16,
    dither: bool,
) -> Result<(Vec<[u8; 4]>, Vec<u8>), AppError> {
    let budget = max_colors.saturating_sub(1).clamp(2, 255);
    let indexed = quantette_rgb(image, budget, dither)?;
    let mut palette = Vec::with_capacity(indexed.palette().len() + 1);
    palette.push([0, 0, 0, 0]);
    palette.extend(
        indexed
            .palette()
            .iter()
            .map(|color| [color.red, color.green, color.blue, 255]),
    );
    let mut indices = Vec::with_capacity(rgba_pixel_count(image));
    for (offset, pixel) in image.pixels().enumerate() {
        if pixel[3] == 0 {
            indices.push(0);
        } else {
            indices.push(indexed.indices()[offset].saturating_add(1));
        }
    }
    Ok((palette, indices))
}

fn quantette_rgb(
    image: &RgbaImage,
    max_colors: u16,
    dither: bool,
) -> Result<quantette::IndexedImage<quantette::deps::palette::Srgb<u8>>, AppError> {
    let rgb = RgbImage::from_fn(image.width(), image.height(), |x, y| {
        let pixel = image.get_pixel(x, y);
        image::Rgb([pixel[0], pixel[1], pixel[2]])
    });
    let quant_image = ImageBuf::try_from(rgb).map_err(|error| {
        AppError::Image(format!(
            "could not prepare image for PNG compression: {error}"
        ))
    })?;
    let pipeline = Pipeline::new()
        .palette_size(PaletteSize::from_u16_clamped(max_colors.clamp(2, 256)))
        .parallel(true);
    // Full error diffusion: optical mixing keeps hues closer to the original
    // while the leftover error reads as speckle / pixelation instead of a
    // global wash. Disable dedup — it fights dithering on busy screenshots.
    let pipeline = if dither {
        let ditherer = FloydSteinberg::with_error_diffusion(1.0).unwrap_or_default();
        pipeline.ditherer(ditherer).dedup(false)
    } else {
        pipeline.ditherer(None)
    };
    let indexed = pipeline
        .input_image(quant_image.as_ref())
        .output_srgb8_indexed_image();
    if indexed.palette().is_empty() || indexed.indices().is_empty() {
        return Err(AppError::Image(
            "PNG color quantization produced an empty palette".to_owned(),
        ));
    }
    Ok(indexed)
}

fn rgba_pixel_count(image: &RgbaImage) -> usize {
    usize::try_from(u64::from(image.width()).saturating_mul(u64::from(image.height())))
        .unwrap_or(usize::MAX)
}

fn median_cut_rgba(image: &RgbaImage, max_colors: u16, dither: bool) -> (Vec<[u8; 4]>, Vec<u8>) {
    let pixel_count = rgba_pixel_count(image);
    let target = usize::from(max_colors)
        .clamp(2, 256)
        .min(pixel_count.max(1));
    let mut boxes = vec![ColorBox {
        members: (0..u32::try_from(pixel_count).unwrap_or(u32::MAX)).collect(),
    }];

    while boxes.len() < target {
        let Some((split_at, channel)) = next_split(&boxes, image) else {
            break;
        };
        let mut members = std::mem::take(&mut boxes[split_at].members);
        members.sort_unstable_by_key(|&index| rgba_at(image, index)[channel]);
        let mid = members.len() / 2;
        if mid == 0 || mid == members.len() {
            boxes[split_at].members = members;
            break;
        }
        let right = members.split_off(mid);
        boxes[split_at].members = members;
        boxes.push(ColorBox { members: right });
    }

    let mut palette = Vec::with_capacity(boxes.len());
    for color_box in &boxes {
        palette.push(box_representative(image, &color_box.members));
    }
    if palette.is_empty() {
        palette.push([0, 0, 0, 255]);
    }
    let indices = if dither {
        dither_rgba_indices(image, &palette)
    } else {
        nearest_rgba_indices(image, &palette)
    };
    (palette, indices)
}

struct ColorBox {
    members: Vec<u32>,
}

fn next_split(boxes: &[ColorBox], image: &RgbaImage) -> Option<(usize, usize)> {
    let mut best: Option<(usize, usize, u8)> = None;
    for (box_index, color_box) in boxes.iter().enumerate() {
        if color_box.members.len() < 2 {
            continue;
        }
        let (min, max) = box_bounds(image, &color_box.members);
        let channel = (0..4)
            .max_by_key(|&channel| max[channel].saturating_sub(min[channel]))
            .unwrap_or(0);
        let range = max[channel].saturating_sub(min[channel]);
        if range == 0 {
            continue;
        }
        if best.is_none_or(|(_, _, best_range)| range > best_range) {
            best = Some((box_index, channel, range));
        }
    }
    best.map(|(box_index, channel, _)| (box_index, channel))
}

fn box_bounds(image: &RgbaImage, members: &[u32]) -> ([u8; 4], [u8; 4]) {
    let mut min = [255_u8; 4];
    let mut max = [0_u8; 4];
    for &index in members {
        let pixel = rgba_at(image, index);
        for channel in 0..4 {
            min[channel] = min[channel].min(pixel[channel]);
            max[channel] = max[channel].max(pixel[channel]);
        }
    }
    (min, max)
}

fn box_centroid(image: &RgbaImage, members: &[u32]) -> [u8; 4] {
    if members.is_empty() {
        return [0, 0, 0, 255];
    }
    let mut sum = [0_u64; 4];
    for &index in members {
        let pixel = rgba_at(image, index);
        for channel in 0..4 {
            sum[channel] += u64::from(pixel[channel]);
        }
    }
    let count = u64::try_from(members.len()).unwrap_or(1);
    [
        (sum[0] / count) as u8,
        (sum[1] / count) as u8,
        (sum[2] / count) as u8,
        (sum[3] / count) as u8,
    ]
}

/// Prefer a real pixel from the box over the RGB mean. Averages of saturated
/// colors drift toward gray; a medoid keeps the original hue.
fn box_representative(image: &RgbaImage, members: &[u32]) -> [u8; 4] {
    if members.is_empty() {
        return [0, 0, 0, 255];
    }
    let centroid = box_centroid(image, members);
    let mut best = members[0];
    let mut best_dist = u32::MAX;
    for &member in members {
        let dist = rgba_dist2(rgba_at(image, member), centroid);
        if dist < best_dist {
            best_dist = dist;
            best = member;
        }
    }
    rgba_at(image, best)
}

fn rgba_dist2(left: [u8; 4], right: [u8; 4]) -> u32 {
    (0..4).fold(0_u32, |sum, channel| {
        let delta = i32::from(left[channel]) - i32::from(right[channel]);
        sum.saturating_add(u32::try_from(delta.saturating_mul(delta)).unwrap_or(u32::MAX))
    })
}

/// Nearest palette lookup. Linear scan is enough for tiny palettes; a 4D k-d
/// tree keeps High (256 colors) from doing a full scan on every pixel.
struct PaletteIndex<'a> {
    colors: &'a [[u8; 4]],
    nodes: Vec<KdNode>,
}

struct KdNode {
    color_index: u8,
    axis: u8,
    left: Option<u16>,
    right: Option<u16>,
}

impl<'a> PaletteIndex<'a> {
    fn new(colors: &'a [[u8; 4]]) -> Self {
        let mut nodes = Vec::with_capacity(colors.len());
        if !colors.is_empty() {
            let mut order: Vec<u8> = (0..colors.len())
                .filter_map(|index| u8::try_from(index).ok())
                .collect();
            build_kd_node(colors, &mut order, 0, &mut nodes);
        }
        Self { colors, nodes }
    }

    fn nearest(&self, query: [u8; 4]) -> u8 {
        if self.colors.is_empty() {
            return 0;
        }
        if self.colors.len() <= 16 || self.nodes.is_empty() {
            return nearest_linear(self.colors, query);
        }
        let mut best_index = self.nodes[0].color_index;
        let mut best_dist = rgba_dist2(query, self.colors[usize::from(best_index)]);
        search_kd(self, 0, query, &mut best_index, &mut best_dist);
        best_index
    }
}

fn nearest_linear(colors: &[[u8; 4]], query: [u8; 4]) -> u8 {
    let mut best_index = 0_u8;
    let mut best_dist = u32::MAX;
    for (palette_index, candidate) in colors.iter().enumerate() {
        let dist = rgba_dist2(query, *candidate);
        if dist < best_dist {
            best_dist = dist;
            best_index = u8::try_from(palette_index).unwrap_or(255);
        }
    }
    best_index
}

fn build_kd_node(
    colors: &[[u8; 4]],
    order: &mut [u8],
    depth: usize,
    nodes: &mut Vec<KdNode>,
) -> Option<u16> {
    if order.is_empty() {
        return None;
    }
    let axis = u8::try_from(depth % 4).unwrap_or(0);
    order.sort_unstable_by_key(|&index| colors[usize::from(index)][usize::from(axis)]);
    let mid = order.len() / 2;
    let node_id = u16::try_from(nodes.len()).ok()?;
    nodes.push(KdNode {
        color_index: order[mid],
        axis,
        left: None,
        right: None,
    });
    let left = build_kd_node(colors, &mut order[..mid], depth.saturating_add(1), nodes);
    let right = build_kd_node(
        colors,
        &mut order[mid.saturating_add(1)..],
        depth.saturating_add(1),
        nodes,
    );
    if let Some(node) = nodes.get_mut(usize::from(node_id)) {
        node.left = left;
        node.right = right;
    }
    Some(node_id)
}

fn search_kd(
    index: &PaletteIndex<'_>,
    node_id: usize,
    query: [u8; 4],
    best_index: &mut u8,
    best_dist: &mut u32,
) {
    let Some(node) = index.nodes.get(node_id) else {
        return;
    };
    let color = index.colors[usize::from(node.color_index)];
    let dist = rgba_dist2(query, color);
    if dist < *best_dist {
        *best_dist = dist;
        *best_index = node.color_index;
    }
    let axis = usize::from(node.axis);
    let delta = i32::from(query[axis]) - i32::from(color[axis]);
    let (near, far) = if delta <= 0 {
        (node.left, node.right)
    } else {
        (node.right, node.left)
    };
    if let Some(child) = near {
        search_kd(index, usize::from(child), query, best_index, best_dist);
    }
    let plane = u32::try_from(delta.saturating_mul(delta)).unwrap_or(u32::MAX);
    if let Some(child) = far
        && plane < *best_dist
    {
        search_kd(index, usize::from(child), query, best_index, best_dist);
    }
}

fn nearest_rgba_indices(image: &RgbaImage, palette: &[[u8; 4]]) -> Vec<u8> {
    let lookup = PaletteIndex::new(palette);
    image
        .pixels()
        .map(|pixel| lookup.nearest(pixel.0))
        .collect()
}

/// Floyd–Steinberg remap so partial-alpha screenshots get speckle instead of a
/// flat, desaturated nearest-color assignment. Only the current and next rows
/// of diffusion error are kept, so a 4K window capture does not allocate an
/// 800 MB error plane.
fn dither_rgba_indices(image: &RgbaImage, palette: &[[u8; 4]]) -> Vec<u8> {
    let width = usize::try_from(image.width()).unwrap_or(0);
    let height = usize::try_from(image.height()).unwrap_or(0);
    let pixel_count = width.saturating_mul(height);
    let mut indices = vec![0_u8; pixel_count];
    if palette.is_empty() || width == 0 || height == 0 {
        return indices;
    }
    let lookup = PaletteIndex::new(palette);
    let mut current_error = vec![[0_i16; 4]; width];
    let mut next_error = vec![[0_i16; 4]; width];
    for y in 0..height {
        for x in 0..width {
            let offset = y * width + x;
            let pixel = image.get_pixel(x as u32, y as u32).0;
            let mut color = [0_i16; 4];
            for channel in 0..4 {
                color[channel] =
                    (i16::from(pixel[channel]) + current_error[x][channel]).clamp(0, 255);
            }
            let query = [
                color[0] as u8,
                color[1] as u8,
                color[2] as u8,
                color[3] as u8,
            ];
            let best_index = lookup.nearest(query);
            indices[offset] = best_index;
            let chosen = palette[usize::from(best_index)];
            let mut quant_error = [0_i16; 4];
            for channel in 0..4 {
                quant_error[channel] = color[channel] - i16::from(chosen[channel]);
            }
            add_row_error(&mut current_error, x.saturating_add(1), quant_error, 7);
            if x > 0 {
                add_row_error(&mut next_error, x - 1, quant_error, 3);
            }
            add_row_error(&mut next_error, x, quant_error, 5);
            add_row_error(&mut next_error, x.saturating_add(1), quant_error, 1);
        }
        std::mem::swap(&mut current_error, &mut next_error);
        next_error.fill([0; 4]);
    }
    indices
}

fn add_row_error(row: &mut [[i16; 4]], x: usize, quant_error: [i16; 4], numerator: i16) {
    let Some(slot) = row.get_mut(x) else {
        return;
    };
    for channel in 0..4 {
        slot[channel] = slot[channel].saturating_add((quant_error[channel] * numerator) / 16);
    }
}

fn rgba_at(image: &RgbaImage, index: u32) -> [u8; 4] {
    let width = image.width().max(1);
    let x = index % width;
    let y = index / width;
    image.get_pixel(x, y).0
}

/// Tag PNG output as sRGB with matching gAMA/cHRM. Untagged PNGs are treated as
/// generic RGB (gamma 1.8) on macOS ColorSync, so the compressed `<img>` preview
/// looks washed out next to the sRGB canvas even when the pixels did not change.
fn mark_png_as_srgb(encoder: &mut png::Encoder<&mut Vec<u8>>) {
    encoder.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
    encoder.set_source_gamma(png::ScaledFloat::from_scaled(45_455));
    encoder.set_source_chromaticities(png::SourceChromaticities::new(
        (0.3127, 0.3290),
        (0.6400, 0.3300),
        (0.3000, 0.6000),
        (0.1500, 0.0600),
    ));
}

fn encode_indexed_png(
    width: u32,
    height: u32,
    palette_colors: &[[u8; 4]],
    indices: &[u8],
) -> Result<Vec<u8>, AppError> {
    let mut palette = Vec::with_capacity(palette_colors.len().max(1) * 3);
    let mut trns = Vec::with_capacity(palette_colors.len());
    let mut has_transparency = false;
    for color in palette_colors {
        palette.push(color[0]);
        palette.push(color[1]);
        palette.push(color[2]);
        trns.push(color[3]);
        has_transparency |= color[3] < 255;
    }
    // png crate requires a non-empty palette for indexed images.
    if palette.is_empty() {
        palette.extend_from_slice(&[0, 0, 0]);
        trns.push(255);
    }

    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(png::ColorType::Indexed);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Best);
        // Palette indices are categorical, not intensities: byte-difference
        // filters like Paeth add noise and inflate the deflate stream, which
        // could make a "compressed" PNG larger than the original.
        encoder.set_filter(png::FilterType::NoFilter);
        mark_png_as_srgb(&mut encoder);
        encoder.set_palette(palette);
        if has_transparency {
            encoder.set_trns(trns);
        }
        let mut writer = encoder
            .write_header()
            .map_err(|error| AppError::Image(error.to_string()))?;
        writer
            .write_image_data(indices)
            .map_err(|error| AppError::Image(error.to_string()))?;
    }
    Ok(bytes)
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
    encode_png_with_quality(image, CompressionType::Fast, filter)
}

fn encode_png_with_quality(
    image: &RgbaImage,
    compression: CompressionType,
    filter: FilterType,
) -> Result<Vec<u8>, AppError> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, image.width(), image.height());
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(match compression {
            CompressionType::Best => png::Compression::Best,
            CompressionType::Default => png::Compression::Default,
            _ => png::Compression::Fast,
        });
        match filter {
            FilterType::Adaptive => {
                encoder.set_adaptive_filter(png::AdaptiveFilterType::Adaptive);
                encoder.set_filter(png::FilterType::Paeth);
            }
            FilterType::Sub => encoder.set_filter(png::FilterType::Sub),
            FilterType::NoFilter => encoder.set_filter(png::FilterType::NoFilter),
            FilterType::Up => encoder.set_filter(png::FilterType::Up),
            FilterType::Avg => encoder.set_filter(png::FilterType::Avg),
            FilterType::Paeth => encoder.set_filter(png::FilterType::Paeth),
            _ => encoder.set_filter(png::FilterType::Paeth),
        }
        mark_png_as_srgb(&mut encoder);
        let mut writer = encoder
            .write_header()
            .map_err(|error| AppError::Image(error.to_string()))?;
        writer
            .write_image_data(image.as_raw())
            .map_err(|error| AppError::Image(error.to_string()))?;
    }
    Ok(bytes)
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
    use captures_recording::{RecordingKind, RecordingTarget};
    use chrono::{Duration, TimeZone, Utc};
    use image::{Rgba, RgbaImage};
    use tempfile::tempdir;

    use super::{
        DRAG_EXPORT_DIRECTORY, DRAG_ICON_FILE, DRAG_ICON_HEIGHT, DRAG_ICON_WIDTH,
        HISTORY_IMAGE_FILE, HISTORY_PREVIEW_FILE, clear_drag_exports_in, encode_drag_icon_png,
        encode_png, encode_png_export, encode_png_export_dithered, encode_preview_png,
        encode_thumbnail_png, load_capture_history_from, png_palette_colors_for_quality,
        prepare_artifact_drag_in, recording_destination_path, recording_destination_path_in,
        recording_replacement_destination_path_in,
        recording_replacement_destination_path_in_with_replaceable, save_encoded_capture,
        save_history_capture_in, save_history_entry_in, save_settings_to, unique_path,
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

        let recent = HistoryEntry::from_recording(&RecordingArtifact {
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
        let expired = HistoryEntry::from_recording(&RecordingArtifact {
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
        let entry = HistoryEntry::from_recording(&RecordingArtifact {
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
            remote_asset_id: None,
            remote_share_url: None,
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
    fn palette_kd_tree_matches_linear_nearest() {
        let palette: Vec<[u8; 4]> = (0..32_u8)
            .map(|index| {
                [
                    index.wrapping_mul(7),
                    index.wrapping_mul(11),
                    index.wrapping_mul(3),
                    255 - index,
                ]
            })
            .collect();
        let lookup = super::PaletteIndex::new(&palette);
        for red in (0..=255).step_by(19) {
            for green in (0..=255).step_by(23) {
                for blue in (0..=255).step_by(29) {
                    let query = [red, green, blue, 200];
                    let kd = lookup.nearest(query);
                    let linear = super::nearest_linear(&palette, query);
                    assert_eq!(
                        super::rgba_dist2(query, palette[usize::from(kd)]),
                        super::rgba_dist2(query, palette[usize::from(linear)]),
                        "kd-tree must find a nearest color for {query:?} (kd={kd}, linear={linear})"
                    );
                }
            }
        }
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
