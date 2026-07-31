use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
};

use chrono::{Local, Utc};
use image::{
    ExtendedColorType, ImageEncoder, ImageFormat, Rgb, RgbImage, RgbaImage,
    codecs::{jpeg::JpegEncoder, webp::WebPEncoder},
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder, window::Color};
use uuid::Uuid;

use captures_capture::CaptureMode;

use crate::{
    AppError, CommandResult,
    models::{
        ArtifactKind, CaptureArtifact, ClipboardCopyStatus, HistoryEntry, artifact_full_url,
        artifact_url, history_full_url, history_preview_url,
    },
    state::AppState,
    storage,
};

pub(crate) const SCREENSHOT_EDITOR_WINDOW_PREFIX: &str = "screenshot-editor-";
const MAX_EDITOR_PNG_BYTES: usize = 256 * 1024 * 1024;
const MAX_EDITOR_DIMENSION: u32 = 16_384;
const MAX_EDITOR_PIXELS: u64 = 100_000_000;

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScreenshotEditFormat {
    Png,
    Jpeg,
    Webp,
}

impl ScreenshotEditFormat {
    const fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Webp => "webp",
        }
    }

    fn extension_matches(self, extension: &str) -> bool {
        match self {
            Self::Png => extension.eq_ignore_ascii_case("png"),
            Self::Jpeg => {
                extension.eq_ignore_ascii_case("jpg") || extension.eq_ignore_ascii_case("jpeg")
            }
            Self::Webp => extension.eq_ignore_ascii_case("webp"),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ScreenshotEditSaveRequest {
    artifact_id: String,
    destination_path: String,
    format: ScreenshotEditFormat,
    jpeg_quality: u8,
    #[serde(default)]
    max_size_bytes: Option<u64>,
    image_png: Vec<u8>,
}

#[derive(Debug, Serialize)]
pub struct SavedScreenshotEdit {
    artifact: CaptureArtifact,
    path: String,
    format: ScreenshotEditFormat,
}

#[tauri::command(async)]
pub fn open_screenshot_editor(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    artifact_id: String,
) -> CommandResult<()> {
    let available = state
        .artifacts
        .lock()
        .iter()
        .any(|artifact| artifact.id == artifact_id);
    if !available {
        return Err("the screenshot is no longer available".to_owned());
    }

    let label = format!("{SCREENSHOT_EDITOR_WINDOW_PREFIX}{artifact_id}");
    if let Some(window) = app.get_webview_window(&label) {
        window.show().map_err(|error| error.to_string())?;
        window.unminimize().map_err(|error| error.to_string())?;
        window.set_focus().map_err(|error| error.to_string())?;
        return Ok(());
    }

    let window = WebviewWindowBuilder::new(
        &app,
        label,
        WebviewUrl::App(
            format!("index.html?view=screenshot-editor&artifact_id={artifact_id}").into(),
        ),
    )
    .title("Captures Screenshot Editor")
    .inner_size(1_280.0, 840.0)
    .min_inner_size(860.0, 600.0)
    .center()
    .resizable(true)
    .disable_drag_drop_handler()
    .background_color(Color(21, 22, 25, 255))
    .focused(false)
    .visible(false)
    .build()
    .map_err(|error| error.to_string())?;
    window.show().map_err(|error| error.to_string())?;
    window.set_focus().map_err(|error| error.to_string())
}

#[tauri::command]
pub fn default_screenshot_edit_path(
    state: tauri::State<'_, Arc<AppState>>,
    artifact_id: String,
    format: ScreenshotEditFormat,
) -> CommandResult<String> {
    // The editor keeps the image in memory, so export still works after the
    // original capture is removed from history. Fall back to the output folder.
    let artifact = state
        .artifacts
        .lock()
        .iter()
        .find(|artifact| artifact.id == artifact_id)
        .cloned();
    let source = artifact
        .as_ref()
        .and_then(|artifact| artifact.path.as_deref())
        .map(Path::new);
    let directory = source
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(state.settings().output_directory));
    let stem = source
        .and_then(Path::file_stem)
        .and_then(|value| value.to_str())
        .filter(|value| !value.trim().is_empty())
        .map_or_else(
            || format!("Captures_{}", Local::now().format("%Y-%m-%d_%H-%M-%S_%3f")),
            str::to_owned,
        );
    Ok(unique_edit_path(&directory, &stem, format.extension())
        .to_string_lossy()
        .into_owned())
}

#[tauri::command]
pub async fn copy_screenshot_edit(app: AppHandle, image_png: Vec<u8>) -> CommandResult<()> {
    let image = tauri::async_runtime::spawn_blocking(move || decode_editor_png(&image_png))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
    super::copy_to_clipboard(&app, image)
        .await
        .map(|_| ())
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn save_screenshot_edit(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    request: ScreenshotEditSaveRequest,
) -> CommandResult<SavedScreenshotEdit> {
    // Export is based on the in-memory editor canvas, not the original file.
    // If the user deleted the source capture while the editor is open, still
    // allow saving a new copy from the edited pixels.
    let source = state
        .artifacts
        .lock()
        .iter()
        .find(|artifact| artifact.id == request.artifact_id)
        .cloned();
    let destination = validated_destination(&request.destination_path, request.format)
        .map_err(|error| error.to_string())?;
    if source
        .as_ref()
        .and_then(|artifact| artifact.path.as_deref())
        .is_some_and(|path| Path::new(path) == destination)
    {
        return Err(
            "Choose a new file name. Captures preserves the original screenshot when editing."
                .to_owned(),
        );
    }

    let format = request.format;
    let jpeg_quality = request.jpeg_quality;
    let max_size_bytes = request.max_size_bytes;
    let capture_mode = source
        .as_ref()
        .map(|artifact| artifact.mode)
        .unwrap_or(CaptureMode::Region);
    let task_destination = destination.clone();
    let task_png = request.image_png;
    let (image_png, preview_png, encoded_size, width, height) =
        tauri::async_runtime::spawn_blocking(move || {
            let image = decode_editor_png(&task_png)?;
            let width = image.width();
            let height = image.height();
            let image_png = storage::encode_png(&image)?;
            let preview_png = storage::encode_thumbnail_png(&image)?;
            let output = encode_export_with_limit(&image, format, jpeg_quality, max_size_bytes)?;
            let encoded_size = u64::try_from(output.len()).unwrap_or(u64::MAX);
            write_export_atomically(&task_destination, &output)?;
            Ok::<_, AppError>((image_png, preview_png, encoded_size, width, height))
        })
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;

    let artifact_id = Uuid::new_v4().to_string();
    let created_at = Utc::now().to_rfc3339();
    let saved_path = destination.to_string_lossy().into_owned();
    let history_entry = HistoryEntry {
        id: artifact_id.clone(),
        kind: ArtifactKind::Screenshot,
        preview_url: history_preview_url(&artifact_id),
        full_url: history_full_url(&artifact_id),
        width,
        height,
        size_bytes: encoded_size,
        created_at: created_at.clone(),
        mode: Some(capture_mode),
        saved_path: Some(saved_path.clone()),
        mime_type: Some(
            match format {
                ScreenshotEditFormat::Png => "image/png",
                ScreenshotEditFormat::Jpeg => "image/jpeg",
                ScreenshotEditFormat::Webp => "image/webp",
            }
            .to_owned(),
        ),
        duration_ms: None,
        target: None,
        has_system_audio: false,
        has_microphone_audio: false,
        dropped_frames: 0,
    };
    let history_saved =
        match storage::save_history_capture(&history_entry, &image_png, &preview_png) {
            Ok(()) => true,
            Err(error) => {
                eprintln!("failed to save edited screenshot history: {error}");
                false
            }
        };
    let artifact = CaptureArtifact {
        id: artifact_id.clone(),
        path: Some(saved_path.clone()),
        preview_url: artifact_url(&artifact_id),
        full_url: artifact_full_url(&artifact_id),
        width,
        height,
        size_bytes: encoded_size,
        created_at,
        mode: capture_mode,
        history_saved,
        clipboard_copy_status: ClipboardCopyStatus::Skipped,
        image_png,
        preview_png,
    };
    if history_saved {
        state.history.lock().insert(0, history_entry);
    }
    state.artifacts.lock().push(artifact.clone());
    state
        .thumbnail_visibility
        .lock()
        .wait_for_artifact(artifact.id.clone());
    app.emit("capture-completed", &artifact)
        .map_err(|error| error.to_string())?;
    if history_saved {
        app.emit("capture-history-changed", ())
            .map_err(|error| error.to_string())?;
    }
    super::restore_thumbnail_stack(&app, state.inner());

    Ok(SavedScreenshotEdit {
        artifact,
        path: saved_path,
        format,
    })
}

fn decode_editor_png(bytes: &[u8]) -> Result<RgbaImage, AppError> {
    if bytes.is_empty() || bytes.len() > MAX_EDITOR_PNG_BYTES {
        return Err(AppError::Image(
            "the edited screenshot payload is empty or too large".to_owned(),
        ));
    }
    let image = image::load_from_memory_with_format(bytes, ImageFormat::Png)
        .map_err(|error| AppError::Image(error.to_string()))?
        .into_rgba8();
    let pixels = u64::from(image.width()) * u64::from(image.height());
    if image.width() > MAX_EDITOR_DIMENSION
        || image.height() > MAX_EDITOR_DIMENSION
        || pixels > MAX_EDITOR_PIXELS
    {
        return Err(AppError::Image(format!(
            "edited screenshots are limited to {MAX_EDITOR_DIMENSION} pixels per side and {MAX_EDITOR_PIXELS} total pixels"
        )));
    }
    Ok(image)
}

fn encode_export(
    image: &RgbaImage,
    format: ScreenshotEditFormat,
    jpeg_quality: u8,
) -> Result<Vec<u8>, AppError> {
    if matches!(format, ScreenshotEditFormat::Png) {
        return storage::encode_png(image);
    }
    let mut bytes = Vec::new();
    match format {
        ScreenshotEditFormat::Png => unreachable!(),
        ScreenshotEditFormat::Jpeg => {
            let rgb = composite_onto_white(image);
            return encode_jpeg(&rgb, jpeg_quality);
        }
        ScreenshotEditFormat::Webp => {
            WebPEncoder::new_lossless(&mut bytes)
                .write_image(
                    image.as_raw(),
                    image.width(),
                    image.height(),
                    ExtendedColorType::Rgba8,
                )
                .map_err(|error| AppError::Image(error.to_string()))?;
        }
    }
    Ok(bytes)
}

fn encode_export_with_limit(
    image: &RgbaImage,
    format: ScreenshotEditFormat,
    jpeg_quality: u8,
    max_size_bytes: Option<u64>,
) -> Result<Vec<u8>, AppError> {
    let output = encode_export(image, format, jpeg_quality)?;
    let Some(maximum) = max_size_bytes else {
        return Ok(output);
    };
    if u64::try_from(output.len()).unwrap_or(u64::MAX) <= maximum {
        return Ok(output);
    }
    if !matches!(format, ScreenshotEditFormat::Jpeg) {
        return Err(AppError::Image(format!(
            "the lossless {} is larger than the requested maximum; choose JPEG or reduce the output size",
            format.extension().to_uppercase()
        )));
    }

    let rgb = composite_onto_white(image);
    let maximum_quality = jpeg_quality.clamp(40, 100);
    let minimum = encode_jpeg(&rgb, 40)?;
    if u64::try_from(minimum.len()).unwrap_or(u64::MAX) > maximum {
        return Err(AppError::Image(
            "JPEG cannot meet the requested maximum at the supported quality range; reduce the output size or raise the limit"
                .to_owned(),
        ));
    }

    let mut best = minimum;
    let mut low = 41_u8;
    let mut high = maximum_quality;
    while low <= high {
        let quality = low + (high - low) / 2;
        let candidate = encode_jpeg(&rgb, quality)?;
        if u64::try_from(candidate.len()).unwrap_or(u64::MAX) <= maximum {
            best = candidate;
            low = quality.saturating_add(1);
        } else {
            if quality == 0 {
                break;
            }
            high = quality - 1;
        }
    }
    Ok(best)
}

fn encode_jpeg(image: &RgbImage, quality: u8) -> Result<Vec<u8>, AppError> {
    let mut bytes = Vec::new();
    JpegEncoder::new_with_quality(&mut bytes, quality.clamp(40, 100))
        .write_image(
            image.as_raw(),
            image.width(),
            image.height(),
            ExtendedColorType::Rgb8,
        )
        .map_err(|error| AppError::Image(error.to_string()))?;
    Ok(bytes)
}

fn composite_onto_white(image: &RgbaImage) -> RgbImage {
    let mut output = RgbImage::new(image.width(), image.height());
    for (x, y, pixel) in image.enumerate_pixels() {
        let alpha = u16::from(pixel[3]);
        let inverse = 255 - alpha;
        output.put_pixel(
            x,
            y,
            Rgb([
                ((u16::from(pixel[0]) * alpha + 255 * inverse) / 255) as u8,
                ((u16::from(pixel[1]) * alpha + 255 * inverse) / 255) as u8,
                ((u16::from(pixel[2]) * alpha + 255 * inverse) / 255) as u8,
            ]),
        );
    }
    output
}

fn validated_destination(
    destination: &str,
    format: ScreenshotEditFormat,
) -> Result<PathBuf, AppError> {
    let path = PathBuf::from(destination.trim());
    if destination.trim().is_empty() || path.file_name().is_none() {
        return Err(AppError::Task(
            "choose a file name for the edited screenshot".to_owned(),
        ));
    }
    let Some(extension) = path.extension().and_then(|value| value.to_str()) else {
        return Err(AppError::Task(format!(
            "the file name must end in .{}",
            format.extension()
        )));
    };
    if !format.extension_matches(extension) {
        return Err(AppError::Task(format!(
            "the file extension does not match the selected {} format",
            format.extension().to_uppercase()
        )));
    }
    if path
        .parent()
        .is_none_or(|parent| parent.as_os_str().is_empty())
    {
        return Err(AppError::Task(
            "choose a destination folder for the edited screenshot".to_owned(),
        ));
    }
    Ok(path)
}

fn unique_edit_path(directory: &Path, stem: &str, extension: &str) -> PathBuf {
    let base = format!("{stem}-edited");
    let initial = directory.join(format!("{base}.{extension}"));
    if !initial.exists() {
        return initial;
    }
    (1_u32..)
        .map(|suffix| directory.join(format!("{base}-{suffix}.{extension}")))
        .find(|candidate| !candidate.exists())
        .unwrap_or_else(|| directory.join(format!("{base}-{}.{}", Uuid::new_v4(), extension)))
}

fn write_export_atomically(path: &Path, bytes: &[u8]) -> Result<(), AppError> {
    let parent = path.parent().ok_or_else(|| {
        AppError::Task("the edited screenshot path has no destination folder".to_owned())
    })?;
    fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(path)
        .map_err(|error| AppError::Io(error.error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use image::{ImageFormat, Rgba, RgbaImage};
    use tempfile::tempdir;

    use super::{
        ScreenshotEditFormat, composite_onto_white, encode_export, encode_export_with_limit,
        unique_edit_path, validated_destination, write_export_atomically,
    };

    fn sample() -> RgbaImage {
        RgbaImage::from_fn(3, 2, |x, y| {
            if x == 0 && y == 0 {
                Rgba([255, 0, 0, 128])
            } else {
                Rgba([20, 80, 160, 255])
            }
        })
    }

    fn detailed_sample() -> RgbaImage {
        RgbaImage::from_fn(192, 128, |x, y| {
            let mixed = x.wrapping_mul(73) ^ y.wrapping_mul(151) ^ (x * y);
            Rgba([
                mixed as u8,
                mixed.rotate_left(5) as u8,
                mixed.rotate_left(11) as u8,
                255,
            ])
        })
    }

    #[test]
    fn exports_png_jpeg_and_lossless_webp() {
        for (format, expected) in [
            (ScreenshotEditFormat::Png, ImageFormat::Png),
            (ScreenshotEditFormat::Jpeg, ImageFormat::Jpeg),
            (ScreenshotEditFormat::Webp, ImageFormat::WebP),
        ] {
            let bytes = encode_export(&sample(), format, 92).expect("image encoded");
            let decoded = image::load_from_memory_with_format(&bytes, expected)
                .expect("encoded image is readable");
            assert_eq!((decoded.width(), decoded.height()), (3, 2));
        }
    }

    #[test]
    fn jpeg_alpha_is_composited_onto_white() {
        let output = composite_onto_white(&sample());
        assert_eq!(output.get_pixel(0, 0).0, [255, 127, 127]);
        assert_eq!(output.get_pixel(1, 0).0, [20, 80, 160]);
    }

    #[test]
    fn jpeg_quality_falls_until_the_requested_file_limit_is_met() {
        let image = detailed_sample();
        let high = encode_export(&image, ScreenshotEditFormat::Jpeg, 100).unwrap();
        let low = encode_export(&image, ScreenshotEditFormat::Jpeg, 40).unwrap();
        assert!(high.len() > low.len());
        let maximum = u64::try_from((high.len() + low.len()) / 2).unwrap();

        let limited =
            encode_export_with_limit(&image, ScreenshotEditFormat::Jpeg, 100, Some(maximum))
                .expect("JPEG fits the requested maximum");

        assert!(u64::try_from(limited.len()).unwrap() <= maximum);
        image::load_from_memory_with_format(&limited, ImageFormat::Jpeg)
            .expect("limited JPEG remains readable");
    }

    #[test]
    fn lossless_exports_explain_when_the_requested_limit_cannot_be_met() {
        let error =
            encode_export_with_limit(&detailed_sample(), ScreenshotEditFormat::Png, 100, Some(10))
                .expect_err("lossless output should not silently degrade");

        assert!(error.to_string().contains("choose JPEG"));
    }

    #[test]
    fn validates_extensions_for_the_selected_format() {
        assert!(validated_destination("/tmp/edit.jpg", ScreenshotEditFormat::Jpeg).is_ok());
        assert!(validated_destination("/tmp/edit.jpeg", ScreenshotEditFormat::Jpeg).is_ok());
        assert!(validated_destination("/tmp/edit.png", ScreenshotEditFormat::Jpeg).is_err());
        assert!(validated_destination("edit.png", ScreenshotEditFormat::Png).is_err());
    }

    #[test]
    fn suggests_a_non_destructive_unique_edit_name() {
        let directory = tempdir().expect("temporary directory");
        let first = unique_edit_path(directory.path(), "capture", "png");
        assert_eq!(first, directory.path().join("capture-edited.png"));
        std::fs::write(&first, b"existing").expect("existing edit");
        assert_eq!(
            unique_edit_path(directory.path(), "capture", "png"),
            directory.path().join("capture-edited-1.png")
        );
    }

    #[test]
    fn export_write_replaces_an_existing_confirmed_destination() {
        let directory = tempdir().expect("temporary directory");
        let destination = directory.path().join("edit.png");
        std::fs::write(&destination, b"old").expect("old destination");
        write_export_atomically(&destination, b"new").expect("atomic export");
        assert_eq!(std::fs::read(destination).unwrap(), b"new");
    }
}
