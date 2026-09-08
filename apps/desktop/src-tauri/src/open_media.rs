use std::{
    collections::HashSet,
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use chrono::Utc;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use uuid::Uuid;

use captures_capture::CaptureMode;
use captures_recording::{RecordingKind, RecordingTarget};

use crate::{
    AppError,
    models::{
        ArtifactKind, CaptureArtifact, ClipboardCopyStatus, HistoryEntry, RecordingArtifact,
        artifact_full_url, artifact_url, history_full_url, history_preview_url,
    },
    recording, screenshot_editor,
    state::AppState,
    storage,
};

const UNSUPPORTED_OPEN_MESSAGE: &str =
    "Captures can open PNG, JPEG, WebP, GIF, MP4, and WebM files.";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OpenedMediaKind {
    Still,
    Gif,
    Video,
}

struct OpenQueue {
    ready: bool,
    pending: Vec<PathBuf>,
}

static OPEN_QUEUE: Mutex<OpenQueue> = Mutex::new(OpenQueue {
    ready: false,
    pending: Vec::new(),
});

fn open_queue() -> std::sync::MutexGuard<'static, OpenQueue> {
    OPEN_QUEUE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub(crate) fn enqueue_or_open(app: &AppHandle, paths: Vec<PathBuf>) {
    let paths = dedupe_paths(paths);
    if paths.is_empty() {
        return;
    }
    let mut queue = open_queue();
    if queue.ready {
        drop(queue);
        spawn_open(app, paths);
        return;
    }
    queue.pending.extend(paths);
}

/// Drain queued paths, include this process's CLI arguments, and open them.
/// Returns whether any media path was handed to the editors.
pub(crate) fn finish_setup(app: &AppHandle) -> bool {
    let launch_paths = open_paths_from_cli_args(std::env::args(), std::env::current_dir().ok());
    let mut queue = open_queue();
    queue.ready = true;
    let mut paths = std::mem::take(&mut queue.pending);
    drop(queue);
    paths.extend(launch_paths);
    let paths = dedupe_paths(paths);
    let opening = !paths.is_empty();
    if opening {
        spawn_open(app, paths);
    }
    opening
}

pub(crate) fn open_paths_from_cli_args<I, S>(args: I, cwd: Option<PathBuf>) -> Vec<PathBuf>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let args: Vec<String> = args
        .into_iter()
        .map(|value| value.as_ref().to_owned())
        .collect();
    let args = skip_program_name(&args);
    args.iter()
        .filter_map(|argument| path_from_open_argument(argument))
        .map(|path| resolve_against_cwd(path, cwd.as_deref()))
        .filter(|path| classify_media_path(path).is_some())
        .collect()
}

fn skip_program_name(args: &[String]) -> &[String] {
    match args.split_first() {
        Some((first, rest)) if classify_media_path(Path::new(first)).is_none() => rest,
        _ => args,
    }
}

fn resolve_against_cwd(path: PathBuf, cwd: Option<&Path>) -> PathBuf {
    if path.is_absolute() {
        return path;
    }
    cwd.map_or(path.clone(), |cwd| cwd.join(path))
}

pub(crate) fn path_from_open_argument(value: &str) -> Option<PathBuf> {
    let value = value.trim();
    if value.is_empty() || is_cli_flag(value) {
        return None;
    }
    if value.starts_with("file:") {
        return parse_file_url(value);
    }
    Some(PathBuf::from(value))
}

fn is_cli_flag(value: &str) -> bool {
    value == crate::AUTOSTART_ARG || value == "--" || value.starts_with('-')
}

fn parse_file_url(value: &str) -> Option<PathBuf> {
    let rest = value.strip_prefix("file:")?;
    let rest = rest.trim_start_matches("//");
    let path_part = if rest.starts_with('/') {
        rest.to_owned()
    } else {
        let (_, path) = rest.split_once('/')?;
        format!("/{path}")
    };
    let decoded = percent_decode(&path_part)?;
    Some(PathBuf::from(normalize_file_url_path(decoded)))
}

fn normalize_file_url_path(decoded: String) -> String {
    #[cfg(windows)]
    {
        let bytes = decoded.as_bytes();
        if bytes.len() >= 3 && bytes[0] == b'/' && bytes[2] == b':' {
            return decoded[1..].to_owned();
        }
    }
    decoded
}

fn percent_decode(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return None;
            }
            let high = hex_digit(bytes[index + 1])?;
            let low = hex_digit(bytes[index + 2])?;
            out.push((high << 4) | low);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).ok()
}

const fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

pub(crate) fn classify_media_path(path: &Path) -> Option<OpenedMediaKind> {
    kind_from_extension(path).or_else(|| kind_from_magic(path))
}

fn kind_from_extension(path: &Path) -> Option<OpenedMediaKind> {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png" | "jpg" | "jpeg" | "webp") => Some(OpenedMediaKind::Still),
        Some("gif") => Some(OpenedMediaKind::Gif),
        Some("mp4" | "webm") => Some(OpenedMediaKind::Video),
        _ => None,
    }
}

fn kind_from_magic(path: &Path) -> Option<OpenedMediaKind> {
    let mut header = [0_u8; 16];
    let mut file = fs::File::open(path).ok()?;
    let read = file.read(&mut header).ok()?;
    if read < 3 {
        return None;
    }
    if header.starts_with(&[0x89, b'P', b'N', b'G'])
        || header.starts_with(&[0xFF, 0xD8, 0xFF])
        || (read >= 12 && header.starts_with(b"RIFF") && &header[8..12] == b"WEBP")
    {
        return Some(OpenedMediaKind::Still);
    }
    if header.starts_with(b"GIF87a") || header.starts_with(b"GIF89a") {
        return Some(OpenedMediaKind::Gif);
    }
    if is_mp4_compatible_header(&header[..read]) {
        return Some(OpenedMediaKind::Video);
    }
    if header.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        return Some(OpenedMediaKind::Video);
    }
    None
}

fn is_mp4_compatible_header(header: &[u8]) -> bool {
    if header.len() < 12 || &header[4..8] != b"ftyp" {
        return false;
    }
    matches!(
        &header[8..12],
        b"isom"
            | b"iso2"
            | b"iso3"
            | b"iso4"
            | b"iso5"
            | b"iso6"
            | b"mp41"
            | b"mp42"
            | b"mp71"
            | b"avc1"
            | b"M4V "
            | b"dash"
            | b"mp4v"
    )
}

fn spawn_open(app: &AppHandle, paths: Vec<PathBuf>) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        for path in paths {
            if let Err(error) = open_one(&app, path).await {
                report_open_error(&app, &error);
            }
        }
    });
}

async fn open_one(app: &AppHandle, path: PathBuf) -> Result<(), AppError> {
    if !path.is_file() {
        return Err(AppError::Task(format!(
            "Captures could not find “{}”.",
            display_name(&path)
        )));
    }
    let kind = classify_media_path(&path)
        .ok_or_else(|| AppError::Task(UNSUPPORTED_OPEN_MESSAGE.to_owned()))?;
    match kind {
        OpenedMediaKind::Still => open_still(app, path).await,
        OpenedMediaKind::Gif => {
            recording::open_recording_from_path(app, path, RecordingKind::Gif).await
        }
        OpenedMediaKind::Video => {
            recording::open_recording_from_path(app, path, RecordingKind::Video).await
        }
    }
}

async fn open_still(app: &AppHandle, path: PathBuf) -> Result<(), AppError> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    if let Some(artifact_id) = existing_screenshot_id(&state, &path) {
        load_screenshot_if_needed(&state, &artifact_id).await?;
        return screenshot_editor::show_screenshot_editor(app, &artifact_id);
    }

    let path_for_decode = path.clone();
    let (image_png, preview_png, width, height) = tauri::async_runtime::spawn_blocking(move || {
        let image = screenshot_editor::decode_still_image_file(&path_for_decode)?;
        let width = image.width();
        let height = image.height();
        let image_png = storage::encode_png(&image)?;
        let preview_png = storage::encode_thumbnail_png(&image)?;
        Ok::<_, AppError>((image_png, preview_png, width, height))
    })
    .await
    .map_err(|error| AppError::Task(error.to_string()))??;

    let artifact_id = Uuid::new_v4().to_string();
    let created_at = Utc::now().to_rfc3339();
    let size_bytes = u64::try_from(image_png.len()).unwrap_or(u64::MAX);
    let saved_path = path.to_string_lossy().into_owned();
    let history_entry = HistoryEntry {
        id: artifact_id.clone(),
        kind: ArtifactKind::Screenshot,
        preview_url: history_preview_url(&artifact_id),
        full_url: history_full_url(&artifact_id),
        width,
        height,
        size_bytes,
        created_at: created_at.clone(),
        mode: Some(CaptureMode::Display),
        saved_path: Some(saved_path.clone()),
        mime_type: None,
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
                eprintln!("failed to save opened screenshot history: {error}");
                false
            }
        };
    let artifact = CaptureArtifact {
        id: artifact_id.clone(),
        path: Some(saved_path),
        preview_url: artifact_url(&artifact_id),
        full_url: artifact_full_url(&artifact_id),
        width,
        height,
        size_bytes,
        created_at,
        mode: CaptureMode::Display,
        history_saved,
        clipboard_copy_status: ClipboardCopyStatus::Skipped,
        image_png,
        preview_png,
    };
    state.artifacts.lock().push(artifact);
    if history_saved {
        state.history.lock().insert(0, history_entry);
        let _ = app.emit("capture-history-changed", ());
    }
    screenshot_editor::show_screenshot_editor(app, &artifact_id)
}

async fn load_screenshot_if_needed(state: &AppState, artifact_id: &str) -> Result<(), AppError> {
    if state.find_artifact(artifact_id).is_some() {
        return Ok(());
    }
    let entry = state
        .history
        .lock()
        .iter()
        .find(|entry| entry.id == artifact_id)
        .cloned()
        .ok_or(AppError::HistoryUnavailable)?;
    if entry.kind != ArtifactKind::Screenshot {
        return Err(AppError::HistoryUnavailable);
    }
    let mode = entry.mode.ok_or(AppError::HistoryUnavailable)?;
    let history_artifact_id = artifact_id.to_owned();
    let (image_png, preview_png) = tauri::async_runtime::spawn_blocking(move || {
        storage::load_history_images(&history_artifact_id)
    })
    .await
    .map_err(|error| AppError::Task(error.to_string()))??;
    let path = entry
        .saved_path
        .as_ref()
        .filter(|saved| Path::new(saved).is_file())
        .cloned();
    state.artifacts.lock().push(CaptureArtifact {
        id: entry.id,
        path,
        preview_url: artifact_url(artifact_id),
        full_url: artifact_full_url(artifact_id),
        width: entry.width,
        height: entry.height,
        size_bytes: entry.size_bytes,
        created_at: entry.created_at,
        mode,
        history_saved: true,
        clipboard_copy_status: ClipboardCopyStatus::Skipped,
        image_png,
        preview_png,
    });
    Ok(())
}

fn existing_screenshot_id(state: &AppState, path: &Path) -> Option<String> {
    let canonical = path.canonicalize().ok();
    state
        .artifacts
        .lock()
        .iter()
        .find_map(|artifact| {
            artifact
                .path
                .as_deref()
                .filter(|saved| paths_match(Path::new(saved), path, canonical.as_deref()))
                .map(|_| artifact.id.clone())
        })
        .or_else(|| {
            state.history.lock().iter().find_map(|entry| {
                if entry.kind != ArtifactKind::Screenshot {
                    return None;
                }
                entry
                    .saved_path
                    .as_deref()
                    .filter(|saved| paths_match(Path::new(saved), path, canonical.as_deref()))
                    .map(|_| entry.id.clone())
            })
        })
}

pub(crate) fn existing_recording_id(state: &AppState, path: &Path) -> Option<String> {
    let canonical = path.canonicalize().ok();
    state
        .recording_artifacts
        .lock()
        .iter()
        .find_map(|artifact| {
            recording_paths(&artifact.summary)
                .into_iter()
                .find(|saved| paths_match(saved, path, canonical.as_deref()))
                .map(|_| artifact.summary.id.clone())
        })
        .or_else(|| {
            state.history.lock().iter().find_map(|entry| {
                entry.kind.recording_kind()?;
                entry
                    .saved_path
                    .as_deref()
                    .filter(|saved| paths_match(Path::new(saved), path, canonical.as_deref()))
                    .map(|_| entry.id.clone())
            })
        })
}

fn recording_paths(artifact: &RecordingArtifact) -> Vec<PathBuf> {
    let mut paths = vec![PathBuf::from(&artifact.path)];
    if let Some(saved_path) = &artifact.saved_path {
        paths.push(PathBuf::from(saved_path));
    }
    paths
}

pub(crate) fn opened_recording_target() -> RecordingTarget {
    RecordingTarget::Display {
        display_id: "opened-file".to_owned(),
    }
}

pub(crate) fn recording_mime_type(path: &Path, fallback: &str) -> String {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("gif") => "image/gif".to_owned(),
        Some("webm") => "video/webm".to_owned(),
        Some("mp4") => "video/mp4".to_owned(),
        _ => fallback.to_owned(),
    }
}

fn paths_match(left: &Path, right: &Path, right_canonical: Option<&Path>) -> bool {
    if left == right {
        return true;
    }
    match (
        left.canonicalize().ok(),
        right_canonical.map(Path::to_path_buf),
    ) {
        (Some(left), Some(right)) => left == right,
        (Some(left), None) => right.canonicalize().ok().is_some_and(|right| left == right),
        (None, Some(right)) => left == right.as_path(),
        (None, None) => false,
    }
}

fn dedupe_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    let mut unique = Vec::new();
    for path in paths {
        let key = path.canonicalize().unwrap_or_else(|_| path.clone());
        if seen.insert(key) {
            unique.push(path);
        }
    }
    unique
}

fn display_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .map_or_else(|| path.display().to_string(), str::to_owned)
}

fn report_open_error(app: &AppHandle, error: &AppError) {
    eprintln!("failed to open media: {error}");
    app.dialog()
        .message(error.to_string())
        .title("Captures")
        .buttons(MessageDialogButtons::Ok)
        .kind(MessageDialogKind::Error)
        .show(|_| {});
}

#[cfg(test)]
mod tests {
    use std::fs;

    use image::{ImageEncoder, Rgba, RgbaImage, codecs::png::PngEncoder};
    use tempfile::tempdir;

    use super::{
        OpenedMediaKind, classify_media_path, is_cli_flag, open_paths_from_cli_args,
        parse_file_url, path_from_open_argument, recording_mime_type, skip_program_name,
    };

    fn write_png(path: &std::path::Path) {
        let image = RgbaImage::from_pixel(2, 2, Rgba([12, 24, 36, 255]));
        let file = fs::File::create(path).expect("png file");
        PngEncoder::new(file)
            .write_image(
                image.as_raw(),
                image.width(),
                image.height(),
                image::ExtendedColorType::Rgba8,
            )
            .expect("png written");
    }

    #[test]
    fn classifies_editor_compatible_extensions() {
        assert_eq!(
            classify_media_path(std::path::Path::new("shot.PNG")),
            Some(OpenedMediaKind::Still)
        );
        assert_eq!(
            classify_media_path(std::path::Path::new("shot.jpeg")),
            Some(OpenedMediaKind::Still)
        );
        assert_eq!(
            classify_media_path(std::path::Path::new("shot.webp")),
            Some(OpenedMediaKind::Still)
        );
        assert_eq!(
            classify_media_path(std::path::Path::new("clip.GIF")),
            Some(OpenedMediaKind::Gif)
        );
        assert_eq!(
            classify_media_path(std::path::Path::new("clip.mp4")),
            Some(OpenedMediaKind::Video)
        );
        assert_eq!(
            classify_media_path(std::path::Path::new("clip.webm")),
            Some(OpenedMediaKind::Video)
        );
        assert_eq!(classify_media_path(std::path::Path::new("notes.txt")), None);
        assert_eq!(classify_media_path(std::path::Path::new("clip.mov")), None);
        assert_eq!(
            classify_media_path(std::path::Path::new("photo.heic")),
            None
        );
    }

    fn write_bytes(path: &std::path::Path, bytes: &[u8]) {
        fs::write(path, bytes).expect("header written");
    }

    #[test]
    fn sniffs_mp4_and_rejects_heic_and_quicktime_ftyp_brands() {
        let directory = tempdir().expect("temporary directory");
        let mp4 = directory.path().join("clip");
        let mut mp4_header = [0_u8; 12];
        mp4_header[4..8].copy_from_slice(b"ftyp");
        mp4_header[8..12].copy_from_slice(b"isom");
        write_bytes(&mp4, &mp4_header);
        assert_eq!(classify_media_path(&mp4), Some(OpenedMediaKind::Video));

        let heic = directory.path().join("photo");
        let mut heic_header = [0_u8; 12];
        heic_header[4..8].copy_from_slice(b"ftyp");
        heic_header[8..12].copy_from_slice(b"heic");
        write_bytes(&heic, &heic_header);
        assert_eq!(classify_media_path(&heic), None);

        let mov = directory.path().join("quicktime");
        let mut mov_header = [0_u8; 12];
        mov_header[4..8].copy_from_slice(b"ftyp");
        mov_header[8..12].copy_from_slice(b"qt  ");
        write_bytes(&mov, &mov_header);
        assert_eq!(classify_media_path(&mov), None);
    }

    #[test]
    fn sniffs_png_files_without_an_extension() {
        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join("screenshot");
        write_png(&path);
        assert_eq!(classify_media_path(&path), Some(OpenedMediaKind::Still));
    }

    #[test]
    fn skips_cli_flags() {
        assert!(is_cli_flag("--captures-autostart"));
        assert!(is_cli_flag("--gtk-version"));
        assert!(path_from_open_argument("--captures-autostart").is_none());
        assert_eq!(
            path_from_open_argument(" /tmp/still.png ").as_deref(),
            Some(std::path::Path::new("/tmp/still.png"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn parses_unix_file_urls() {
        assert_eq!(
            parse_file_url("file:///tmp/My%20Photo.png").as_deref(),
            Some(std::path::Path::new("/tmp/My Photo.png"))
        );
        assert_eq!(
            parse_file_url("file://localhost/tmp/caf%C3%A9.webp").as_deref(),
            Some(std::path::Path::new("/tmp/café.webp"))
        );
    }

    #[cfg(windows)]
    #[test]
    fn parses_windows_file_urls() {
        assert_eq!(
            parse_file_url("file:///C:/Users/me/My%20Photo.png").as_deref(),
            Some(std::path::Path::new("C:/Users/me/My Photo.png"))
        );
    }

    #[test]
    fn skips_the_executable_name_then_keeps_media_paths() {
        let args = vec![
            "/usr/bin/captures".to_owned(),
            "--captures-autostart".to_owned(),
            "photo.png".to_owned(),
            "clip.mp4".to_owned(),
        ];
        assert_eq!(
            skip_program_name(&args),
            &[
                "--captures-autostart".to_owned(),
                "photo.png".to_owned(),
                "clip.mp4".to_owned()
            ]
        );
        let cwd = tempdir().expect("cwd");
        let paths = open_paths_from_cli_args(
            [
                "/usr/bin/captures",
                "--captures-autostart",
                "photo.png",
                "clip.webm",
            ],
            Some(cwd.path().to_path_buf()),
        );
        assert_eq!(
            paths,
            vec![cwd.path().join("photo.png"), cwd.path().join("clip.webm")]
        );
        assert!(
            open_paths_from_cli_args(
                ["/usr/bin/captures", "true", "debug", "notes.txt"],
                Some(cwd.path().to_path_buf()),
            )
            .is_empty()
        );
    }

    #[test]
    fn uses_the_container_mime_type_for_opened_recordings() {
        assert_eq!(
            recording_mime_type(std::path::Path::new("clip.webm"), "video/mp4"),
            "video/webm"
        );
        assert_eq!(
            recording_mime_type(std::path::Path::new("clip.gif"), "video/mp4"),
            "image/gif"
        );
        assert_eq!(
            recording_mime_type(std::path::Path::new("clip.mp4"), "video/webm"),
            "video/mp4"
        );
    }

    #[test]
    fn file_associations_cover_every_openable_extension() {
        let config = include_str!("../tauri.conf.json");
        for extension in ["png", "jpg", "jpeg", "webp", "gif", "mp4", "webm"] {
            assert!(
                config.contains(&format!("\"{extension}\"")),
                "tauri.conf.json should associate .{extension}"
            );
        }
        assert!(config.contains("\"rank\": \"Alternate\""));
    }
}
