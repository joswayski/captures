//! Host-independent publication of edited screenshots.
//!
//! New copies never clobber files. Confirmed original replacement is restricted
//! to an existing screenshot's saved path and updates that same History entry.
//! Neither output operation changes the editable session or its draft.

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use captures_capture::CaptureMode;
use captures_history::{ArtifactKind, HistoryEntry};
use captures_image::{ExportFormat, ExportOptions, ExportSize};
use chrono::Utc;
use image::RgbaImage;
use serde::{Deserialize, Serialize};

use crate::Artifact;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    InvalidDestination(String),
    #[error("image encoding failed: {0}")]
    Image(String),
    #[error(transparent)]
    History(#[from] captures_history::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// A published export either has its distinct History artifact or carries the
/// recoverable path and warning from a post-publication History failure.
#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SavedExport {
    Saved {
        path: PathBuf,
        artifact: Box<Artifact>,
    },
    SavedWithoutHistory {
        path: PathBuf,
        warning: String,
    },
}

/// Encode edited pixels and publish a new copy without overwriting any file.
///
/// Encoding and file-write failures publish neither the destination nor a
/// History entry. Once the destination is published, a History failure is
/// returned as [`SavedExport::SavedWithoutHistory`] so callers can offer the
/// saved file rather than incorrectly reporting a total failure.
pub fn save_new_export(
    history_root: &Path,
    image: &RgbaImage,
    destination: &Path,
    options: ExportOptions,
    mode: CaptureMode,
) -> Result<SavedExport, Error> {
    let parent = validate_destination(destination, options.format)?;
    let resized = captures_image::resize_for_export(image, options.size).map_err(Error::Image)?;
    let image = resized.as_ref();
    // The export and its History pixels share the same once-resized frame.
    let options = ExportOptions {
        size: ExportSize::Original,
        ..options
    };
    let output = captures_image::encode_export(image, options).map_err(Error::Image)?;
    let history_png = captures_history::encode_png(image)?;
    let preview_png = captures_history::encode_thumbnail_png(image)?;

    fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(&output)?;
    temporary.as_file().sync_all()?;
    temporary
        .persist_noclobber(destination)
        .map_err(|error| error.error)?;

    let entry = HistoryEntry {
        id: uuid::Uuid::new_v4().to_string(),
        kind: ArtifactKind::Screenshot,
        preview_url: String::new(),
        full_url: String::new(),
        width: image.width(),
        height: image.height(),
        size_bytes: u64::try_from(output.len()).unwrap_or(u64::MAX),
        created_at: Utc::now().to_rfc3339(),
        mode: Some(mode),
        saved_path: Some(destination.to_string_lossy().into_owned()),
        mime_type: Some(mime_type(options.format).to_owned()),
        duration_ms: None,
        target: None,
        has_system_audio: false,
        has_microphone_audio: false,
        dropped_frames: 0,
    };
    if let Err(error) =
        captures_history::save_capture(history_root, &entry, &history_png, &preview_png)
    {
        return Ok(SavedExport::SavedWithoutHistory {
            path: destination.to_owned(),
            warning: error.to_string(),
        });
    }

    let directory = captures_history::entry_directory(history_root, &entry.id)?;
    Ok(SavedExport::Saved {
        path: destination.to_owned(),
        artifact: Box::new(Artifact {
            image_path: directory.join(captures_history::HISTORY_IMAGE_FILE),
            preview_path: directory.join(captures_history::HISTORY_PREVIEW_FILE),
            entry,
        }),
    })
}

/// Atomically replace the saved file belonging to one screenshot and replace
/// that same History entry. Validation is repeated from disk immediately before
/// encoding so a stale editor session cannot clobber an unrelated path.
pub fn save_original_export(
    history_root: &Path,
    artifact_id: &str,
    destination: &Path,
    image: &RgbaImage,
    options: ExportOptions,
) -> Result<SavedExport, Error> {
    validate_destination(destination, options.format)?;
    let directory = captures_history::entry_directory(history_root, artifact_id)?;
    let metadata = fs::read(directory.join(captures_history::HISTORY_METADATA_FILE))?;
    let mut entry: HistoryEntry = serde_json::from_slice(&metadata)
        .map_err(|error| Error::History(captures_history::Error::Json(error)))?;
    if entry.id != artifact_id
        || entry.kind != ArtifactKind::Screenshot
        || entry.saved_path.as_deref().map(Path::new) != Some(destination)
    {
        return Err(Error::InvalidDestination(
            "The original screenshot History entry changed; reopen the editor before replacing it."
                .to_owned(),
        ));
    }
    if !destination.is_file() {
        return Err(Error::InvalidDestination(
            "The original saved screenshot is no longer available.".to_owned(),
        ));
    }

    let resized = captures_image::resize_for_export(image, options.size).map_err(Error::Image)?;
    let image = resized.as_ref();
    let options = ExportOptions {
        size: ExportSize::Original,
        ..options
    };
    let output = captures_image::encode_export(image, options).map_err(Error::Image)?;
    let history_png = captures_history::encode_png(image)?;
    let preview_png = captures_history::encode_thumbnail_png(image)?;

    let parent = destination.parent().expect("validated destination parent");
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(&output)?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(destination)
        .map_err(|error| error.error)?;

    entry.width = image.width();
    entry.height = image.height();
    entry.size_bytes = u64::try_from(output.len()).unwrap_or(u64::MAX);
    entry.mime_type = Some(mime_type(options.format).to_owned());
    if let Err(error) =
        captures_history::save_capture(history_root, &entry, &history_png, &preview_png)
    {
        return Ok(SavedExport::SavedWithoutHistory {
            path: destination.to_owned(),
            warning: error.to_string(),
        });
    }
    Ok(SavedExport::Saved {
        path: destination.to_owned(),
        artifact: Box::new(Artifact {
            image_path: directory.join(captures_history::HISTORY_IMAGE_FILE),
            preview_path: directory.join(captures_history::HISTORY_PREVIEW_FILE),
            entry,
        }),
    })
}

fn validate_destination(destination: &Path, format: ExportFormat) -> Result<&Path, Error> {
    if destination.as_os_str().is_empty() || destination.file_name().is_none() {
        return Err(Error::InvalidDestination(
            "choose a file name for the edited screenshot".to_owned(),
        ));
    }
    let Some(extension) = destination.extension().and_then(|value| value.to_str()) else {
        return Err(Error::InvalidDestination(format!(
            "the file name must end in .{}",
            canonical_extension(format)
        )));
    };
    if !extension_matches(format, extension) {
        return Err(Error::InvalidDestination(format!(
            "the file extension does not match the selected {} format",
            canonical_extension(format).to_uppercase()
        )));
    }
    destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| {
            Error::InvalidDestination(
                "choose a destination folder for the edited screenshot".to_owned(),
            )
        })
}

const fn canonical_extension(format: ExportFormat) -> &'static str {
    match format {
        ExportFormat::Png => "png",
        ExportFormat::Jpeg => "jpg",
        ExportFormat::Webp => "webp",
    }
}

pub(crate) fn extension_matches(format: ExportFormat, extension: &str) -> bool {
    match format {
        ExportFormat::Png => extension.eq_ignore_ascii_case("png"),
        ExportFormat::Jpeg => {
            extension.eq_ignore_ascii_case("jpg") || extension.eq_ignore_ascii_case("jpeg")
        }
        ExportFormat::Webp => extension.eq_ignore_ascii_case("webp"),
    }
}

const fn mime_type(format: ExportFormat) -> &'static str {
    match format {
        ExportFormat::Png => "image/png",
        ExportFormat::Jpeg => "image/jpeg",
        ExportFormat::Webp => "image/webp",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use captures_image::{ExportQuality, PngOptions};
    use image::{ImageFormat, Rgba};

    fn pixels() -> RgbaImage {
        RgbaImage::from_fn(11, 7, |x, y| {
            Rgba([
                (x * 19 + y * 3) as u8,
                (x * 7 + y * 29) as u8,
                (255 - x * 13 - y * 5) as u8,
                if (x + y) % 3 == 0 { 83 } else { 255 },
            ])
        })
    }

    fn options(format: ExportFormat) -> ExportOptions {
        ExportOptions {
            format,
            quality: ExportQuality::Preserve,
            quality_value: 100,
            max_size_bytes: None,
            png: PngOptions::default(),
            size: ExportSize::Original,
        }
    }

    fn saved_screenshot(root: &Path, destination: &Path) -> Artifact {
        let image = RgbaImage::from_pixel(8, 6, Rgba([3, 7, 11, 255]));
        fs::write(destination, captures_history::encode_png(&image).unwrap()).unwrap();
        let mut artifact = crate::persist_screenshot(root, &image, CaptureMode::Display).unwrap();
        artifact.entry.saved_path = Some(destination.to_string_lossy().into_owned());
        captures_history::update_metadata(root, &artifact.entry).unwrap();
        artifact
    }

    #[test]
    fn overwrite_resizes_once_and_preserves_history_identity() {
        let data = tempfile::tempdir().unwrap();
        let destination = data.path().join("original.png");
        let original = saved_screenshot(data.path(), &destination);
        let edited = RgbaImage::from_pixel(10, 8, Rgba([91, 43, 17, 255]));
        let SavedExport::Saved { artifact, path } = save_original_export(
            data.path(),
            &original.entry.id,
            &destination,
            &edited,
            ExportOptions {
                size: ExportSize::Percent { percent: 50 },
                ..options(ExportFormat::Png)
            },
        )
        .unwrap() else {
            panic!("History replacement should succeed")
        };

        assert_eq!(path, destination);
        assert_eq!(artifact.entry.id, original.entry.id);
        assert_eq!(artifact.entry.created_at, original.entry.created_at);
        assert_eq!(artifact.entry.mode, original.entry.mode);
        assert_eq!((artifact.entry.width, artifact.entry.height), (5, 4));
        let output = fs::read(&destination).unwrap();
        assert_eq!(artifact.entry.size_bytes, output.len() as u64);
        let expected = RgbaImage::from_pixel(5, 4, Rgba([91, 43, 17, 255]));
        assert_eq!(
            image::load_from_memory(&output).unwrap().to_rgba8(),
            expected
        );
        assert_eq!(
            image::open(&artifact.image_path).unwrap().to_rgba8(),
            expected
        );
        assert_eq!(
            image::open(&artifact.preview_path).unwrap().to_rgba8(),
            expected
        );
        let entries = captures_history::load(data.path(), Utc::now()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, original.entry.id);
        assert_eq!(edited.dimensions(), (10, 8));
    }

    #[test]
    fn overwrite_rejects_stale_missing_and_mismatched_sources_without_clobbering() {
        let data = tempfile::tempdir().unwrap();
        let destination = data.path().join("original.png");
        let artifact = saved_screenshot(data.path(), &destination);
        let original_bytes = fs::read(&destination).unwrap();
        let other = data.path().join("other.png");
        fs::write(&other, b"other").unwrap();

        for (id, target, format) in [
            (
                uuid::Uuid::new_v4().to_string(),
                destination.clone(),
                ExportFormat::Png,
            ),
            (artifact.entry.id.clone(), other.clone(), ExportFormat::Png),
            (
                artifact.entry.id.clone(),
                destination.clone(),
                ExportFormat::Jpeg,
            ),
        ] {
            assert!(
                save_original_export(data.path(), &id, &target, &pixels(), options(format))
                    .is_err()
            );
            assert_eq!(fs::read(&destination).unwrap(), original_bytes);
            assert_eq!(fs::read(&other).unwrap(), b"other");
        }

        fs::remove_file(&destination).unwrap();
        assert!(
            save_original_export(
                data.path(),
                &artifact.entry.id,
                &destination,
                &pixels(),
                options(ExportFormat::Png)
            )
            .is_err()
        );
        assert!(!destination.exists());

        let mut stale = artifact.entry;
        stale.kind = ArtifactKind::Video;
        captures_history::update_metadata(data.path(), &stale).unwrap();
        fs::write(&destination, &original_bytes).unwrap();
        assert!(
            save_original_export(
                data.path(),
                &stale.id,
                &destination,
                &pixels(),
                options(ExportFormat::Png)
            )
            .is_err()
        );
        assert_eq!(fs::read(destination).unwrap(), original_bytes);
    }

    #[cfg(unix)]
    #[test]
    fn overwrite_reports_history_failure_after_publishing_file() {
        use std::os::unix::fs::PermissionsExt;

        let history = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap();
        let destination = output.path().join("original.png");
        let artifact = saved_screenshot(history.path(), &destination);
        fs::set_permissions(history.path(), fs::Permissions::from_mode(0o555)).unwrap();

        let result = save_original_export(
            history.path(),
            &artifact.entry.id,
            &destination,
            &pixels(),
            options(ExportFormat::Png),
        );
        fs::set_permissions(history.path(), fs::Permissions::from_mode(0o755)).unwrap();
        let SavedExport::SavedWithoutHistory { path, warning } = result.unwrap() else {
            panic!("file publication must remain a recoverable success")
        };
        assert_eq!(path, destination);
        assert!(!warning.is_empty());
        assert_eq!(image::open(path).unwrap().to_rgba8(), pixels());
        let retained = captures_history::load(history.path(), Utc::now()).unwrap();
        assert_eq!(retained[0].id, artifact.entry.id);
        assert_eq!(retained[0].width, artifact.entry.width);
        assert_eq!(retained[0].size_bytes, artifact.entry.size_bytes);
    }

    #[test]
    fn resized_publication_uses_one_frame_for_file_history_and_thumbnail() {
        let data = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap();
        let image = RgbaImage::from_pixel(5, 8, Rgba([19, 71, 193, 255]));
        let destination = output.path().join("half.png");
        let SavedExport::Saved { artifact, .. } = save_new_export(
            data.path(),
            &image,
            &destination,
            ExportOptions {
                size: ExportSize::Percent { percent: 50 },
                ..options(ExportFormat::Png)
            },
            CaptureMode::Region,
        )
        .unwrap() else {
            panic!("missing History item")
        };
        assert_eq!((artifact.entry.width, artifact.entry.height), (3, 5));
        let expected = RgbaImage::from_pixel(3, 5, Rgba([19, 71, 193, 255]));
        for path in [&destination, &artifact.image_path, &artifact.preview_path] {
            assert_eq!(image::open(path).unwrap().to_rgba8(), expected);
        }
        assert_eq!(image.dimensions(), (5, 8));
        let invalid = output.path().join("absent/oversized.png");
        assert!(
            save_new_export(
                data.path(),
                &image,
                &invalid,
                ExportOptions {
                    size: ExportSize::Custom {
                        width: 10_001,
                        height: 10_000
                    },
                    ..options(ExportFormat::Png)
                },
                CaptureMode::Region
            )
            .is_err()
        );
        assert!(!invalid.parent().unwrap().exists());
        assert_eq!(
            captures_history::load(data.path(), Utc::now())
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn publishes_each_format_with_export_metadata_and_lossless_history_pixels() {
        for (format, extension, encoded_format, mime) in [
            (ExportFormat::Png, "png", ImageFormat::Png, "image/png"),
            (ExportFormat::Jpeg, "jpeg", ImageFormat::Jpeg, "image/jpeg"),
            (ExportFormat::Webp, "webp", ImageFormat::WebP, "image/webp"),
        ] {
            let data = tempfile::tempdir().unwrap();
            let output = tempfile::tempdir().unwrap();
            let image = pixels();
            let destination = output.path().join(format!("edited.{extension}"));
            let expected_export = captures_image::encode_export(&image, options(format)).unwrap();

            let SavedExport::Saved { path, artifact } = save_new_export(
                data.path(),
                &image,
                &destination,
                options(format),
                CaptureMode::Region,
            )
            .unwrap() else {
                panic!("history should be available")
            };

            assert_eq!(path, destination);
            assert_eq!(fs::read(&path).unwrap(), expected_export);
            assert_eq!(
                image::guess_format(&expected_export).unwrap(),
                encoded_format
            );
            assert_eq!(artifact.entry.size_bytes, expected_export.len() as u64);
            assert_eq!(artifact.entry.saved_path.as_deref(), path.to_str());
            assert_eq!(artifact.entry.mime_type.as_deref(), Some(mime));
            assert_eq!(artifact.entry.mode, Some(CaptureMode::Region));
            assert_eq!((artifact.entry.width, artifact.entry.height), (11, 7));
            uuid::Uuid::parse_str(&artifact.entry.id).unwrap();
            assert_eq!(
                fs::read(&artifact.image_path).unwrap(),
                captures_history::encode_png(&image).unwrap()
            );
            assert_eq!(image::open(&artifact.image_path).unwrap().to_rgba8(), image);
            assert_eq!(
                image::open(&artifact.preview_path).unwrap().to_rgba8(),
                image
            );
            assert_eq!(
                captures_history::load(data.path(), Utc::now())
                    .unwrap()
                    .into_iter()
                    .map(|entry| entry.id)
                    .collect::<Vec<_>>(),
                vec![artifact.entry.id]
            );
        }
    }

    #[test]
    fn creates_a_distinct_artifact_without_touching_the_original() {
        let data = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap();
        let original_pixels = RgbaImage::from_pixel(4, 3, Rgba([9, 17, 31, 255]));
        let original =
            crate::persist_screenshot(data.path(), &original_pixels, CaptureMode::Display).unwrap();
        let original_image = fs::read(&original.image_path).unwrap();
        let edited = pixels();
        let destination = output.path().join("new-copy.png");

        let SavedExport::Saved { artifact, .. } = save_new_export(
            data.path(),
            &edited,
            &destination,
            options(ExportFormat::Png),
            CaptureMode::Window,
        )
        .unwrap() else {
            panic!("history should be available")
        };

        assert_ne!(artifact.entry.id, original.entry.id);
        assert_eq!(fs::read(&original.image_path).unwrap(), original_image);
        assert_eq!(
            image::open(&original.image_path).unwrap().to_rgba8(),
            original_pixels
        );
        let entries = captures_history::load(data.path(), Utc::now()).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(
            entries
                .iter()
                .find(|entry| entry.id == original.entry.id)
                .unwrap()
                .saved_path
                .is_none()
        );
    }

    #[test]
    fn existing_destination_is_never_overwritten_or_added_to_history() {
        let data = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap();
        let destination = output.path().join("occupied.png");
        fs::write(&destination, b"keep the original").unwrap();

        assert!(matches!(
            save_new_export(
                data.path(),
                &pixels(),
                &destination,
                options(ExportFormat::Png),
                CaptureMode::Region,
            ),
            Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::AlreadyExists
        ));
        assert_eq!(fs::read(&destination).unwrap(), b"keep the original");
        assert!(
            captures_history::load(data.path(), Utc::now())
                .unwrap()
                .is_empty()
        );
        assert_eq!(fs::read_dir(output.path()).unwrap().count(), 1);
    }

    #[test]
    fn validates_folder_filename_and_shipping_format_extensions() {
        let data = tempfile::tempdir().unwrap();
        let image = pixels();
        for (destination, format, expected) in [
            (
                Path::new(""),
                ExportFormat::Png,
                "choose a file name for the edited screenshot",
            ),
            (
                Path::new("/"),
                ExportFormat::Png,
                "choose a file name for the edited screenshot",
            ),
            (
                Path::new("/tmp/edit"),
                ExportFormat::Png,
                "the file name must end in .png",
            ),
            (
                Path::new("/tmp/edit.png"),
                ExportFormat::Jpeg,
                "the file extension does not match the selected JPG format",
            ),
            (
                Path::new("edit.png"),
                ExportFormat::Png,
                "choose a destination folder for the edited screenshot",
            ),
        ] {
            let error = save_new_export(
                data.path(),
                &image,
                destination,
                options(format),
                CaptureMode::Region,
            )
            .unwrap_err();
            assert!(matches!(&error, Error::InvalidDestination(_)));
            assert_eq!(error.to_string(), expected);
        }
        assert!(!data.path().exists() || fs::read_dir(data.path()).unwrap().next().is_none());

        let output = tempfile::tempdir().unwrap();
        for (name, format) in [
            ("edit.PNG", ExportFormat::Png),
            ("edit.JPG", ExportFormat::Jpeg),
            ("edit.jpeg", ExportFormat::Jpeg),
            ("edit.WeBp", ExportFormat::Webp),
        ] {
            let destination = output.path().join(name);
            let result = save_new_export(
                data.path(),
                &image,
                &destination,
                options(format),
                CaptureMode::Region,
            )
            .unwrap();
            assert!(matches!(result, SavedExport::Saved { .. }));
        }
    }

    #[test]
    fn encode_and_io_failures_leave_no_export_or_history_entry() {
        let data = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap();
        let invalid_destination = output.path().join("invalid.png");
        assert!(matches!(
            save_new_export(
                data.path(),
                &RgbaImage::new(0, 1),
                &invalid_destination,
                options(ExportFormat::Png),
                CaptureMode::Region,
            ),
            Err(Error::Image(_))
        ));
        assert!(!invalid_destination.exists());

        let blocked_parent = output.path().join("not-a-directory");
        fs::write(&blocked_parent, b"keep").unwrap();
        let blocked_destination = blocked_parent.join("edited.png");
        assert!(matches!(
            save_new_export(
                data.path(),
                &pixels(),
                &blocked_destination,
                options(ExportFormat::Png),
                CaptureMode::Region,
            ),
            Err(Error::Io(_))
        ));
        assert_eq!(fs::read(&blocked_parent).unwrap(), b"keep");
        assert!(!blocked_destination.exists());
        assert!(
            captures_history::load(data.path(), Utc::now())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn history_failure_reports_the_published_path_and_warning() {
        let data = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap();
        let blocked_history = data.path().join("history-is-a-file");
        fs::write(&blocked_history, b"keep").unwrap();
        let destination = output.path().join("recovered.webp");
        let image = pixels();

        let SavedExport::SavedWithoutHistory { path, warning } = save_new_export(
            &blocked_history,
            &image,
            &destination,
            options(ExportFormat::Webp),
            CaptureMode::Window,
        )
        .unwrap() else {
            panic!("published export should report partial success")
        };

        assert_eq!(path, destination);
        assert!(path.is_file());
        assert_eq!(
            fs::read(&path).unwrap(),
            captures_image::encode_export(&image, options(ExportFormat::Webp)).unwrap()
        );
        assert!(!warning.is_empty());
        assert_eq!(fs::read(blocked_history).unwrap(), b"keep");
        assert_eq!(fs::read_dir(output.path()).unwrap().count(), 1);
    }

    #[test]
    fn saved_export_uses_a_stable_tagged_shape() {
        assert_eq!(
            serde_json::to_value(SavedExport::SavedWithoutHistory {
                path: PathBuf::from("edited.png"),
                warning: "history unavailable".to_owned(),
            })
            .unwrap(),
            serde_json::json!({
                "status": "saved_without_history",
                "path": "edited.png",
                "warning": "history unavailable",
            })
        );
    }
}
