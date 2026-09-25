//! Files handed to the OS must outlive a drag, even if the preview or History disappears.
//! Hosts call `prepare` on a worker and `clear_previous_exports` once at startup.
use std::{
    fs,
    path::{Path, PathBuf},
};

use captures_history::ArtifactKind;

use crate::Error;

// History's loader owns and prunes ordinary child directories; dot-prefixed
// directories are explicitly excluded from that scan.
const EXPORTS: &str = ".preview-drag-exports";

/// Remove only our disposable exports at startup, never a saved user file.
pub fn clear_previous_exports(root: &Path) -> Result<(), Error> {
    let directory = root.join(EXPORTS);
    if directory.exists() {
        fs::remove_dir_all(directory)?;
    }
    Ok(())
}

/// Resolve the exact persisted artifact, not an image shown by the current card.
/// A unique directory prevents simultaneous drags from replacing one another's bytes.
pub fn prepare(root: &Path, id: &str) -> Result<PathBuf, Error> {
    let entry = captures_history::load(root, chrono::Utc::now())?
        .into_iter()
        .find(|entry| entry.id == id)
        .ok_or(Error::Missing)?;
    if let Some(path) = entry
        .saved_path
        .as_deref()
        .map(Path::new)
        .filter(|p| p.is_file())
    {
        return Ok(fs::canonicalize(path)?);
    }
    let source = if entry.kind == ArtifactKind::Screenshot {
        captures_history::entry_directory(root, id)?.join(captures_history::HISTORY_IMAGE_FILE)
    } else {
        entry.recording_media_path(root).ok_or(Error::Missing)?
    };
    if !source.is_file() {
        return Err(Error::Missing);
    }
    let extension = if entry.kind == ArtifactKind::Screenshot {
        "png"
    } else {
        source
            .extension()
            .and_then(|e| e.to_str())
            .ok_or(Error::Missing)?
    };
    let destination = root.join(EXPORTS).join(uuid::Uuid::new_v4().to_string());
    fs::create_dir_all(&destination)?;
    let path = destination.join(format!("Captures_{}.{}", id, extension));
    fs::copy(source, &path)?;
    Ok(fs::canonicalize(path)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use captures_capture::CaptureMode;
    use captures_history::HistoryEntry;
    use captures_recording::RecordingTarget;

    fn entry(id: &str, kind: ArtifactKind, saved_path: Option<String>) -> HistoryEntry {
        HistoryEntry {
            id: id.into(),
            kind,
            saved_path,
            preview_url: String::new(),
            full_url: String::new(),
            width: 3,
            height: 2,
            size_bytes: 0,
            created_at: chrono::Utc::now().to_rfc3339(),
            mode: (kind == ArtifactKind::Screenshot).then_some(CaptureMode::Region),
            mime_type: (kind != ArtifactKind::Screenshot).then_some("video/mp4".into()),
            duration_ms: (kind != ArtifactKind::Screenshot).then_some(10),
            target: (kind != ArtifactKind::Screenshot).then_some(RecordingTarget::Display {
                display_id: "1".into(),
            }),
            has_system_audio: false,
            has_microphone_audio: false,
            dropped_frames: 0,
        }
    }

    #[test]
    fn retained_screenshot_survives_source_deletion_until_next_startup() {
        let root = tempfile::tempdir().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let item = entry(&id, ArtifactKind::Screenshot, None);
        captures_history::save_capture(root.path(), &item, b"full image", b"small preview")
            .unwrap();
        let first = prepare(root.path(), &id).unwrap();
        let second = prepare(root.path(), &id).unwrap();
        assert_ne!(first, second);
        captures_history::delete(root.path(), &id).unwrap();
        assert_eq!(fs::read(&first).unwrap(), b"full image");
        assert_eq!(fs::read(&second).unwrap(), b"full image");
        assert!(matches!(prepare(root.path(), &id), Err(Error::Missing)));
        clear_previous_exports(root.path()).unwrap();
        assert!(!first.exists());
    }

    #[test]
    fn saved_file_wins_and_is_never_cleaned_up() {
        let root = tempfile::tempdir().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let saved = root.path().join("Café.gif");
        fs::write(&saved, b"moving GIF").unwrap();
        let item = entry(
            &id,
            ArtifactKind::Gif,
            Some(saved.to_string_lossy().into_owned()),
        );
        captures_history::save_entry(root.path(), &item, None, b"poster PNG", None).unwrap();
        // Windows resolves the temporary directory's short name and adds the
        // verbatim prefix. Compare file identity in the same canonical form.
        assert_eq!(
            prepare(root.path(), &id).unwrap(),
            fs::canonicalize(&saved).unwrap()
        );
        clear_previous_exports(root.path()).unwrap();
        assert_eq!(fs::read(saved).unwrap(), b"moving GIF");
    }

    #[test]
    fn recording_recovery_is_media_not_poster() {
        let root = tempfile::tempdir().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let saved = root.path().join("original.mp4");
        fs::write(&saved, b"video frames").unwrap();
        let item = entry(
            &id,
            ArtifactKind::Video,
            Some(saved.to_string_lossy().into_owned()),
        );
        captures_history::save_entry(
            root.path(),
            &item,
            None,
            b"poster PNG",
            Some((&saved, "media.mp4")),
        )
        .unwrap();
        fs::remove_file(&saved).unwrap();
        let dragged = prepare(root.path(), &id).unwrap();
        assert_eq!(dragged.extension().unwrap(), "mp4");
        assert_eq!(fs::read(&dragged).unwrap(), b"video frames");
    }
}
