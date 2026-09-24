//! Shared native application operations. Hosts schedule these off the UI thread.
//! Images stay in owned buffers/files, never JSON/base64. No browser or host window APIs.

pub mod capture_flow;
pub mod editor;
pub mod editor_fonts;
pub mod editor_image_background;
pub mod editor_image_decode;
pub mod editor_output;
pub mod editor_render;
pub mod editor_session;
pub mod editor_text;
pub mod editor_viewport;
pub mod preview;
pub mod recording_editor;
pub mod recording_timeline;
pub mod region;
pub mod selection;
pub mod shortcuts;
pub mod window;

use captures_capture::{CaptureError, CaptureMode, DisplayDescriptor, XcapBackend};
use captures_history::{ArtifactKind, HistoryEntry};
use captures_settings::ScreenshotFormat;
use chrono::{Local, Utc};
use image::RgbaImage;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Capture(#[from] CaptureError),
    #[error(transparent)]
    History(#[from] captures_history::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("image encoding failed: {0}")]
    Image(String),
    #[error("capture is no longer available")]
    Missing,
    #[error("Capture cancelled")]
    Cancelled,
    #[error("The selected display changed. Select the capture target again.")]
    DisplayChanged,
    #[error("Select a valid region inside the display.")]
    InvalidRegion,
    #[error("Window corner radius must be finite and nonnegative.")]
    InvalidWindowRadius,
    #[error("Saved to {path}, but history could not be updated: {reason}")]
    SavedWithoutMetadata { path: String, reason: String },
}

/// Development data stays beside the separate native settings identity.
pub fn default_history_root() -> PathBuf {
    captures_settings::default_native_settings_path().with_file_name("capture-history")
}

#[derive(Debug, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum Request {
    DefaultHistoryRoot,
    Displays,
    RequestPermission,
    CaptureDisplay {
        root: PathBuf,
        display_id: String,
        generation: u64,
        include_cursor: bool,
    },
    History {
        root: PathBuf,
    },
    SaveScreenshot {
        root: PathBuf,
        id: String,
        directory: PathBuf,
        format: ScreenshotFormat,
    },
    SaveRecording {
        root: PathBuf,
        id: String,
        directory: PathBuf,
    },
    Delete {
        root: PathBuf,
        id: String,
    },
    ClearHistory {
        root: PathBuf,
    },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Artifact {
    pub entry: HistoryEntry,
    pub image_path: PathBuf,
    pub preview_path: PathBuf,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Response {
    HistoryRoot { path: PathBuf },
    Displays { displays: Vec<DisplayDescriptor> },
    PermissionGranted,
    Captured { artifact: Artifact },
    History { artifacts: Vec<Artifact> },
    Saved { artifact: Artifact, path: PathBuf },
    Deleted { id: String },
}

/// Permission prompting happens only in RequestPermission, an explicit user action.
/// Hosts hide their capture UI before CaptureDisplay and always restore it on reply.
pub fn execute(request: Request) -> Result<Response, Error> {
    match request {
        Request::DefaultHistoryRoot => Ok(Response::HistoryRoot {
            path: default_history_root(),
        }),
        Request::Displays => {
            XcapBackend.ensure_permission(false)?;
            Ok(Response::Displays {
                displays: XcapBackend.displays()?,
            })
        }
        Request::RequestPermission => {
            XcapBackend.ensure_permission(true)?;
            Ok(Response::PermissionGranted)
        }
        Request::CaptureDisplay {
            root,
            display_id,
            generation,
            include_cursor,
        } => {
            if !capture_flow::is_current(generation) {
                return Err(Error::Cancelled);
            }
            XcapBackend.ensure_permission(false)?;
            if !captures_session::capture_session_available() {
                return Err(CaptureError::SessionUnavailable.into());
            }
            captures_session::dismiss_transient_shell_ui_before_capture();
            let cursor = include_cursor
                .then(captures_capture::pointer_cursor)
                .flatten();
            let mut frame = XcapBackend.capture_display(&display_id)?;
            // A session may lock during a backend/portal round trip. Discard it.
            if !captures_session::capture_session_available() {
                return Err(CaptureError::SessionUnavailable.into());
            }
            if let Some(cursor) = cursor {
                captures_capture::overlay_pointer_cursor(
                    &mut frame.image,
                    &frame.descriptor,
                    &cursor,
                    captures_capture::screenshot_pointer_scale(frame.descriptor.scale_factor),
                );
            }
            // Linearize Cancel versus Save before the irreversible history write.
            // Once committed, Escape cannot claim that the capture was cancelled.
            if !capture_flow::commit(generation) {
                return Err(Error::Cancelled);
            }
            Ok(Response::Captured {
                artifact: persist_screenshot(&root, &frame.image, CaptureMode::Display)?,
            })
        }
        Request::History { root } => Ok(Response::History {
            artifacts: list(&root)?,
        }),
        Request::SaveScreenshot {
            root,
            id,
            directory,
            format,
        } => save_screenshot(&root, &id, &directory, format),
        Request::SaveRecording {
            root,
            id,
            directory,
        } => save_recording(&root, &id, &directory),
        Request::Delete { root, id } => {
            captures_history::delete(&root, &id)?;
            Ok(Response::Deleted { id })
        }
        Request::ClearHistory { root } => {
            // Both hosts confirm clearing every capture kind, independent of the
            // selected filter. Never follow export paths or remove another root.
            // Hosts refresh even on error: deletion can be partial.
            for entry in captures_history::load(&root, Utc::now())? {
                captures_history::delete(&root, &entry.id)?;
            }
            Ok(Response::History {
                artifacts: list(&root)?,
            })
        }
    }
}

fn artifact(root: &Path, entry: HistoryEntry) -> Result<Artifact, Error> {
    let directory = captures_history::entry_directory(root, &entry.id)?;
    let image_path = directory.join(if entry.kind.is_recording() {
        captures_history::HISTORY_PREVIEW_FILE
    } else {
        captures_history::HISTORY_IMAGE_FILE
    });
    Ok(Artifact {
        entry,
        image_path,
        preview_path: directory.join(captures_history::HISTORY_PREVIEW_FILE),
    })
}

pub fn list(root: &Path) -> Result<Vec<Artifact>, Error> {
    captures_history::load(root, Utc::now())?
        .into_iter()
        .filter(|entry| entry.kind == ArtifactKind::Screenshot)
        .map(|entry| artifact(root, entry))
        .collect()
}

/// Commit a captured, color-normalized buffer once. Reused by backend tests.
pub fn persist_screenshot(
    root: &Path,
    image: &RgbaImage,
    mode: CaptureMode,
) -> Result<Artifact, Error> {
    let png = captures_history::encode_png(image)?;
    let preview = captures_history::encode_thumbnail_png(image)?;
    let entry = HistoryEntry {
        id: uuid::Uuid::new_v4().to_string(),
        kind: ArtifactKind::Screenshot,
        preview_url: String::new(),
        full_url: String::new(),
        width: image.width(),
        height: image.height(),
        size_bytes: png.len() as u64,
        created_at: Utc::now().to_rfc3339(),
        mode: Some(mode),
        saved_path: None,
        mime_type: Some("image/png".into()),
        duration_ms: None,
        target: None,
        has_system_audio: false,
        has_microphone_audio: false,
        dropped_frames: 0,
    };
    captures_history::save_capture(root, &entry, &png, &preview)?;
    artifact(root, entry)
}

fn save_screenshot(
    root: &Path,
    id: &str,
    directory: &Path,
    format: ScreenshotFormat,
) -> Result<Response, Error> {
    let item = list(root)?
        .into_iter()
        .find(|item| item.entry.id == id)
        .ok_or(Error::Missing)?;
    // Like shipping Save, a second click reuses the exported file. If it was
    // removed outside Captures, the lossless history copy can recreate it.
    if let Some(path) = item
        .entry
        .saved_path
        .as_ref()
        .map(PathBuf::from)
        .filter(|p| p.is_file())
    {
        return Ok(Response::Saved {
            artifact: item,
            path,
        });
    }
    fs::create_dir_all(directory)?;
    let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
    if format == ScreenshotFormat::Png {
        std::io::copy(&mut fs::File::open(&item.image_path)?, &mut temporary)?;
    } else {
        let image = image::open(&item.image_path)
            .map_err(|e| Error::Image(e.to_string()))?
            .into_rgba8();
        let bytes = match format {
            ScreenshotFormat::Jpeg => {
                captures_image::encode_jpeg(&captures_image::composite_onto_white(&image), 100)
            }
            ScreenshotFormat::Webp => captures_image::encode_webp(&image, None),
            ScreenshotFormat::Png => unreachable!("PNG is copied without re-encoding"),
        }
        .map_err(Error::Image)?;
        temporary.write_all(&bytes)?;
    }
    publish_export(root, item, temporary, directory, format.extension())
}

fn save_recording(root: &Path, id: &str, directory: &Path) -> Result<Response, Error> {
    let entry = captures_history::load(root, Utc::now())?
        .into_iter()
        .find(|entry| entry.id == id && entry.kind.is_recording())
        .ok_or(Error::Missing)?;
    let item = artifact(root, entry)?;
    if let Some(path) = item
        .entry
        .saved_path
        .as_ref()
        .map(PathBuf::from)
        .filter(|path| path.is_file())
    {
        return Ok(Response::Saved {
            artifact: item,
            path,
        });
    }
    let source = item
        .entry
        .recording_media_path(root)
        .filter(|path| path.is_file())
        .ok_or(Error::Missing)?;
    let extension = source
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or(if item.entry.kind == ArtifactKind::Gif {
            "gif"
        } else {
            "mp4"
        });
    fs::create_dir_all(directory)?;
    let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
    std::io::copy(&mut fs::File::open(&source)?, &mut temporary)?;
    publish_export(root, item, temporary, directory, extension)
}

/// Atomically publish prepared bytes without overwriting another file, then
/// record the export separately from the retained private History artifact.
fn publish_export(
    root: &Path,
    mut item: Artifact,
    temporary: tempfile::NamedTempFile,
    directory: &Path,
    extension: &str,
) -> Result<Response, Error> {
    temporary.as_file().sync_all()?;
    let stem = format!("Captures_{}", Local::now().format("%Y-%m-%d_%H-%M-%S_%3f"));
    let path = (0_u32..)
        .find_map(|suffix| {
            let name = if suffix == 0 {
                format!("{stem}.{extension}")
            } else {
                format!("{stem}-{suffix}.{extension}")
            };
            let path = directory.join(name);
            (!path.exists()).then_some(path)
        })
        .expect("available export filename");
    // No-clobber also protects against another save racing the filename check.
    temporary
        .persist_noclobber(&path)
        .map_err(|error| error.error)?;
    item.entry.saved_path = Some(path.to_string_lossy().into_owned());
    captures_history::update_metadata(root, &item.entry).map_err(|error| {
        Error::SavedWithoutMetadata {
            path: path.to_string_lossy().into_owned(),
            reason: error.to_string(),
        }
    })?;
    Ok(Response::Saved {
        artifact: item,
        path,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_capture_is_rejected_before_permission_or_disk_access() {
        let root = tempfile::tempdir().unwrap();
        assert!(matches!(
            execute(Request::CaptureDisplay {
                root: root.path().join("must-not-be-created"),
                display_id: "not-a-real-display".into(),
                generation: 0,
                include_cursor: true,
            }),
            Err(Error::Cancelled)
        ));
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    }

    #[test]
    fn recording_save_copies_original_media_and_preserves_private_history() {
        for (kind, extension, mime) in [
            (ArtifactKind::Video, "mp4", "video/mp4"),
            (ArtifactKind::Video, "webm", "video/webm"),
            (ArtifactKind::Gif, "gif", "image/gif"),
        ] {
            let root = tempfile::tempdir().unwrap();
            let output = tempfile::tempdir().unwrap();
            let screenshot =
                persist_screenshot(root.path(), &RgbaImage::new(7, 3), CaptureMode::Window)
                    .unwrap();
            let poster = fs::read(&screenshot.preview_path).unwrap();
            let mut entry = screenshot.entry.clone();
            entry.id = uuid::Uuid::new_v4().to_string();
            entry.kind = kind;
            entry.mime_type = Some(mime.into());
            entry.duration_ms = Some(1234);
            entry.target = Some(
                serde_json::from_value(
                    serde_json::json!({"type":"display", "display_id":"fixture"}),
                )
                .unwrap(),
            );
            let source = root.path().join(format!("source.{extension}"));
            let media = b"original media bytes, not the PNG poster\x00\x18\xff";
            fs::write(&source, media).unwrap();
            let private =
                captures_history::save_recording(root.path(), &entry, &poster, &source).unwrap();

            // Wrong-kind requests and blocked destinations must not publish or
            // mark a recording saved. They must leave the private media intact.
            let unused = output.path().join("unused");
            assert!(save_recording(root.path(), &screenshot.entry.id, &unused).is_err());
            assert!(!unused.exists());
            let blocked = output.path().join("blocked");
            fs::write(&blocked, b"keep").unwrap();
            assert!(save_recording(root.path(), &entry.id, &blocked).is_err());
            assert!(
                captures_history::load(root.path(), Utc::now())
                    .unwrap()
                    .iter()
                    .find(|item| item.id == entry.id)
                    .unwrap()
                    .saved_path
                    .is_none()
            );

            let request = serde_json::from_value(serde_json::json!({
                "operation":"save_recording", "root":root.path(), "id":entry.id, "directory":output.path(),
            })).unwrap();
            let Response::Saved { artifact, path } = execute(request).unwrap() else {
                panic!("saved response")
            };
            assert_eq!(path.extension().unwrap(), extension);
            assert_eq!(fs::read(&path).unwrap(), media);
            assert_eq!(fs::read(&private).unwrap(), media);
            assert_eq!(
                artifact.image_path, artifact.preview_path,
                "recordings must keep rendering their poster"
            );
            assert_eq!(fs::read(&artifact.image_path).unwrap(), poster);
            assert_eq!(artifact.entry.duration_ms, Some(1234));
            assert_eq!(artifact.entry.saved_path.as_deref(), path.to_str());
            let Response::Saved { path: reused, .. } =
                save_recording(root.path(), &entry.id, &unused).unwrap()
            else {
                panic!("saved response")
            };
            assert_eq!(reused, path);
            assert!(
                !unused.exists(),
                "repeat Save does not create another export directory"
            );

            fs::remove_file(&path).unwrap();
            let Response::Saved {
                path: recreated, ..
            } = save_recording(root.path(), &entry.id, output.path()).unwrap()
            else {
                panic!("saved response")
            };
            assert_eq!(fs::read(&recreated).unwrap(), media);
            assert_eq!(fs::read(blocked).unwrap(), b"keep");
            execute(Request::ClearHistory {
                root: root.path().into(),
            })
            .unwrap();
            assert!(!private.exists());
            assert_eq!(
                fs::read(recreated).unwrap(),
                media,
                "export survives deleting history"
            );
        }
    }

    #[test]
    fn save_uses_requested_format_keeps_lossless_history_and_reuses_existing_export() {
        for (format, signature, extension) in [
            (ScreenshotFormat::Png, image::ImageFormat::Png, "png"),
            (ScreenshotFormat::Jpeg, image::ImageFormat::Jpeg, "jpg"),
            (ScreenshotFormat::Webp, image::ImageFormat::WebP, "webp"),
        ] {
            let root = tempfile::tempdir().unwrap();
            let output = tempfile::tempdir().unwrap();
            let pixels = RgbaImage::from_fn(48, 24, |x, _| {
                if x < 24 {
                    image::Rgba([255, 0, 0, 128])
                } else {
                    image::Rgba([0, 255, 0, 255])
                }
            });
            let artifact = persist_screenshot(root.path(), &pixels, CaptureMode::Display).unwrap();
            let Response::Saved { path, .. } =
                save_screenshot(root.path(), &artifact.entry.id, output.path(), format).unwrap()
            else {
                panic!("saved response")
            };
            assert_eq!(path.extension().unwrap(), extension);
            assert_eq!(
                image::guess_format(&fs::read(&path).unwrap()).unwrap(),
                signature
            );
            let decoded = image::open(&path).unwrap().into_rgba8();
            if format == ScreenshotFormat::Jpeg {
                // Independently computed alpha-over-white, away from the color boundary.
                for (x, expected) in [(8, [255, 127, 127, 255]), (40, [0, 255, 0, 255])] {
                    for (actual, expected) in decoded.get_pixel(x, 12).0.into_iter().zip(expected) {
                        assert!(actual.abs_diff(expected) <= 2, "{actual} != {expected}");
                    }
                }
            } else {
                assert_eq!(decoded, pixels);
            }
            assert_eq!(
                image::open(&artifact.image_path).unwrap().into_rgba8(),
                pixels
            );
            let other_directory = output.path().join("should-not-be-created");
            let Response::Saved { path: second, .. } = save_screenshot(
                root.path(),
                &artifact.entry.id,
                &other_directory,
                ScreenshotFormat::Png,
            )
            .unwrap() else {
                panic!("second save")
            };
            assert_eq!(path, second);
            assert!(!other_directory.exists());
            fs::remove_file(&path).unwrap();
            let Response::Saved {
                path: recreated, ..
            } = save_screenshot(root.path(), &artifact.entry.id, output.path(), format).unwrap()
            else {
                panic!("recreated save")
            };
            assert!(recreated.is_file());
        }
    }

    #[test]
    fn capture_save_reopen_delete_keeps_export_and_pixels() {
        let data = tempfile::tempdir().unwrap();
        let exports = tempfile::tempdir().unwrap();
        let image = RgbaImage::from_fn(7, 3, |x, y| {
            image::Rgba([x as u8 * 23, y as u8 * 91, 42, 255])
        });
        let item = persist_screenshot(data.path(), &image, CaptureMode::Display).unwrap();
        assert_eq!(list(data.path()).unwrap()[0].entry.width, 7);
        let Response::Saved { path, .. } = save_screenshot(
            data.path(),
            &item.entry.id,
            exports.path(),
            ScreenshotFormat::Png,
        )
        .unwrap() else {
            panic!("save result")
        };
        assert_eq!(image::open(&path).unwrap().to_rgba8(), image);
        assert_eq!(
            list(data.path()).unwrap()[0].entry.saved_path.as_deref(),
            path.to_str()
        );
        execute(Request::Delete {
            root: data.path().into(),
            id: item.entry.id,
        })
        .unwrap();
        assert!(list(data.path()).unwrap().is_empty());
        assert!(path.is_file());
    }

    #[test]
    fn failed_export_preserves_unsaved_history_for_retry() {
        let data = tempfile::tempdir().unwrap();
        let item =
            persist_screenshot(data.path(), &RgbaImage::new(3, 5), CaptureMode::Display).unwrap();
        let obstruction = data.path().join("not-a-directory");
        fs::write(&obstruction, b"keep").unwrap();
        assert!(
            save_screenshot(
                data.path(),
                &item.entry.id,
                &obstruction,
                ScreenshotFormat::Png
            )
            .is_err()
        );
        assert!(list(data.path()).unwrap()[0].entry.saved_path.is_none());
        assert!(item.image_path.is_file());
        assert_eq!(fs::read(obstruction).unwrap(), b"keep");
    }

    #[test]
    fn clear_history_removes_all_kinds_but_keeps_exports_recovery_and_other_roots() {
        let data = tempfile::tempdir().unwrap();
        let exports = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let pixels = RgbaImage::from_fn(7, 3, |x, y| {
            image::Rgba([x as u8 * 23, y as u8 * 91, 42, 255])
        });
        let first = persist_screenshot(data.path(), &pixels, CaptureMode::Window).unwrap();
        let second = persist_screenshot(data.path(), &pixels, CaptureMode::Region).unwrap();
        let untouched = persist_screenshot(other.path(), &pixels, CaptureMode::Display).unwrap();
        let Response::Saved { path, .. } = save_screenshot(
            data.path(),
            &first.entry.id,
            exports.path(),
            ScreenshotFormat::Png,
        )
        .unwrap() else {
            panic!("saved response")
        };
        // Both referenced exports and history-owned recording copies are visible.
        // Clearing history removes only the copies inside this history root.
        let media = exports.path().join("keep.mp4");
        fs::write(&media, b"recording fixture").unwrap();
        let mut recording = first.entry.clone();
        recording.id = uuid::Uuid::new_v4().to_string();
        recording.kind = ArtifactKind::Video;
        recording.saved_path = Some(media.to_string_lossy().into());
        recording.mime_type = Some("video/mp4".into());
        recording.duration_ms = Some(1000);
        recording.target = Some(
            serde_json::from_value(serde_json::json!({
                "type": "display", "display_id": "fixture",
            }))
            .unwrap(),
        );
        captures_history::save_recording_reference(data.path(), &recording, b"preview").unwrap();
        let gif_source = exports.path().join("keep.gif");
        fs::write(&gif_source, b"GIF fixture").unwrap();
        let mut gif = recording.clone();
        gif.id = uuid::Uuid::new_v4().to_string();
        gif.kind = ArtifactKind::Gif;
        gif.mime_type = Some("image/gif".into());
        gif.saved_path = Some(gif_source.to_string_lossy().into());
        let gif_copy =
            captures_history::save_recording(data.path(), &gif, b"poster", &gif_source).unwrap();
        assert_eq!(
            captures_history::load(data.path(), Utc::now())
                .unwrap()
                .len(),
            4
        );
        // A non-history file in the root is not permission to recursively remove it.
        fs::write(data.path().join("keep.txt"), b"keep").unwrap();
        let recovery = data.path().join(".recovery");
        fs::create_dir(&recovery).unwrap();
        fs::write(recovery.join("segment.mp4"), b"unfinished take").unwrap();
        let request = serde_json::from_value(serde_json::json!({
            "operation": "clear_history", "root": data.path(),
        }))
        .unwrap();
        let Response::History { artifacts } = execute(request).unwrap() else {
            panic!("refreshed history response")
        };
        assert!(artifacts.is_empty());
        assert!(!first.image_path.parent().unwrap().exists());
        assert!(!second.image_path.parent().unwrap().exists());
        assert_eq!(image::open(&path).unwrap().into_rgba8(), pixels);
        assert!(untouched.image_path.is_file());
        let remaining = captures_history::load(data.path(), Utc::now()).unwrap();
        assert!(remaining.is_empty());
        assert!(!data.path().join(&recording.id).exists());
        assert!(!gif_copy.exists());
        assert_eq!(fs::read(media).unwrap(), b"recording fixture");
        assert_eq!(fs::read(gif_source).unwrap(), b"GIF fixture");
        assert_eq!(
            fs::read(recovery.join("segment.mp4")).unwrap(),
            b"unfinished take"
        );
        assert_eq!(fs::read(data.path().join("keep.txt")).unwrap(), b"keep");
        assert!(
            execute(Request::ClearHistory {
                root: data.path().into()
            })
            .is_ok()
        );
    }

    #[test]
    fn clear_history_accepts_missing_roots_but_does_not_report_io_failure_as_success() {
        let data = tempfile::tempdir().unwrap();
        let missing = data.path().join("missing");
        assert!(
            execute(Request::ClearHistory {
                root: missing.clone()
            })
            .is_ok()
        );
        assert!(!missing.exists());
        fs::write(&missing, b"not a history directory").unwrap();
        assert!(
            execute(Request::ClearHistory {
                root: missing.clone()
            })
            .is_err()
        );
        assert_eq!(fs::read(missing).unwrap(), b"not a history directory");
    }
}
