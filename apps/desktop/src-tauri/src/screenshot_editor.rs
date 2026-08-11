use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
};

use chrono::{Local, Utc};
use image::{
    ExtendedColorType, ImageEncoder, ImageFormat, Rgb, RgbImage, RgbaImage,
    codecs::jpeg::JpegEncoder,
};
use serde::{Deserialize, Serialize};
use tauri::{
    AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder, webview::PageLoadEvent,
    window::Color,
};
use uuid::Uuid;

use captures_capture::CaptureMode;

use crate::{
    AppError, CommandResult,
    models::{
        ArtifactKind, CaptureArtifact, ClipboardCopyStatus, ClipboardState, HistoryEntry,
        artifact_full_url, artifact_url, editor_draft_asset_url, history_full_url,
        history_preview_url, screenshot_editor_drafts_directory,
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
    #[serde(default)]
    quality_mode: ScreenshotExportQualityMode,
    jpeg_quality: u8,
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
    let available = state
        .artifacts
        .lock()
        .iter()
        .any(|artifact| artifact.id == artifact_id);
    if !available {
        return Err("the screenshot is no longer available".to_owned());
    }

    let label = format!("{SCREENSHOT_EDITOR_WINDOW_PREFIX}{artifact_id}");
    // Opening the editor is an intentional focus change; do not hand activation
    // back to whatever app was frontmost before a prior capture shortcut.
    #[cfg(target_os = "macos")]
    captures_macos_window::clear_frontmost_app_anchor();
    if let Some(window) = app.get_webview_window(&label) {
        crate::reveal_and_focus_document_window(&window).map_err(|error| error.to_string())?;
        return Ok(());
    }

    WebviewWindowBuilder::new(
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
    .on_page_load(|window, payload| {
        if payload.event() == PageLoadEvent::Finished
            && let Err(error) = crate::reveal_and_focus_document_window(&window)
        {
            eprintln!("failed to reveal screenshot editor: {error}");
        }
    })
    .build()
    .map_err(|error| error.to_string())?;
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
    // The frontend only appends `-edited` when Make a copy is turned on for an
    // already-saved original.
    let artifact = state
        .artifacts
        .lock()
        .iter()
        .find(|artifact| artifact.id == artifact_id)
        .cloned();
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
) -> CommandResult<u64> {
    tauri::async_runtime::spawn_blocking(move || {
        let image = decode_editor_png(&image_png)?;
        let output =
            encode_export_with_limit(&image, format, quality_mode, jpeg_quality, max_size_bytes)?;
        Ok::<_, AppError>(u64::try_from(output.len()).unwrap_or(u64::MAX))
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
    let source = state
        .artifacts
        .lock()
        .iter()
        .find(|artifact| artifact.id == request.artifact_id)
        .cloned();
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
            "Choose a new file name, or turn off Make a copy to replace the original screenshot."
                .to_owned(),
        );
    }

    let overwrite_source = request.overwrite_source;
    let format = request.format;
    let quality_mode = request.quality_mode;
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
            let output = encode_export_with_limit(
                &image,
                format,
                quality_mode,
                jpeg_quality,
                max_size_bytes,
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
        let mut artifacts = state.artifacts.lock();
        let existing = artifacts
            .iter_mut()
            .find(|existing| existing.id == artifact_id)
            .ok_or_else(|| "the original screenshot is no longer available".to_owned())?;
        *existing = artifact.clone();
        drop(artifacts);
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
        state.artifacts.lock().push(artifact.clone());
        app.emit("capture-completed", &artifact)
            .map_err(|error| error.to_string())?;
        super::refresh_thumbnail_stack(&app);
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

const EDITOR_DRAFT_SCHEMA_VERSION: u16 = 1;
const EDITOR_DRAFT_MANIFEST_FILE: &str = "manifest.json";
const EDITOR_DRAFT_ASSETS_DIR: &str = "assets";
const MAX_EDITOR_DRAFT_ASSETS: usize = 64;
const MAX_EDITOR_DRAFT_TOTAL_BYTES: usize = 80 * 1024 * 1024;

#[derive(Debug, Deserialize)]
pub struct ScreenshotEditorDraftAssetInput {
    id: String,
    png: Vec<u8>,
}

#[derive(Debug, Deserialize)]
pub struct SaveScreenshotEditorDraftRequest {
    artifact_id: String,
    document: serde_json::Value,
    assets: Vec<ScreenshotEditorDraftAssetInput>,
    updated_at_ms: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct ScreenshotEditorDraftManifest {
    schema_version: u16,
    artifact_id: String,
    updated_at_ms: u64,
    document: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct LoadedScreenshotEditorDraft {
    document: serde_json::Value,
    updated_at_ms: u64,
}

/// Persist a layered screenshot editor session so reopening restores unsaved work.
#[tauri::command]
pub fn save_screenshot_editor_draft(
    request: SaveScreenshotEditorDraftRequest,
) -> CommandResult<()> {
    validate_draft_component_id(&request.artifact_id)
        .map_err(|error| error.to_string())?;
    if request.assets.len() > MAX_EDITOR_DRAFT_ASSETS {
        return Err("this edit has too many image layers to keep as a draft".to_owned());
    }
    let mut total_bytes = 0usize;
    for asset in &request.assets {
        validate_draft_component_id(&asset.id).map_err(|error| error.to_string())?;
        if asset.png.is_empty() || asset.png.len() > MAX_EDITOR_PNG_BYTES {
            return Err("a draft image asset is empty or too large".to_owned());
        }
        total_bytes = total_bytes.saturating_add(asset.png.len());
        if total_bytes > MAX_EDITOR_DRAFT_TOTAL_BYTES {
            return Err(
                "unsaved edits are too large to keep as a draft; save a file first".to_owned(),
            );
        }
    }

    let root = screenshot_editor_drafts_directory().join(&request.artifact_id);
    let assets_dir = root.join(EDITOR_DRAFT_ASSETS_DIR);
    fs::create_dir_all(&assets_dir).map_err(|error| error.to_string())?;

    // Replace assets wholesale so removed layers do not leave orphan files.
    if assets_dir.is_dir() {
        for entry in fs::read_dir(&assets_dir).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            let path = entry.path();
            if path.is_file() {
                let _ = fs::remove_file(path);
            }
        }
    }
    for asset in &request.assets {
        let path = assets_dir.join(format!("{}.png", asset.id));
        write_export_atomically(&path, &asset.png).map_err(|error| error.to_string())?;
    }

    let manifest = ScreenshotEditorDraftManifest {
        schema_version: EDITOR_DRAFT_SCHEMA_VERSION,
        artifact_id: request.artifact_id.clone(),
        updated_at_ms: request.updated_at_ms,
        document: request.document,
    };
    let bytes = serde_json::to_vec_pretty(&manifest).map_err(|error| error.to_string())?;
    write_export_atomically(&root.join(EDITOR_DRAFT_MANIFEST_FILE), &bytes)
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// Load a previously autosaved editor draft, if one exists for this capture.
#[tauri::command]
pub fn load_screenshot_editor_draft(
    artifact_id: String,
) -> CommandResult<Option<LoadedScreenshotEditorDraft>> {
    validate_draft_component_id(&artifact_id).map_err(|error| error.to_string())?;
    let root = screenshot_editor_drafts_directory().join(&artifact_id);
    let manifest_path = root.join(EDITOR_DRAFT_MANIFEST_FILE);
    if !manifest_path.is_file() {
        return Ok(None);
    }
    let bytes = fs::read(&manifest_path).map_err(|error| error.to_string())?;
    let mut manifest: ScreenshotEditorDraftManifest =
        serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    if manifest.schema_version != EDITOR_DRAFT_SCHEMA_VERSION {
        let _ = discard_screenshot_editor_draft_files(&artifact_id);
        return Ok(None);
    }
    if manifest.artifact_id != artifact_id {
        return Err("screenshot editor draft does not match its folder".to_owned());
    }
    if !rewrite_draft_document_asset_urls(&mut manifest.document, &artifact_id) {
        let _ = discard_screenshot_editor_draft_files(&artifact_id);
        return Ok(None);
    }
    Ok(Some(LoadedScreenshotEditorDraft {
        document: manifest.document,
        updated_at_ms: manifest.updated_at_ms,
    }))
}

#[tauri::command]
pub fn discard_screenshot_editor_draft(artifact_id: String) -> CommandResult<()> {
    validate_draft_component_id(&artifact_id).map_err(|error| error.to_string())?;
    discard_screenshot_editor_draft_files(&artifact_id).map_err(|error| error.to_string())?;
    Ok(())
}

/// Best-effort cleanup used when history entries are deleted.
pub fn discard_screenshot_editor_draft_files(artifact_id: &str) -> Result<(), AppError> {
    validate_draft_component_id(artifact_id)?;
    let root = screenshot_editor_drafts_directory().join(artifact_id);
    if !root.exists() {
        return Ok(());
    }
    fs::remove_dir_all(root).map_err(AppError::Io)?;
    Ok(())
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
    validate_draft_component_id(artifact_id).ok()?;
    validate_draft_component_id(asset_id).ok()?;
    let file = screenshot_editor_drafts_directory()
        .join(artifact_id)
        .join(EDITOR_DRAFT_ASSETS_DIR)
        .join(format!("{asset_id}.png"));
    fs::read(file).ok()
}

fn validate_draft_component_id(id: &str) -> Result<(), AppError> {
    if id.is_empty() || id.len() > 80 {
        return Err(AppError::Task("invalid screenshot editor draft id".to_owned()));
    }
    if !id
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        return Err(AppError::Task("invalid screenshot editor draft id".to_owned()));
    }
    Ok(())
}

/// Rewrite `draft-asset:{id}` image refs to protocol URLs. Returns false if an
/// asset file is missing so callers can discard a broken draft.
fn rewrite_draft_document_asset_urls(document: &mut serde_json::Value, artifact_id: &str) -> bool {
    let Some(elements) = document
        .get_mut("elements")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return true;
    };
    for element in elements {
        if element.get("kind").and_then(serde_json::Value::as_str) != Some("image") {
            continue;
        }
        for field in ["src", "originalSrc"] {
            let Some(value) = element.get(field).and_then(serde_json::Value::as_str) else {
                continue;
            };
            let Some(asset_id) = value.strip_prefix("draft-asset:") else {
                continue;
            };
            if validate_draft_component_id(asset_id).is_err() {
                return false;
            }
            let asset_path = screenshot_editor_drafts_directory()
                .join(artifact_id)
                .join(EDITOR_DRAFT_ASSETS_DIR)
                .join(format!("{asset_id}.png"));
            if !asset_path.is_file() {
                return false;
            }
            element[field] = serde_json::Value::String(editor_draft_asset_url(artifact_id, asset_id));
        }
    }
    true
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
    quality_mode: ScreenshotExportQualityMode,
    jpeg_quality: u8,
) -> Result<Vec<u8>, AppError> {
    match format {
        ScreenshotEditFormat::Png => {
            let max_colors = match quality_mode {
                ScreenshotExportQualityMode::Preserve => None,
                // Compress: reduce colors like compresspng.com, then pack hard.
                ScreenshotExportQualityMode::Compress => {
                    Some(storage::png_palette_colors_for_quality(jpeg_quality))
                }
                // Maximum without a byte budget still quantizes aggressively.
                ScreenshotExportQualityMode::Maximum => Some(64),
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

fn encode_export_with_limit(
    image: &RgbaImage,
    format: ScreenshotEditFormat,
    quality_mode: ScreenshotExportQualityMode,
    jpeg_quality: u8,
    max_size_bytes: Option<u64>,
) -> Result<Vec<u8>, AppError> {
    let effective_mode = if max_size_bytes.is_some() {
        ScreenshotExportQualityMode::Maximum
    } else {
        quality_mode
    };
    let output = encode_export(image, format, effective_mode, jpeg_quality)?;
    let Some(maximum) = max_size_bytes else {
        return Ok(output);
    };
    if u64::try_from(output.len()).unwrap_or(u64::MAX) <= maximum {
        return Ok(output);
    }

    match format {
        ScreenshotEditFormat::Jpeg => {
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
        ScreenshotEditFormat::Png => {
            // Walk down the color budget until the file fits (same idea as quality notches).
            let mut best: Option<Vec<u8>> = None;
            for colors in storage::PNG_MAXIMUM_COLOR_STEPS {
                let candidate = storage::encode_png_export(image, true, Some(colors))?;
                if u64::try_from(candidate.len()).unwrap_or(u64::MAX) <= maximum {
                    best = Some(candidate);
                    // Keep trying fewer colors? Prefer the highest quality that still fits.
                    // First success in the descending list is the largest palette that fits.
                    break;
                }
                best = Some(candidate);
            }
            let best = best.ok_or_else(|| {
                AppError::Image(
                    "PNG cannot meet the requested maximum; reduce the output size or raise the limit"
                        .to_owned(),
                )
            })?;
            if u64::try_from(best.len()).unwrap_or(u64::MAX) <= maximum {
                Ok(best)
            } else {
                Err(AppError::Image(
                    "the PNG is larger than the requested maximum even after reducing colors; reduce the output size, raise the limit, or switch to JPEG for more aggressive size control"
                        .to_owned(),
                ))
            }
        }
        ScreenshotEditFormat::Webp => {
            let maximum_quality = jpeg_quality.clamp(1, 100);
            let minimum = encode_webp(image, Some(1))?;
            if u64::try_from(minimum.len()).unwrap_or(u64::MAX) > maximum {
                return Err(AppError::Image(
                    "WebP cannot meet the requested maximum at the supported quality range; reduce the output size or raise the limit"
                        .to_owned(),
                ));
            }
            let mut best = minimum;
            let mut low = 2_u8;
            let mut high = maximum_quality;
            while low <= high {
                let quality = low + (high - low) / 2;
                let candidate = encode_webp(image, Some(quality))?;
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
    }
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

/// Encode WebP. `None` quality is lossless; `Some(q)` is lossy at quality 1–100.
/// Keeps alpha (unlike JPEG). Uses libwebp because the `image` crate only encodes lossless WebP.
fn encode_webp(image: &RgbaImage, quality: Option<u8>) -> Result<Vec<u8>, AppError> {
    if image.width() == 0 || image.height() == 0 {
        return Err(AppError::Image(
            "cannot encode an empty WebP image".to_owned(),
        ));
    }
    let encoder = webp::Encoder::from_rgba(image.as_raw(), image.width(), image.height());
    let encoded = match quality {
        None => encoder
            .encode_simple(true, 100.0)
            .map_err(|error| AppError::Image(format!("WebP lossless encode failed: {error:?}")))?,
        Some(q) => encoder
            .encode_simple(false, f32::from(q.clamp(1, 100)))
            .map_err(|error| AppError::Image(format!("WebP lossy encode failed: {error:?}")))?,
    };
    Ok(encoded.to_vec())
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

/// First-available `{stem}.{extension}` in `directory`, with numeric suffixes
/// only when that name is already taken. Does not invent an `-edited` stem —
/// copy naming is a frontend “Make a copy” concern.
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
        ScreenshotEditFormat, ScreenshotExportQualityMode, composite_onto_white, encode_export,
        encode_export_with_limit, resolve_editor_draft_asset, unique_export_path,
        validate_draft_component_id, validated_destination, write_export_atomically,
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
    fn exports_png_jpeg_and_webp_without_changing_format() {
        for (format, expected) in [
            (ScreenshotEditFormat::Png, ImageFormat::Png),
            (ScreenshotEditFormat::Jpeg, ImageFormat::Jpeg),
            (ScreenshotEditFormat::Webp, ImageFormat::WebP),
        ] {
            let bytes = encode_export(&sample(), format, ScreenshotExportQualityMode::Compress, 92)
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
        )
        .unwrap();
        let compressed = encode_export(
            &image,
            ScreenshotEditFormat::Png,
            ScreenshotExportQualityMode::Compress,
            85,
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
        )
        .unwrap();
        let tiny = encode_export(
            &image,
            ScreenshotEditFormat::Png,
            ScreenshotExportQualityMode::Compress,
            55,
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
    fn webp_compress_quality_reduces_file_size_via_lossy_encode() {
        let image = detailed_sample();
        let high = encode_export(
            &image,
            ScreenshotEditFormat::Webp,
            ScreenshotExportQualityMode::Compress,
            92,
        )
        .unwrap();
        let tiny = encode_export(
            &image,
            ScreenshotEditFormat::Webp,
            ScreenshotExportQualityMode::Compress,
            55,
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
        )
        .unwrap();
        let low = encode_export(
            &image,
            ScreenshotEditFormat::Webp,
            ScreenshotExportQualityMode::Compress,
            20,
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
    fn jpeg_quality_falls_until_the_requested_file_limit_is_met() {
        let image = detailed_sample();
        let high = encode_export(
            &image,
            ScreenshotEditFormat::Jpeg,
            ScreenshotExportQualityMode::Compress,
            100,
        )
        .unwrap();
        let low = encode_export(
            &image,
            ScreenshotEditFormat::Jpeg,
            ScreenshotExportQualityMode::Compress,
            40,
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
        )
        .expect("JPEG fits the requested maximum");

        assert!(u64::try_from(limited.len()).unwrap() <= maximum);
        image::load_from_memory_with_format(&limited, ImageFormat::Jpeg)
            .expect("limited JPEG remains readable");
    }

    #[test]
    fn png_exports_explain_when_the_requested_limit_cannot_be_met() {
        let error = encode_export_with_limit(
            &detailed_sample(),
            ScreenshotEditFormat::Png,
            ScreenshotExportQualityMode::Maximum,
            100,
            Some(10),
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
    fn validates_draft_component_ids() {
        assert!(validate_draft_component_id("capture-1").is_ok());
        assert!(validate_draft_component_id("a_b-09").is_ok());
        assert!(validate_draft_component_id("../etc").is_err());
        assert!(validate_draft_component_id("has/slash").is_err());
        assert!(validate_draft_component_id("").is_err());
    }

    #[test]
    fn resolves_editor_draft_asset_paths_safely() {
        assert!(resolve_editor_draft_asset("editor-draft/../secret/asset").is_none());
        assert!(resolve_editor_draft_asset("editor-draft/capture-1").is_none());
        assert!(resolve_editor_draft_asset("artifact-full/capture-1").is_none());
    }
}
