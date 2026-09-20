//! Host-independent capture history persistence and basic PNG encoding.

pub mod editor_draft;

use std::{
    fs,
    path::{Path, PathBuf},
};

use captures_capture::CaptureMode;
use captures_recording::{RecordingKind, RecordingTarget};
use chrono::{DateTime, Duration, Utc};
use image::{
    RgbaImage,
    codecs::png::{CompressionType, FilterType},
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const HISTORY_RETENTION_DAYS: i64 = 30;
pub const HISTORY_IMAGE_FILE: &str = "capture.png";
pub const HISTORY_PREVIEW_FILE: &str = "preview.png";
pub const HISTORY_METADATA_FILE: &str = "metadata.json";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("capture history is unavailable")]
    HistoryUnavailable,
    #[error("{0}")]
    Invalid(String),
    #[error("image encoding failed: {0}")]
    Image(String),
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    #[default]
    Screenshot,
    Video,
    Gif,
}

impl ArtifactKind {
    pub const fn is_recording(self) -> bool {
        !matches!(self, Self::Screenshot)
    }
    pub const fn recording_kind(self) -> Option<RecordingKind> {
        match self {
            Self::Screenshot => None,
            Self::Video => Some(RecordingKind::Video),
            Self::Gif => Some(RecordingKind::Gif),
        }
    }
}
impl From<RecordingKind> for ArtifactKind {
    fn from(value: RecordingKind) -> Self {
        match value {
            RecordingKind::Video => Self::Video,
            RecordingKind::Gif => Self::Gif,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct HistoryEntry {
    pub id: String,
    #[serde(default)]
    pub kind: ArtifactKind,
    pub preview_url: String,
    pub full_url: String,
    pub width: u32,
    pub height: u32,
    pub size_bytes: u64,
    pub created_at: String,
    #[serde(default)]
    pub mode: Option<CaptureMode>,
    #[serde(default)]
    pub saved_path: Option<String>,
    #[serde(default)]
    pub mime_type: Option<String>,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub target: Option<RecordingTarget>,
    #[serde(default)]
    pub has_system_audio: bool,
    #[serde(default)]
    pub has_microphone_audio: bool,
    #[serde(default)]
    pub dropped_frames: u64,
}

impl HistoryEntry {
    /// Prefer recovery media under `root`; retain the legacy saved-path and
    /// prospective default-name fallbacks used by existing hosts.
    pub fn recording_media_path(&self, root: &Path) -> Option<PathBuf> {
        if !self.kind.is_recording() || Uuid::parse_str(&self.id).is_err() {
            return None;
        }
        let directory = root.join(&self.id);
        find_recording_media(&directory)
            .or_else(|| self.saved_path.as_ref().map(PathBuf::from))
            .or_else(|| {
                Some(directory.join(if self.kind == ArtifactKind::Gif {
                    "media.gif"
                } else {
                    "media.mp4"
                }))
            })
    }
}

pub fn recording_media_file_name(kind: ArtifactKind, source: &Path) -> Option<String> {
    if !kind.is_recording() {
        return None;
    }
    let extension =
        source
            .extension()
            .and_then(|v| v.to_str())
            .unwrap_or(if kind == ArtifactKind::Gif {
                "gif"
            } else {
                "mp4"
            });
    Some(format!("media.{extension}"))
}

pub fn find_recording_media(directory: &Path) -> Option<PathBuf> {
    ["media.mp4", "media.gif", "media.webm"]
        .iter()
        .map(|n| directory.join(n))
        .find(|p| p.is_file())
        .or_else(|| {
            fs::read_dir(directory)
                .ok()?
                .flatten()
                .map(|e| e.path())
                .find(|p| {
                    p.is_file()
                        && p.file_name()
                            .and_then(|n| n.to_str())
                            .is_some_and(|n| n.starts_with("media."))
                })
        })
}

pub fn entry_directory(root: &Path, id: &str) -> Result<PathBuf, Error> {
    Uuid::parse_str(id).map_err(|_| Error::HistoryUnavailable)?;
    Ok(root.join(id))
}

pub fn save_capture(
    root: &Path,
    entry: &HistoryEntry,
    image: &[u8],
    preview: &[u8],
) -> Result<(), Error> {
    save_entry(root, entry, Some(image), preview, None).map(|_| ())
}

pub fn save_recording(
    root: &Path,
    entry: &HistoryEntry,
    preview: &[u8],
    source: &Path,
) -> Result<PathBuf, Error> {
    if !entry.kind.is_recording() {
        return Err(Error::Invalid(
            "recording history metadata is incomplete".into(),
        ));
    }
    if !source.is_file() {
        return Err(Error::Invalid(
            "recording media is no longer available".into(),
        ));
    }
    let name = recording_media_file_name(entry.kind, source)
        .ok_or_else(|| Error::Invalid("recording history metadata is incomplete".into()))?;
    save_entry(root, entry, None, preview, Some((source, &name)))?
        .ok_or_else(|| Error::Invalid("recording history media was not written".into()))
}

pub fn save_recording_reference(
    root: &Path,
    entry: &HistoryEntry,
    preview: &[u8],
) -> Result<(), Error> {
    if !entry.kind.is_recording() {
        return Err(Error::Invalid(
            "recording history metadata is incomplete".into(),
        ));
    }
    if entry
        .saved_path
        .as_deref()
        .is_none_or(|p| !Path::new(p).is_file())
    {
        return Err(Error::Invalid(
            "recording media is no longer available".into(),
        ));
    }
    save_entry(root, entry, None, preview, None).map(|_| ())
}

pub fn save_entry(
    root: &Path,
    entry: &HistoryEntry,
    image: Option<&[u8]>,
    preview: &[u8],
    media: Option<(&Path, &str)>,
) -> Result<Option<PathBuf>, Error> {
    fs::create_dir_all(root)?;
    let destination = entry_directory(root, &entry.id)?;
    let temporary = root.join(format!(".{}.{}.tmp", entry.id, Uuid::new_v4()));
    let backup = root.join(format!(".{}.{}.bak", entry.id, Uuid::new_v4()));
    fs::create_dir(&temporary)?;
    let result = (|| {
        if let Some(bytes) = image {
            fs::write(temporary.join(HISTORY_IMAGE_FILE), bytes)?;
        }
        fs::write(temporary.join(HISTORY_PREVIEW_FILE), preview)?;
        let recovery = if let Some((source, name)) = media {
            let target = temporary.join(name);
            if source != target {
                fs::copy(source, &target)?;
            }
            Some(target)
        } else if let Some(existing) = find_recording_media(&destination) {
            let target = temporary.join(
                existing
                    .file_name()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| "media.bin".into()),
            );
            fs::copy(existing, &target)?;
            Some(target)
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
                return Err(match fs::rename(&backup, &destination) {
                    Ok(()) => error.into(),
                    Err(rollback) => Error::Invalid(format!(
                        "capture history could not be replaced ({error}), and the previous entry could not be restored ({rollback}); its backup remains at {}",
                        backup.display()
                    )),
                });
            }
            let _ = fs::remove_dir_all(&backup);
        } else {
            fs::rename(&temporary, &destination)?;
        }
        Ok(recovery.map(|p| destination.join(p.file_name().expect("media file name"))))
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(temporary);
    }
    result
}

pub fn update_metadata(root: &Path, entry: &HistoryEntry) -> Result<(), Error> {
    let directory = entry_directory(root, &entry.id)?;
    if !directory.is_dir() {
        return Err(Error::HistoryUnavailable);
    }
    let temporary = directory.join(format!(".{}.metadata.tmp", Uuid::new_v4()));
    fs::write(&temporary, serde_json::to_vec_pretty(entry)?)?;
    fs::rename(temporary, directory.join(HISTORY_METADATA_FILE))?;
    Ok(())
}

pub fn load(root: &Path, now: DateTime<Utc>) -> Result<Vec<HistoryEntry>, Error> {
    let entries = match fs::read_dir(root) {
        Ok(v) => v,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e.into()),
    };
    let cutoff = now - Duration::days(HISTORY_RETENTION_DAYS);
    let mut history = vec![];
    for item in entries.flatten() {
        let path = item.path();
        let name = item.file_name().to_string_lossy().into_owned();
        if !path.is_dir() || name.starts_with('.') {
            continue;
        }
        let parsed = fs::read(path.join(HISTORY_METADATA_FILE))
            .ok()
            .and_then(|b| serde_json::from_slice::<HistoryEntry>(&b).ok());
        let Some(entry) = parsed else {
            let _ = fs::remove_dir_all(path);
            continue;
        };
        let created = DateTime::parse_from_rfc3339(&entry.created_at)
            .ok()
            .map(|v| v.with_timezone(&Utc));
        let Some(created) = created else {
            let _ = fs::remove_dir_all(path);
            continue;
        };
        if entry.id != name || Uuid::parse_str(&entry.id).is_err() || created < cutoff {
            let _ = fs::remove_dir_all(path);
            continue;
        }
        let valid = path.join(HISTORY_PREVIEW_FILE).is_file()
            && match entry.kind {
                ArtifactKind::Screenshot => {
                    entry.mode.is_some() && path.join(HISTORY_IMAGE_FILE).is_file()
                }
                _ => {
                    entry.mime_type.is_some()
                        && entry.duration_ms.is_some()
                        && entry.target.is_some()
                        && (find_recording_media(&path).is_some() || entry.saved_path.is_some())
                }
            };
        if valid {
            history.push((created, entry));
        } else {
            let _ = fs::remove_dir_all(path);
        }
    }
    history.sort_by(|a, b| b.0.cmp(&a.0));
    Ok(history.into_iter().map(|(_, e)| e).collect())
}

pub fn read_images(root: &Path, id: &str) -> Result<(Vec<u8>, Vec<u8>), Error> {
    let d = entry_directory(root, id)?;
    Ok((
        fs::read(d.join(HISTORY_IMAGE_FILE))?,
        fs::read(d.join(HISTORY_PREVIEW_FILE))?,
    ))
}
pub fn read_image(root: &Path, id: &str, preview: bool) -> Result<Vec<u8>, Error> {
    let d = entry_directory(root, id)?;
    Ok(fs::read(d.join(if preview {
        HISTORY_PREVIEW_FILE
    } else {
        HISTORY_IMAGE_FILE
    }))?)
}
pub fn delete(root: &Path, id: &str) -> Result<(), Error> {
    match fs::remove_dir_all(entry_directory(root, id)?) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

pub fn encode_png(image: &RgbaImage) -> Result<Vec<u8>, Error> {
    encode_png_with_quality(image, CompressionType::Fast, FilterType::Sub)
}
pub fn encode_thumbnail_png(image: &RgbaImage) -> Result<Vec<u8>, Error> {
    if image.width() > 568 || image.height() > 320 {
        let scale = (568.0 / f64::from(image.width())).min(320.0 / f64::from(image.height()));
        let output = image::imageops::resize(
            image,
            (f64::from(image.width()) * scale).round().max(1.0) as u32,
            (f64::from(image.height()) * scale).round().max(1.0) as u32,
            image::imageops::FilterType::Triangle,
        );
        return encode_png(&output);
    }
    encode_png(image)
}
pub fn encode_png_with_quality(
    image: &RgbaImage,
    compression: CompressionType,
    filter: FilterType,
) -> Result<Vec<u8>, Error> {
    let mut bytes = vec![];
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
                encoder.set_filter(png::FilterType::Paeth)
            }
            FilterType::Sub => encoder.set_filter(png::FilterType::Sub),
            FilterType::NoFilter => encoder.set_filter(png::FilterType::NoFilter),
            FilterType::Up => encoder.set_filter(png::FilterType::Up),
            FilterType::Avg => encoder.set_filter(png::FilterType::Avg),
            _ => encoder.set_filter(png::FilterType::Paeth),
        };
        mark_png_as_srgb(&mut encoder);
        encoder
            .write_header()
            .map_err(|e| Error::Image(e.to_string()))?
            .write_image_data(image.as_raw())
            .map_err(|e| Error::Image(e.to_string()))?;
    }
    Ok(bytes)
}
pub fn mark_png_as_srgb(encoder: &mut png::Encoder<&mut Vec<u8>>) {
    encoder.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
    encoder.set_source_gamma(png::ScaledFloat::from_scaled(45_455));
    encoder.set_source_chromaticities(png::SourceChromaticities::new(
        (0.3127, 0.3290),
        (0.6400, 0.3300),
        (0.3000, 0.6000),
        (0.1500, 0.0600),
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use captures_recording::RecordingTarget;
    use chrono::TimeZone;
    use tempfile::tempdir;

    fn screenshot(id: &str, created_at: String) -> HistoryEntry {
        HistoryEntry {
            id: id.into(),
            kind: ArtifactKind::Screenshot,
            preview_url: "host-preview".into(),
            full_url: "host-full".into(),
            width: 1,
            height: 1,
            size_bytes: 4,
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
    fn recording(id: &str, created_at: String, saved_path: Option<String>) -> HistoryEntry {
        HistoryEntry {
            id: id.into(),
            kind: ArtifactKind::Video,
            preview_url: "host-poster".into(),
            full_url: "host-media".into(),
            width: 10,
            height: 10,
            size_bytes: 5,
            created_at,
            mode: None,
            saved_path,
            mime_type: Some("video/mp4".into()),
            duration_ms: Some(10),
            target: Some(RecordingTarget::Display {
                display_id: "1".into(),
            }),
            has_system_audio: false,
            has_microphone_audio: false,
            dropped_frames: 0,
        }
    }

    #[test]
    fn retention_prunes_entries_beyond_cutoff_without_deleting_permanent_files() {
        let root = tempdir().unwrap();
        let permanent = root.path().join("saved.mp4");
        fs::write(&permanent, b"keep").unwrap();
        let now = Utc.with_ymd_and_hms(2026, 7, 19, 12, 0, 0).unwrap();
        let id = Uuid::new_v4().to_string();
        let entry = recording(
            &id,
            (now - Duration::days(31)).to_rfc3339(),
            Some(permanent.to_string_lossy().into()),
        );
        save_entry(root.path(), &entry, None, b"poster", None).unwrap();
        let cutoff = screenshot(
            &Uuid::new_v4().to_string(),
            (now - Duration::days(30)).to_rfc3339(),
        );
        let newer = screenshot(
            &Uuid::new_v4().to_string(),
            (now - Duration::days(2)).to_rfc3339(),
        );
        let expired = screenshot(
            &Uuid::new_v4().to_string(),
            (now - Duration::days(30) - Duration::milliseconds(1)).to_rfc3339(),
        );
        for entry in [&cutoff, &newer, &expired] {
            save_capture(root.path(), entry, b"full", b"preview").unwrap();
        }
        let history = load(root.path(), now).unwrap();
        assert_eq!(
            history.iter().map(|e| &e.id).collect::<Vec<_>>(),
            [&newer.id, &cutoff.id]
        );
        assert!(!root.path().join(&expired.id).exists());
        assert!(!root.path().join(&entry.id).exists());
        assert_eq!(fs::read(permanent).unwrap(), b"keep");
    }

    #[test]
    fn invalid_directories_and_metadata_are_removed() {
        let root = tempdir().unwrap();
        let bad = root.path().join("not-a-uuid");
        fs::create_dir(&bad).unwrap();
        fs::write(bad.join(HISTORY_METADATA_FILE), b"{").unwrap();
        assert!(load(root.path(), Utc::now()).unwrap().is_empty());
        assert!(!bad.exists());
    }

    #[test]
    fn metadata_updates_and_replacement_preserve_recording_recovery() {
        let root = tempdir().unwrap();
        let id = Uuid::new_v4().to_string();
        let source = root.path().join("source.mp4");
        fs::write(&source, b"media").unwrap();
        let mut entry = recording(&id, Utc::now().to_rfc3339(), None);
        save_recording(root.path(), &entry, b"poster", &source).unwrap();
        entry.size_bytes = 99;
        update_metadata(root.path(), &entry).unwrap();
        save_entry(root.path(), &entry, None, b"new-poster", None).unwrap();
        assert_eq!(
            fs::read(root.path().join(&id).join("media.mp4")).unwrap(),
            b"media"
        );
        assert_eq!(load(root.path(), Utc::now()).unwrap()[0].size_bytes, 99);
    }

    #[test]
    fn screenshot_round_trip_requires_complete_files() {
        let root = tempdir().unwrap();
        let id = Uuid::new_v4().to_string();
        let entry = screenshot(&id, Utc::now().to_rfc3339());
        save_capture(root.path(), &entry, b"full", b"preview").unwrap();
        assert_eq!(
            read_images(root.path(), &id).unwrap(),
            (b"full".to_vec(), b"preview".to_vec())
        );
        assert_eq!(load(root.path(), Utc::now()).unwrap().len(), 1);
        fs::remove_file(root.path().join(&id).join(HISTORY_IMAGE_FILE)).unwrap();
        assert!(load(root.path(), Utc::now()).unwrap().is_empty());
        assert!(matches!(
            read_images(root.path(), "../escape"),
            Err(Error::HistoryUnavailable)
        ));
    }

    #[test]
    fn failed_replacement_keeps_old_entry_and_removes_partial_write() {
        let root = tempdir().unwrap();
        let id = Uuid::new_v4().to_string();
        let mut entry = screenshot(&id, Utc::now().to_rfc3339());
        save_capture(root.path(), &entry, b"old full", b"old preview").unwrap();
        entry.width = 73;
        assert!(
            save_entry(
                root.path(),
                &entry,
                Some(b"new full"),
                b"new preview",
                Some((&root.path().join("missing.mp4"), "media.mp4"))
            )
            .is_err()
        );
        assert_eq!(
            read_images(root.path(), &id).unwrap(),
            (b"old full".to_vec(), b"old preview".to_vec())
        );
        assert_eq!(load(root.path(), Utc::now()).unwrap()[0].width, 1);
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    }

    #[test]
    fn thumbnail_preserves_aspect_and_small_rgba_pixels() {
        let small = RgbaImage::from_fn(3, 2, |x, y| {
            image::Rgba([x as u8 * 50, y as u8 * 80, 27, 120])
        });
        let decoded = image::load_from_memory(&encode_thumbnail_png(&small).unwrap())
            .unwrap()
            .to_rgba8();
        assert_eq!(decoded, small);
        let portrait = RgbaImage::new(600, 1200);
        let decoded = image::load_from_memory(&encode_thumbnail_png(&portrait).unwrap()).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (160, 320));
    }
}
