//! Shared native application operations. Hosts schedule these off the UI thread.
//! Images stay in owned files, never JSON/base64. No browser or host window APIs.

pub mod capture_flow;
pub mod selection;

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
    Delete {
        root: PathBuf,
        id: String,
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
                artifact: persist_screenshot(&root, &frame.image)?,
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
        Request::Delete { root, id } => {
            captures_history::delete(&root, &id)?;
            Ok(Response::Deleted { id })
        }
    }
}

fn artifact(root: &Path, entry: HistoryEntry) -> Result<Artifact, Error> {
    let directory = captures_history::entry_directory(root, &entry.id)?;
    Ok(Artifact {
        entry,
        image_path: directory.join(captures_history::HISTORY_IMAGE_FILE),
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
pub fn persist_screenshot(root: &Path, image: &RgbaImage) -> Result<Artifact, Error> {
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
        mode: Some(CaptureMode::Display),
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
    let mut item = list(root)?
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
    let stem = format!("Captures_{}", Local::now().format("%Y-%m-%d_%H-%M-%S_%3f"));
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
    temporary.as_file().sync_all()?;
    let extension = format.extension();
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
            let artifact = persist_screenshot(root.path(), &pixels).unwrap();
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
        let item = persist_screenshot(data.path(), &image).unwrap();
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
        let item = persist_screenshot(data.path(), &RgbaImage::new(3, 5)).unwrap();
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
}
