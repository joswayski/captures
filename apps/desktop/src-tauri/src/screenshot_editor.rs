use std::{
    borrow::Cow,
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
};

use chrono::{Local, Utc};
use image::{ImageFormat, RgbImage, RgbaImage};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
use uuid::Uuid;

use captures_capture::CaptureMode;
use captures_history::editor_draft;
pub use captures_history::editor_draft::{
    LoadedDraft as LoadedScreenshotEditorDraft, SaveRequest as SaveScreenshotEditorDraftRequest,
};
use captures_image::composite_onto_white;

use crate::{
    AppError, CommandResult,
    models::{
        ArtifactKind, CaptureArtifact, ClipboardCopyStatus, ClipboardState, HistoryEntry,
        ScreenshotFormat, artifact_full_url, artifact_url, editor_draft_asset_url,
        history_full_url, history_preview_url, screenshot_editor_drafts_directory,
    },
    state::AppState,
    storage,
};

pub(crate) const SCREENSHOT_EDITOR_WINDOW_PREFIX: &str = "screenshot-editor-";
pub(crate) const MAX_EDITOR_PNG_BYTES: usize = editor_draft::MAX_IMAGE_BYTES;
pub(crate) const MAX_EDITOR_DIMENSION: u32 = 16_384;
pub(crate) const MAX_EDITOR_PIXELS: u64 = 100_000_000;

use crate::models::ScreenshotFormat as ScreenshotEditFormat;

/// How aggressively to encode while keeping the user-selected format.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ScreenshotExportQualityMode {
    #[default]
    Preserve,
    Compress,
    Maximum,
}

impl ScreenshotExportQualityMode {
    const fn uses_compact_encode(self) -> bool {
        !matches!(self, Self::Preserve)
    }
}

#[derive(Debug, Deserialize)]
pub struct ScreenshotEditSaveRequest {
    artifact_id: String,
    destination_path: String,
    format: ScreenshotEditFormat,
    #[serde(default)]
    quality_mode: ScreenshotExportQualityMode,
    jpeg_quality: u8,
    #[serde(default)]
    png_max_colors: Option<u16>,
    #[serde(default)]
    max_size_bytes: Option<u64>,
    #[serde(default)]
    overwrite_source: bool,
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
    if state.find_artifact(&artifact_id).is_none() {
        return Err("the screenshot is no longer available".to_owned());
    }
    show_screenshot_editor(&app, &artifact_id).map_err(|error| error.to_string())
}

pub(crate) fn screenshot_editor_is_open(app: &AppHandle, artifact_id: &str) -> bool {
    app.get_webview_window(&screenshot_editor_window_label(artifact_id))
        .is_some()
}

fn screenshot_editor_window_label(artifact_id: &str) -> String {
    format!("{SCREENSHOT_EDITOR_WINDOW_PREFIX}{artifact_id}")
}

pub(crate) fn show_screenshot_editor(app: &AppHandle, artifact_id: &str) -> Result<(), AppError> {
    let label = screenshot_editor_window_label(artifact_id);
    // Opening the editor is an intentional focus change; do not hand activation
    // back to whatever app was frontmost before a prior capture shortcut.
    #[cfg(target_os = "macos")]
    captures_macos_window::clear_frontmost_app_anchor();
    if let Some(window) = app.get_webview_window(&label) {
        crate::reveal_and_focus_document_window(&window)?;
        return Ok(());
    }

    let (theme, background) = crate::document_window_chrome(app);
    WebviewWindowBuilder::new(
        app,
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
    .theme(theme)
    .background_color(background)
    .focused(false)
    .visible(false)
    .on_page_load(crate::document_window_page_load_handler(
        "failed to reveal screenshot editor",
    ))
    .build()?;
    Ok(())
}

#[tauri::command]
pub fn default_screenshot_edit_path(
    state: tauri::State<'_, Arc<AppState>>,
    artifact_id: String,
    format: ScreenshotEditFormat,
) -> CommandResult<String> {
    // First permanent save for a path-less capture (fresh screenshot / history
    // restore). Suggest a normal Captures name — not an `-edited` copy suffix.
    // The frontend only appends `-edited` when Save as new file is turned on for an
    // already-saved original.
    let artifact = state.find_artifact(&artifact_id);
    let history_saved_path = state
        .history
        .lock()
        .iter()
        .find(|entry| entry.id == artifact_id)
        .and_then(|entry| entry.saved_path.clone());
    let source_owned = artifact
        .as_ref()
        .and_then(|artifact| artifact.path.clone())
        .or(history_saved_path);
    let source = source_owned.as_deref().map(Path::new);
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
    // When a permanent path already exists, keep that stem so Save can overwrite
    // the original. Only mint a collision-safe name for brand-new first saves.
    let path = if source.is_some_and(Path::is_file) {
        directory.join(format!("{stem}.{}", format.extension()))
    } else {
        unique_export_path(&directory, &stem, format.extension())
    };
    Ok(path.to_string_lossy().into_owned())
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

/// Encode the flattened editor canvas the same way save does, and return only the
/// resulting byte length — used for the live Est. size readout (especially PNG
/// color quantization, which the browser cannot estimate accurately).
#[tauri::command]
pub async fn estimate_screenshot_export(
    image_png: Vec<u8>,
    format: ScreenshotEditFormat,
    quality_mode: ScreenshotExportQualityMode,
    jpeg_quality: u8,
    max_size_bytes: Option<u64>,
    png_max_colors: Option<u16>,
) -> CommandResult<u64> {
    tauri::async_runtime::spawn_blocking(move || {
        let image = decode_editor_png(&image_png)?;
        let output = encode_export_with_limit(
            &image,
            format,
            quality_mode,
            jpeg_quality,
            max_size_bytes,
            png_max_colors,
        )?;
        Ok::<_, AppError>(u64::try_from(output.len()).unwrap_or(u64::MAX))
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| error.to_string())
}

/// Encode the flattened editor canvas the same way save does and return the
/// compressed file bytes for an on-screen before/after preview.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenshotExportPreview {
    bytes: Vec<u8>,
    size_bytes: u64,
    format: ScreenshotEditFormat,
}

#[tauri::command]
pub async fn preview_screenshot_export(
    image_png: Vec<u8>,
    format: ScreenshotEditFormat,
    quality_mode: ScreenshotExportQualityMode,
    jpeg_quality: u8,
    max_size_bytes: Option<u64>,
    png_max_colors: Option<u16>,
) -> CommandResult<ScreenshotExportPreview> {
    tauri::async_runtime::spawn_blocking(move || {
        let image = decode_editor_png(&image_png)?;
        let bytes = encode_export_with_limit(
            &image,
            format,
            quality_mode,
            jpeg_quality,
            max_size_bytes,
            png_max_colors,
        )?;
        let size_bytes = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        Ok::<_, AppError>(ScreenshotExportPreview {
            bytes,
            size_bytes,
            format,
        })
    })
    .await
    .map_err(|error| error.to_string())?
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
    let source = state.find_artifact(&request.artifact_id);
    let destination = validated_destination(&request.destination_path, request.format)
        .map_err(|error| error.to_string())?;
    let source_path = source
        .as_ref()
        .and_then(|artifact| artifact.path.as_deref())
        .map(Path::new);
    if request.overwrite_source && source_path != Some(destination.as_path()) {
        return Err(
            "the original screenshot is unavailable or does not match the save destination"
                .to_owned(),
        );
    }
    if !request.overwrite_source && source_path == Some(destination.as_path()) {
        return Err(
            "Choose a new file name, or turn off Save as new file to replace the original screenshot."
                .to_owned(),
        );
    }

    let overwrite_source = request.overwrite_source;
    let format = request.format;
    let quality_mode = request.quality_mode;
    let jpeg_quality = request.jpeg_quality;
    let png_max_colors = request.png_max_colors;
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
            let output = encode_save_export(
                &image,
                &image_png,
                format,
                quality_mode,
                jpeg_quality,
                max_size_bytes,
                png_max_colors,
            )?;
            let encoded_size = u64::try_from(output.len()).unwrap_or(u64::MAX);
            write_export_atomically(&task_destination, &output)?;
            Ok::<_, AppError>((image_png, preview_png, encoded_size, width, height))
        })
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;

    let artifact_id = if overwrite_source {
        source
            .as_ref()
            .expect("overwrite source was validated")
            .id
            .clone()
    } else {
        Uuid::new_v4().to_string()
    };
    let created_at = if overwrite_source {
        source
            .as_ref()
            .expect("overwrite source was validated")
            .created_at
            .clone()
    } else {
        Utc::now().to_rfc3339()
    };
    let url_revision = overwrite_source.then(|| Uuid::new_v4().to_string());
    let versioned_url = |url: String| {
        url_revision
            .as_ref()
            .map_or(url.clone(), |revision| format!("{url}?revision={revision}"))
    };
    let saved_path = destination.to_string_lossy().into_owned();
    let history_entry = HistoryEntry {
        id: artifact_id.clone(),
        kind: ArtifactKind::Screenshot,
        preview_url: versioned_url(history_preview_url(&artifact_id)),
        full_url: versioned_url(history_full_url(&artifact_id)),
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
        preview_url: versioned_url(artifact_url(&artifact_id)),
        full_url: versioned_url(artifact_full_url(&artifact_id)),
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
        let mut history = state.history.lock();
        if overwrite_source
            && let Some(existing) = history.iter_mut().find(|entry| entry.id == artifact_id)
        {
            *existing = history_entry;
        } else {
            history.insert(0, history_entry);
        }
    }
    if overwrite_source {
        if !state.replace_artifact(artifact.clone()) {
            return Err("the original screenshot is no longer available".to_owned());
        }
        app.emit("artifact-updated", &artifact)
            .map_err(|error| error.to_string())?;
        if state
            .clipboard_ownership
            .lock()
            .clear_if_artifact(&artifact_id)
        {
            app.emit(
                "clipboard-owner-changed",
                ClipboardState {
                    revision: super::current_clipboard_revision(),
                    artifact_id: None,
                },
            )
            .map_err(|error| error.to_string())?;
        }
    } else {
        // A folder save is not a new capture. Keep this editor's latest export
        // available for Reveal in Folder and later overwrites, but do not open
        // a mini preview. Repeated Save as new file replaces the previous slot.
        state.store_editor_artifact(&request.artifact_id, artifact.clone());
    }
    if history_saved {
        app.emit("capture-history-changed", ())
            .map_err(|error| error.to_string())?;
    }

    Ok(SavedScreenshotEdit {
        artifact,
        path: saved_path,
        format,
    })
}

/// Persist a layered screenshot editor session so reopening restores unsaved work.
#[tauri::command]
pub fn save_screenshot_editor_draft(
    request: SaveScreenshotEditorDraftRequest,
) -> CommandResult<()> {
    editor_draft::save(&screenshot_editor_drafts_directory(), request)
        .map_err(|error| error.to_string())
}

/// Load a previously autosaved editor draft, if one exists for this capture.
#[tauri::command]
pub fn load_screenshot_editor_draft(
    artifact_id: String,
) -> CommandResult<Option<LoadedScreenshotEditorDraft>> {
    editor_draft::load(
        &screenshot_editor_drafts_directory(),
        &artifact_id,
        editor_draft_asset_url,
    )
    .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn discard_screenshot_editor_draft(artifact_id: String) -> CommandResult<()> {
    discard_screenshot_editor_draft_files(&artifact_id).map_err(|error| error.to_string())
}

/// Best-effort cleanup used when history entries are deleted.
pub fn discard_screenshot_editor_draft_files(artifact_id: &str) -> Result<(), AppError> {
    editor_draft::discard(&screenshot_editor_drafts_directory(), artifact_id).map_err(Into::into)
}

pub(crate) fn resolve_editor_draft_asset(path: &str) -> Option<Vec<u8>> {
    let mut segments = path.split('/');
    if segments.next() != Some("editor-draft") {
        return None;
    }
    let artifact_id = segments.next()?;
    let asset_id = segments.next()?;
    if segments.next().is_some() {
        return None;
    }
    editor_draft::read_asset(&screenshot_editor_drafts_directory(), artifact_id, asset_id).ok()
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
    ensure_editor_image_limits(image.width(), image.height())?;
    Ok(image)
}

pub(crate) fn decode_still_image_file(path: &Path) -> Result<RgbaImage, AppError> {
    let metadata = fs::metadata(path)?;
    if metadata.len() > u64::try_from(MAX_EDITOR_PNG_BYTES).unwrap_or(u64::MAX) {
        return Err(AppError::Image(
            "this image is too large to open in Captures".to_owned(),
        ));
    }
    let image = image::open(path)
        .map_err(|error| AppError::Image(error.to_string()))?
        .into_rgba8();
    ensure_editor_image_limits(image.width(), image.height())?;
    Ok(image)
}

pub(crate) fn ensure_editor_image_limits(width: u32, height: u32) -> Result<(), AppError> {
    let pixels = u64::from(width) * u64::from(height);
    if width > MAX_EDITOR_DIMENSION || height > MAX_EDITOR_DIMENSION || pixels > MAX_EDITOR_PIXELS {
        return Err(AppError::Image(format!(
            "images are limited to {MAX_EDITOR_DIMENSION} pixels per side and {MAX_EDITOR_PIXELS} total pixels"
        )));
    }
    Ok(())
}

/// Encode a captured PNG using the user's default screenshot save format.
pub(crate) fn encode_saved_screenshot(
    png: &[u8],
    format: ScreenshotFormat,
) -> Result<Vec<u8>, AppError> {
    if matches!(format, ScreenshotFormat::Png) {
        return Ok(png.to_vec());
    }
    let image = decode_editor_png(png)?;
    encode_export(
        &image,
        format,
        ScreenshotExportQualityMode::Preserve,
        92,
        None,
    )
}

fn encode_export(
    image: &RgbaImage,
    format: ScreenshotEditFormat,
    quality_mode: ScreenshotExportQualityMode,
    jpeg_quality: u8,
    png_max_colors: Option<u16>,
) -> Result<Vec<u8>, AppError> {
    match format {
        ScreenshotEditFormat::Png => {
            let max_colors = match quality_mode {
                ScreenshotExportQualityMode::Preserve => None,
                ScreenshotExportQualityMode::Compress => png_max_colors
                    .filter(|count| *count > 0)
                    .or_else(|| storage::png_palette_colors_for_quality(jpeg_quality)),
                // Maximum without a byte budget still quantizes aggressively.
                ScreenshotExportQualityMode::Maximum => {
                    Some(png_max_colors.filter(|count| *count > 0).unwrap_or(64))
                }
            };
            storage::encode_png_export(image, quality_mode.uses_compact_encode(), max_colors)
        }
        ScreenshotEditFormat::Jpeg => {
            let quality = if matches!(quality_mode, ScreenshotExportQualityMode::Preserve) {
                100
            } else {
                jpeg_quality
            };
            let rgb = composite_onto_white(image);
            encode_jpeg(&rgb, quality)
        }
        ScreenshotEditFormat::Webp => {
            // Preserve = lossless WebP. Compress/maximum = lossy quality (libwebp).
            let quality = match quality_mode {
                ScreenshotExportQualityMode::Preserve => None,
                ScreenshotExportQualityMode::Compress => Some(jpeg_quality.clamp(1, 100)),
                ScreenshotExportQualityMode::Maximum => Some(jpeg_quality.clamp(1, 100).min(80)),
            };
            encode_webp(image, quality)
        }
    }
}

fn encoded_len(bytes: &[u8]) -> u64 {
    u64::try_from(bytes.len()).unwrap_or(u64::MAX)
}

/// Select export bytes when saving also needs a lossless history PNG.
fn encode_save_export<'a>(
    image: &RgbaImage,
    history_png: &'a [u8],
    format: ScreenshotEditFormat,
    quality_mode: ScreenshotExportQualityMode,
    jpeg_quality: u8,
    max_size_bytes: Option<u64>,
    png_max_colors: Option<u16>,
) -> Result<Cow<'a, [u8]>, AppError> {
    if matches!(format, ScreenshotEditFormat::Png)
        && matches!(quality_mode, ScreenshotExportQualityMode::Preserve)
        && max_size_bytes.is_none()
    {
        // The history PNG uses the same encoder as Preserve export. Borrow
        // those canonical bytes, avoiding both a second encode and its buffer.
        return Ok(Cow::Borrowed(history_png));
    }
    encode_export_with_limit(
        image,
        format,
        quality_mode,
        jpeg_quality,
        max_size_bytes,
        png_max_colors,
    )
    .map(Cow::Owned)
}

fn encode_export_with_limit(
    image: &RgbaImage,
    format: ScreenshotEditFormat,
    quality_mode: ScreenshotExportQualityMode,
    jpeg_quality: u8,
    max_size_bytes: Option<u64>,
    png_max_colors: Option<u16>,
) -> Result<Vec<u8>, AppError> {
    let Some(maximum) = max_size_bytes else {
        return encode_export(image, format, quality_mode, jpeg_quality, png_max_colors);
    };

    // Highest quality that still fits: start from an uncompressed/preserve
    // encode. A large cap (500 MB on a 4 MB original) must not keep a previous
    // 64-color PNG or q80 WebP just because that already fit.
    let preserve = encode_export(
        image,
        format,
        ScreenshotExportQualityMode::Preserve,
        100,
        None,
    )?;
    if encoded_len(&preserve) <= maximum {
        return Ok(preserve);
    }

    let quality_ceiling = match quality_mode {
        ScreenshotExportQualityMode::Compress => jpeg_quality,
        ScreenshotExportQualityMode::Preserve | ScreenshotExportQualityMode::Maximum => 100,
    };

    match format {
        ScreenshotEditFormat::Jpeg => {
            let rgb = composite_onto_white(image);
            let maximum_quality = quality_ceiling.clamp(40, 100);
            let minimum = encode_jpeg(&rgb, 40)?;
            if encoded_len(&minimum) > maximum {
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
                if encoded_len(&candidate) <= maximum {
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
        ScreenshotEditFormat::Png => {
            // Walk down the color budget until the file fits (same idea as quality notches).
            let mut best: Option<Vec<u8>> = None;
            'palettes: for colors in storage::PNG_MAXIMUM_COLOR_STEPS {
                // Dither first (matches Compress). If Floyd–Steinberg noise makes
                // every indexed file larger than lossless, try the undithered
                // palette so a size cap can still be met with posterization.
                for dither in [true, false] {
                    let candidate =
                        storage::encode_png_export_dithered(image, true, Some(colors), dither)?;
                    let fits = encoded_len(&candidate) <= maximum;
                    if fits {
                        best = Some(candidate);
                        break 'palettes;
                    }
                    best = Some(candidate);
                }
            }
            let best = best.ok_or_else(|| {
                AppError::Image(
                    "PNG cannot meet the requested maximum; reduce the output size or raise the limit"
                        .to_owned(),
                )
            })?;
            if encoded_len(&best) <= maximum {
                Ok(best)
            } else {
                Err(AppError::Image(
                    "the PNG is larger than the requested maximum even after reducing colors; reduce the output size, raise the limit, or switch to JPEG for more aggressive size control"
                        .to_owned(),
                ))
            }
        }
        ScreenshotEditFormat::Webp => {
            let minimum = encode_webp(image, Some(1))?;
            if encoded_len(&minimum) > maximum {
                return Err(AppError::Image(
                    "WebP cannot meet the requested maximum at the supported quality range; reduce the output size or raise the limit"
                        .to_owned(),
                ));
            }
            let mut best = minimum;
            let mut low = 2_u8;
            let mut high = quality_ceiling.clamp(1, 100);
            while low <= high {
                let quality = low + (high - low) / 2;
                let candidate = encode_webp(image, Some(quality))?;
                if encoded_len(&candidate) <= maximum {
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
    }
}

fn encode_jpeg(image: &RgbImage, quality: u8) -> Result<Vec<u8>, AppError> {
    captures_image::encode_jpeg(image, quality).map_err(AppError::Image)
}

/// Encode WebP. `None` quality is lossless; `Some(q)` is lossy at quality 1–100.
/// Keeps alpha (unlike JPEG). Uses libwebp because the `image` crate only encodes lossless WebP.
fn encode_webp(image: &RgbaImage, quality: Option<u8>) -> Result<Vec<u8>, AppError> {
    captures_image::encode_webp(image, quality).map_err(AppError::Image)
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

/// First-available `{stem}.{extension}` in `directory`, with numeric suffixes
/// only when that name is already taken. Does not invent an `-edited` stem —
/// copy naming is a frontend “Save as new file” concern.
fn unique_export_path(directory: &Path, stem: &str, extension: &str) -> PathBuf {
    let initial = directory.join(format!("{stem}.{extension}"));
    if !initial.exists() {
        return initial;
    }
    (1_u32..)
        .map(|suffix| directory.join(format!("{stem}-{suffix}.{extension}")))
        .find(|candidate| !candidate.exists())
        .unwrap_or_else(|| directory.join(format!("{stem}-{}.{}", Uuid::new_v4(), extension)))
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
        ScreenshotEditFormat, ScreenshotExportQualityMode, composite_onto_white,
        decode_still_image_file, encode_export, encode_export_with_limit, encode_save_export,
        encoded_len, ensure_editor_image_limits, resolve_editor_draft_asset, unique_export_path,
        validated_destination, write_export_atomically,
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
    fn save_export_matches_uncached_formats_quality_and_size_limits() {
        let image = RgbaImage::from_fn(17, 11, |x, y| {
            Rgba([(x * 13) as u8, (y * 23) as u8, 173, (x * y) as u8])
        });
        let history = crate::storage::encode_png(&image).unwrap();
        let size = encoded_len(&history);
        for format in [
            ScreenshotEditFormat::Png,
            ScreenshotEditFormat::Jpeg,
            ScreenshotEditFormat::Webp,
        ] {
            for quality in [
                ScreenshotExportQualityMode::Preserve,
                ScreenshotExportQualityMode::Compress,
                ScreenshotExportQualityMode::Maximum,
            ] {
                for limit in [None, Some(size), Some(size - 1), Some(0)] {
                    let expected =
                        encode_export_with_limit(&image, format, quality, 70, limit, Some(8));
                    let actual =
                        encode_save_export(&image, &history, format, quality, 70, limit, Some(8));
                    match (actual, expected) {
                        (Ok(actual), Ok(expected)) => assert_eq!(
                            actual.as_ref(),
                            expected,
                            "{format:?} {quality:?} {limit:?}"
                        ),
                        (Err(actual), Err(expected)) => {
                            assert_eq!(actual.to_string(), expected.to_string())
                        }
                        (actual, expected) => {
                            panic!("{format:?} {quality:?} {limit:?}: {actual:?} != {expected:?}")
                        }
                    }
                }
            }
        }
        let output = encode_save_export(
            &image,
            &history,
            ScreenshotEditFormat::Png,
            ScreenshotExportQualityMode::Preserve,
            1,
            None,
            Some(8),
        )
        .unwrap();
        assert_eq!(image::load_from_memory(&output).unwrap().to_rgba8(), image);
        assert!(matches!(output, std::borrow::Cow::Borrowed(_)));
        assert_eq!(output.as_ptr(), history.as_ptr());
    }

    #[test]
    #[ignore = "manual release benchmark: --release --ignored --nocapture"]
    fn benchmark_png_save_encoding() {
        use std::{hint::black_box, time::Instant};

        // Timing includes history + thumbnail + export encoding, not input
        // decode, IPC, filesystem writes, or UI. Input creation is untimed.
        for (name, width, height, detailed) in [
            ("1080p-ui", 1920, 1080, false),
            ("4k-ui", 3840, 2160, false),
            ("4k-detail-alpha", 3840, 2160, true),
        ] {
            let image = RgbaImage::from_fn(width, height, |x, y| {
                if detailed {
                    let mixed = x.wrapping_mul(73) ^ y.wrapping_mul(151) ^ (x * y);
                    Rgba([
                        mixed as u8,
                        (mixed >> 5) as u8,
                        (mixed >> 11) as u8,
                        x as u8,
                    ])
                } else if x % 240 < 2 || y % 80 < 2 {
                    Rgba([37, 61, 93, 255])
                } else {
                    Rgba([245, 243, 240, 255])
                }
            });
            let mut samples = Vec::new();
            for iteration in 0..8 {
                let start = Instant::now();
                let history = crate::storage::encode_png(black_box(&image)).unwrap();
                let thumbnail = crate::storage::encode_thumbnail_png(&image).unwrap();
                let output = encode_save_export(
                    &image,
                    &history,
                    ScreenshotEditFormat::Png,
                    ScreenshotExportQualityMode::Preserve,
                    92,
                    None,
                    None,
                )
                .unwrap();
                black_box((&history, &thumbnail, &output));
                let elapsed = start.elapsed().as_secs_f64() * 1000.0;
                assert_eq!(output.as_ref(), history);
                if iteration > 0 {
                    samples.push(elapsed);
                }
            }
            eprintln!("{name}: samples_ms={samples:?}");
            samples.sort_by(f64::total_cmp);
            eprintln!("{name}: median_ms={:.3}", samples[samples.len() / 2]);
        }
    }

    #[test]
    fn opens_png_jpeg_and_webp_files_from_disk() {
        let directory = tempdir().expect("temporary directory");
        for (format, name) in [
            (ScreenshotEditFormat::Png, "still.png"),
            (ScreenshotEditFormat::Jpeg, "still.jpg"),
            (ScreenshotEditFormat::Webp, "still.webp"),
        ] {
            let bytes = encode_export(
                &sample(),
                format,
                ScreenshotExportQualityMode::Preserve,
                100,
                None,
            )
            .expect("image encoded");
            let path = directory.path().join(name);
            std::fs::write(&path, bytes).expect("image written");
            let image = decode_still_image_file(&path).expect("image opened");
            assert_eq!((image.width(), image.height()), (3, 2), "{name}");
        }
    }

    #[test]
    fn rejects_images_over_the_editor_dimension_limit() {
        assert!(ensure_editor_image_limits(10_000, 10_000).is_ok());
        assert!(ensure_editor_image_limits(16_385, 1).is_err());
        assert!(ensure_editor_image_limits(10_001, 10_001).is_err());
    }

    #[test]
    fn exports_png_jpeg_and_webp_without_changing_format() {
        for (format, expected) in [
            (ScreenshotEditFormat::Png, ImageFormat::Png),
            (ScreenshotEditFormat::Jpeg, ImageFormat::Jpeg),
            (ScreenshotEditFormat::Webp, ImageFormat::WebP),
        ] {
            let bytes = encode_export(
                &sample(),
                format,
                ScreenshotExportQualityMode::Compress,
                92,
                None,
            )
            .expect("image encoded");
            let decoded = image::load_from_memory_with_format(&bytes, expected)
                .expect("encoded image is readable");
            assert_eq!((decoded.width(), decoded.height()), (3, 2));
        }
    }

    #[test]
    fn compact_png_stays_png_and_is_not_larger_than_fast_encode() {
        let image = detailed_sample();
        let preserve = encode_export(
            &image,
            ScreenshotEditFormat::Png,
            ScreenshotExportQualityMode::Preserve,
            100,
            None,
        )
        .unwrap();
        let compressed = encode_export(
            &image,
            ScreenshotEditFormat::Png,
            ScreenshotExportQualityMode::Compress,
            85,
            None,
        )
        .unwrap();
        assert_eq!(&compressed[..8], b"\x89PNG\r\n\x1a\n");
        assert!(compressed.len() <= preserve.len());
    }

    #[test]
    fn png_compress_quality_reduces_file_size_via_color_quantization() {
        let image = detailed_sample();
        let high = encode_export(
            &image,
            ScreenshotEditFormat::Png,
            ScreenshotExportQualityMode::Compress,
            92,
            None,
        )
        .unwrap();
        let tiny = encode_export(
            &image,
            ScreenshotEditFormat::Png,
            ScreenshotExportQualityMode::Compress,
            55,
            None,
        )
        .unwrap();
        assert_eq!(&tiny[..8], b"\x89PNG\r\n\x1a\n");
        assert!(
            tiny.len() < high.len(),
            "fewer palette colors should shrink the PNG (tiny={}, high={})",
            tiny.len(),
            high.len()
        );
        image::load_from_memory_with_format(&tiny, ImageFormat::Png)
            .expect("quantized PNG remains readable");
    }

    #[test]
    fn png_highest_compress_keeps_truecolor_instead_of_a_palette() {
        // Flat UI chrome: a few solid fills. High's 256-color index beats RGBA
        // here; photo-like noise can go the other way, which is why Highest
        // exists as a no-quantization option.
        let image = RgbaImage::from_fn(320, 200, |x, y| {
            if y < 36 {
                Rgba([28, 30, 38, 255])
            } else if x < 72 {
                Rgba([18, 20, 26, 255])
            } else if (120..200).contains(&x) && (80..140).contains(&y) {
                Rgba([47, 124, 246, 255])
            } else {
                Rgba([246, 247, 249, 255])
            }
        });
        let preserve = encode_export(
            &image,
            ScreenshotEditFormat::Png,
            ScreenshotExportQualityMode::Preserve,
            100,
            None,
        )
        .unwrap();
        let highest = encode_export(
            &image,
            ScreenshotEditFormat::Png,
            ScreenshotExportQualityMode::Compress,
            98,
            None,
        )
        .unwrap();
        assert_eq!(&highest[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(png_color_type(&highest), png::ColorType::Rgba);
        assert!(
            highest.len() <= preserve.len(),
            "highest should only pack tighter (highest={}, preserve={})",
            highest.len(),
            preserve.len()
        );
        let high = encode_export(
            &image,
            ScreenshotEditFormat::Png,
            ScreenshotExportQualityMode::Compress,
            92,
            None,
        )
        .unwrap();
        assert_eq!(png_color_type(&high), png::ColorType::Indexed);
        assert!(
            high.len() < highest.len(),
            "256-color High should beat lossless Highest packing on flat UI (high={}, highest={})",
            high.len(),
            highest.len()
        );
    }

    fn png_color_type(bytes: &[u8]) -> png::ColorType {
        png::Decoder::new(std::io::Cursor::new(bytes))
            .read_info()
            .expect("png header")
            .info()
            .color_type
    }

    fn photo_like(width: u32, height: u32, with_shadow: bool) -> RgbaImage {
        RgbaImage::from_fn(width, height, |x, y| {
            let r = (x.wrapping_mul(17).wrapping_add(y.wrapping_mul(3)) % 256) as u8;
            let g = (x.wrapping_mul(5).wrapping_add(y.wrapping_mul(11)) % 256) as u8;
            let b = (x.wrapping_mul(y).wrapping_add(40) % 256) as u8;
            let alpha = if with_shadow && (x < 36 || y + 12 > height) {
                let edge = x.min(height.saturating_sub(y).saturating_sub(1)).min(36);
                ((edge * 255) / 36) as u8
            } else {
                255
            };
            Rgba([r, g, b, alpha])
        })
    }

    #[test]
    fn png_compress_uses_indexed_color_even_when_pixels_have_alpha() {
        let image = photo_like(240, 160, true);
        assert!(image.pixels().any(|pixel| pixel[3] < 255));

        let compressed = encode_export(
            &image,
            ScreenshotEditFormat::Png,
            ScreenshotExportQualityMode::Compress,
            85,
            Some(64),
        )
        .unwrap();

        assert_eq!(png_color_type(&compressed), png::ColorType::Indexed);
        let lossless = encode_export(
            &image,
            ScreenshotEditFormat::Png,
            ScreenshotExportQualityMode::Preserve,
            100,
            None,
        )
        .unwrap();
        assert!(
            compressed.len() < lossless.len(),
            "indexed PNG should beat 32-bit RGBA packing (indexed={}, rgba={})",
            compressed.len(),
            lossless.len()
        );
        image::load_from_memory_with_format(&compressed, ImageFormat::Png)
            .expect("indexed PNG with tRNS remains readable");
    }

    #[test]
    fn png_color_slider_changes_file_size() {
        let image = photo_like(200, 140, false);
        let high = encode_export(
            &image,
            ScreenshotEditFormat::Png,
            ScreenshotExportQualityMode::Compress,
            92,
            Some(256),
        )
        .unwrap();
        let tiny = encode_export(
            &image,
            ScreenshotEditFormat::Png,
            ScreenshotExportQualityMode::Compress,
            92,
            Some(32),
        )
        .unwrap();
        assert_eq!(png_color_type(&high), png::ColorType::Indexed);
        assert_eq!(png_color_type(&tiny), png::ColorType::Indexed);
        assert!(
            tiny.len() < high.len(),
            "fewer colors should shrink the indexed PNG (32={}, 256={})",
            tiny.len(),
            high.len()
        );
    }

    #[test]
    fn webp_compress_quality_reduces_file_size_via_lossy_encode() {
        let image = detailed_sample();
        let high = encode_export(
            &image,
            ScreenshotEditFormat::Webp,
            ScreenshotExportQualityMode::Compress,
            92,
            None,
        )
        .unwrap();
        let tiny = encode_export(
            &image,
            ScreenshotEditFormat::Webp,
            ScreenshotExportQualityMode::Compress,
            55,
            None,
        )
        .unwrap();
        // RIFF....WEBP
        assert!(
            high.starts_with(b"RIFF"),
            "lossy WebP should be a RIFF container"
        );
        assert!(
            tiny.len() < high.len(),
            "lower WebP quality should shrink the file (tiny={}, high={})",
            tiny.len(),
            high.len()
        );
        image::load_from_memory_with_format(&tiny, ImageFormat::WebP)
            .expect("lossy WebP remains readable");
    }

    #[test]
    fn webp_maximum_lowers_quality_to_meet_a_size_limit() {
        let image = detailed_sample();
        let high = encode_export(
            &image,
            ScreenshotEditFormat::Webp,
            ScreenshotExportQualityMode::Compress,
            100,
            None,
        )
        .unwrap();
        let low = encode_export(
            &image,
            ScreenshotEditFormat::Webp,
            ScreenshotExportQualityMode::Compress,
            20,
            None,
        )
        .unwrap();
        assert!(high.len() > low.len());
        let maximum = u64::try_from((high.len() + low.len()) / 2).unwrap();

        let limited = encode_export_with_limit(
            &image,
            ScreenshotEditFormat::Webp,
            ScreenshotExportQualityMode::Maximum,
            100,
            Some(maximum),
            None,
        )
        .expect("WebP fits the requested maximum");

        assert!(u64::try_from(limited.len()).unwrap() <= maximum);
        image::load_from_memory_with_format(&limited, ImageFormat::WebP)
            .expect("limited WebP remains readable");
    }

    #[test]
    fn jpeg_alpha_is_composited_onto_white() {
        let output = composite_onto_white(&sample());
        assert_eq!(output.get_pixel(0, 0).0, [255, 127, 127]);
        assert_eq!(output.get_pixel(1, 0).0, [20, 80, 160]);
    }

    #[test]
    fn white_composite_preserves_every_channel_alpha_pair_and_pixel_position() {
        for (width, height) in [(256, 256), (7, 3), (0, 5), (5, 0)] {
            let input = RgbaImage::from_fn(width, height, |x, y| {
                Rgba([x as u8, (255 - x) as u8, (x * 73 + y * 11) as u8, y as u8])
            });
            let output = composite_onto_white(&input);
            assert_eq!(output.dimensions(), input.dimensions());
            for (x, y, actual) in output.enumerate_pixels() {
                let source = input.get_pixel(x, y);
                for channel in 0..3 {
                    // Subtract the alpha-weighted distance from white, rounding
                    // that distance up (equivalent to flooring the final color).
                    let distance = (255 - u32::from(source[channel])) * u32::from(source[3]);
                    let expected = 255 - distance.div_ceil(255);
                    assert_eq!(
                        u32::from(actual[channel]),
                        expected,
                        "({x}, {y}) channel {channel}"
                    );
                }
            }
        }
    }

    #[test]
    #[ignore = "manual release benchmark: --release --ignored --nocapture"]
    fn benchmark_jpeg_white_composite() {
        use std::{hint::black_box, time::Instant};

        for (name, width, height, transparent) in [
            ("1080p-opaque", 1920, 1080, false),
            ("4k-opaque", 3840, 2160, false),
            ("4k-alpha", 3840, 2160, true),
        ] {
            let input = RgbaImage::from_fn(width, height, |x, y| {
                let mixed = x.wrapping_mul(73) ^ y.wrapping_mul(151) ^ (x * y);
                Rgba([
                    mixed as u8,
                    (mixed >> 5) as u8,
                    (mixed >> 11) as u8,
                    if transparent { (x + y) as u8 } else { 255 },
                ])
            });
            for include_encode in [false, true] {
                let stage = if include_encode {
                    "jpeg-export"
                } else {
                    "composite"
                };
                let mut samples = Vec::new();
                // Fixture construction and output destruction are untimed.
                // JPEG export includes compositing + encoding, not PNG decode,
                // IPC, history/thumbnail generation, disk writes, or UI work.
                for iteration in 0..8 {
                    let start = Instant::now();
                    let output = if include_encode {
                        encode_export(
                            black_box(&input),
                            ScreenshotEditFormat::Jpeg,
                            ScreenshotExportQualityMode::Compress,
                            92,
                            None,
                        )
                        .unwrap()
                    } else {
                        composite_onto_white(black_box(&input)).into_raw()
                    };
                    black_box(&output);
                    let elapsed = start.elapsed().as_secs_f64() * 1000.0;
                    if iteration > 0 {
                        samples.push(elapsed);
                    }
                }
                eprintln!("{name} {stage}: samples_ms={samples:?}");
                samples.sort_by(f64::total_cmp);
                eprintln!(
                    "{name} {stage}: median_ms={:.3}",
                    samples[samples.len() / 2]
                );
            }
        }
    }

    fn mean_saturation_rgb(image: &image::RgbImage) -> f64 {
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

    fn saturated_orange() -> RgbaImage {
        RgbaImage::from_fn(96, 64, |x, _y| {
            let t = f64::from(x) / 95.0;
            Rgba([
                235,
                (70.0 + 40.0 * (1.0 - t)) as u8,
                (18.0 + 12.0 * t) as u8,
                255,
            ])
        })
    }

    #[test]
    fn jpeg_compress_keeps_saturated_chroma_instead_of_washing_it() {
        let image = saturated_orange();
        let original = composite_onto_white(&image);
        let original_saturation = mean_saturation_rgb(&original);
        let bytes = encode_export(
            &image,
            ScreenshotEditFormat::Jpeg,
            ScreenshotExportQualityMode::Compress,
            55,
            None,
        )
        .expect("tiny JPEG");
        let decoded = image::load_from_memory_with_format(&bytes, ImageFormat::Jpeg)
            .expect("tiny JPEG is readable")
            .to_rgb8();
        let compressed_saturation = mean_saturation_rgb(&decoded);
        let center = decoded.get_pixel(48, 32).0;
        assert!(
            center[0] > center[1].saturating_add(80),
            "orange should stay clearly redder than green after JPEG compress ({center:?})"
        );
        assert!(
            compressed_saturation > original_saturation * 0.85,
            "JPEG compress should not discard chroma first (original={original_saturation}, compressed={compressed_saturation})"
        );
    }

    #[test]
    fn lossy_webp_compress_keeps_saturated_chroma() {
        let image = saturated_orange();
        let original = composite_onto_white(&image);
        let original_saturation = mean_saturation_rgb(&original);
        let bytes = encode_export(
            &image,
            ScreenshotEditFormat::Webp,
            ScreenshotExportQualityMode::Compress,
            55,
            None,
        )
        .expect("tiny WebP");
        let decoded = image::load_from_memory_with_format(&bytes, ImageFormat::WebP)
            .expect("tiny WebP is readable")
            .to_rgb8();
        let compressed_saturation = mean_saturation_rgb(&decoded);
        assert!(
            compressed_saturation > original_saturation * 0.85,
            "lossy WebP should keep saturated hues (original={original_saturation}, compressed={compressed_saturation})"
        );
    }

    #[test]
    fn jpeg_quality_falls_until_the_requested_file_limit_is_met() {
        let image = detailed_sample();
        let high = encode_export(
            &image,
            ScreenshotEditFormat::Jpeg,
            ScreenshotExportQualityMode::Compress,
            100,
            None,
        )
        .unwrap();
        let low = encode_export(
            &image,
            ScreenshotEditFormat::Jpeg,
            ScreenshotExportQualityMode::Compress,
            40,
            None,
        )
        .unwrap();
        assert!(high.len() > low.len());
        let maximum = u64::try_from((high.len() + low.len()) / 2).unwrap();

        let limited = encode_export_with_limit(
            &image,
            ScreenshotEditFormat::Jpeg,
            ScreenshotExportQualityMode::Maximum,
            100,
            Some(maximum),
            None,
        )
        .expect("JPEG fits the requested maximum");

        assert!(u64::try_from(limited.len()).unwrap() <= maximum);
        image::load_from_memory_with_format(&limited, ImageFormat::Jpeg)
            .expect("limited JPEG remains readable");
    }

    #[test]
    fn png_maximum_can_use_undithered_palette_to_meet_a_size_cap() {
        let image = RgbaImage::from_fn(320, 120, |x, _y| {
            let t = ((x * 255) / 319) as u8;
            Rgba([t, 80, 200_u8.saturating_sub(t / 2), 255])
        });
        let lossless = encode_export(
            &image,
            ScreenshotEditFormat::Png,
            ScreenshotExportQualityMode::Preserve,
            100,
            None,
        )
        .unwrap();
        let dithered = crate::storage::encode_png_export_dithered(&image, true, Some(8), true)
            .expect("dithered 8-color");
        let undithered = crate::storage::encode_png_export_dithered(&image, true, Some(8), false)
            .expect("undithered 8-color");
        assert!(
            undithered.len() < dithered.len().min(lossless.len()),
            "undithered 8-color should undercut dither/lossless (undithered={}, dithered={}, lossless={})",
            undithered.len(),
            dithered.len(),
            lossless.len()
        );
        let maximum =
            u64::try_from((undithered.len() + dithered.len().min(lossless.len())) / 2).unwrap();
        let limited = encode_export_with_limit(
            &image,
            ScreenshotEditFormat::Png,
            ScreenshotExportQualityMode::Maximum,
            100,
            Some(maximum),
            None,
        )
        .expect("undithered palette should meet the cap");
        assert!(u64::try_from(limited.len()).unwrap() <= maximum);
        image::load_from_memory_with_format(&limited, ImageFormat::Png)
            .expect("limited PNG remains readable");
    }

    #[test]
    fn maximum_file_size_keeps_preserve_quality_when_the_original_already_fits() {
        let image = detailed_sample();
        for format in [
            ScreenshotEditFormat::Png,
            ScreenshotEditFormat::Jpeg,
            ScreenshotEditFormat::Webp,
        ] {
            let preserve = encode_export(
                &image,
                format,
                ScreenshotExportQualityMode::Preserve,
                100,
                None,
            )
            .unwrap();
            let limited = encode_export_with_limit(
                &image,
                format,
                ScreenshotExportQualityMode::Maximum,
                100,
                Some(encoded_len(&preserve).saturating_mul(8)),
                None,
            )
            .expect("a generous cap should keep original quality");
            assert_eq!(
                limited, preserve,
                "{format:?} should not compress when preserve already fits"
            );
        }
    }

    #[test]
    fn jpeg_maximum_stays_just_under_a_tight_size_cap() {
        let image = detailed_sample();
        let high = encode_export(
            &image,
            ScreenshotEditFormat::Jpeg,
            ScreenshotExportQualityMode::Preserve,
            100,
            None,
        )
        .unwrap();
        let low = encode_export(
            &image,
            ScreenshotEditFormat::Jpeg,
            ScreenshotExportQualityMode::Compress,
            40,
            None,
        )
        .unwrap();
        assert!(high.len() > low.len());
        let maximum = u64::try_from((high.len() + low.len()) / 2).unwrap();
        let limited = encode_export_with_limit(
            &image,
            ScreenshotEditFormat::Jpeg,
            ScreenshotExportQualityMode::Maximum,
            100,
            Some(maximum),
            None,
        )
        .expect("JPEG fits the requested maximum");
        assert!(encoded_len(&limited) <= maximum);
        assert!(
            encoded_len(&limited) > encoded_len(&low),
            "should keep more quality than the floor when the cap allows it (limited={}, low={}, max={maximum})",
            limited.len(),
            low.len()
        );
    }

    #[test]
    fn png_exports_explain_when_the_requested_limit_cannot_be_met() {
        let error = encode_export_with_limit(
            &detailed_sample(),
            ScreenshotEditFormat::Png,
            ScreenshotExportQualityMode::Maximum,
            100,
            Some(10),
            None,
        )
        .expect_err("PNG should not silently switch format to meet a size cap");

        let message = error.to_string();
        assert!(message.contains("PNG"));
        assert!(message.contains("JPEG") || message.contains("reduce the output size"));
        assert!(!message.contains("lossless"));
    }

    #[test]
    fn validates_extensions_for_the_selected_format() {
        assert!(validated_destination("/tmp/edit.jpg", ScreenshotEditFormat::Jpeg).is_ok());
        assert!(validated_destination("/tmp/edit.jpeg", ScreenshotEditFormat::Jpeg).is_ok());
        assert!(validated_destination("/tmp/edit.png", ScreenshotEditFormat::Jpeg).is_err());
        assert!(validated_destination("edit.png", ScreenshotEditFormat::Png).is_err());
    }

    #[test]
    fn suggests_a_unique_export_name_without_edited_suffix() {
        let directory = tempdir().expect("temporary directory");
        let first = unique_export_path(directory.path(), "capture", "png");
        assert_eq!(first, directory.path().join("capture.png"));
        std::fs::write(&first, b"existing").expect("existing export");
        assert_eq!(
            unique_export_path(directory.path(), "capture", "png"),
            directory.path().join("capture-1.png")
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

    #[test]
    fn resolves_editor_draft_asset_paths_safely() {
        assert!(resolve_editor_draft_asset("editor-draft/../secret/asset").is_none());
        assert!(resolve_editor_draft_asset("editor-draft/capture-1").is_none());
        assert!(resolve_editor_draft_asset("artifact-full/capture-1").is_none());
    }
}
