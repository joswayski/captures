//! Durable index and retention for the GPUI experiment's private capture files.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs,
    hash::{DefaultHasher, Hash, Hasher},
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, SystemTime},
};
use uuid::Uuid;

const STATE_FILE: &str = "capture-history.json";
const RETENTION: Duration = Duration::from_secs(30 * 24 * 60 * 60);
static HISTORY_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Debug)]
pub struct Entry {
    pub path: PathBuf,
    pub source: PathBuf,
    pub created_at: SystemTime,
    pub dropped_frames: u64,
    pub saved_path: Option<PathBuf>,
    pub missing: bool,
    pub preview: Option<Preview>,
}

#[derive(Clone, Debug)]
pub struct Preview {
    pub image: PathBuf,
    pub metadata: captures_media::MediaMetadata,
    pub modified: SystemTime,
}

/// Run on a worker: probing/creating posters must not stall the history window.
pub fn preview(profile: &Path, path: &Path) -> Result<Preview> {
    let source = {
        let _guard = HISTORY_LOCK.lock().expect("history lock poisoned");
        let name = checked_capture_name(profile, path)?;
        let state = read_state(profile)?;
        media_source(
            path,
            state
                .entries
                .get(&name)
                .and_then(|entry| entry.saved_path.as_deref()),
        )
    };
    let file = fs::symlink_metadata(&source)?;
    if !file.file_type().is_file() {
        bail!("capture is not regular media");
    }
    let modified = file.modified()?;
    let (image, metadata) = if super::is_image_path(path) {
        let (width, height) = image::image_dimensions(path)?;
        (
            path.to_path_buf(),
            captures_media::MediaMetadata {
                kind: captures_media::MediaKind::Screenshot,
                mime_type: String::new(),
                width,
                height,
                duration_ms: None,
                size_bytes: file.len(),
            },
        )
    } else {
        let tools = captures_media::MediaToolchain::from_command_names();
        let metadata = tools.probe(&source)?.metadata;
        let directory = profile.join("preview-posters");
        fs::create_dir_all(&directory)?;
        let poster = crate::previews::media::poster_path(&source, &directory)?;
        if !poster.is_file() {
            tools.create_poster(&source, &poster, &captures_media::CancelToken::default())?;
        }
        (poster, metadata)
    };
    let preview = Preview {
        image,
        metadata,
        modified,
    };
    let _guard = HISTORY_LOCK.lock().expect("history lock poisoned");
    let name = checked_capture_name(profile, path)?;
    let mut state = read_state(profile)?;
    // A probe may finish after deletion. Never resurrect the removed entry.
    if let Some(entry) = state.entries.get_mut(&name) {
        entry.preview = Some(StoredPreview {
            image: preview.image.file_name().unwrap().to_string_lossy().into(),
            metadata: preview.metadata.clone(),
            modified,
        });
        write_state(profile, &state)?;
    }
    Ok(preview)
}

/// Cheap change detection for open History windows, including external removal.
pub fn revision(profile: &Path) -> u64 {
    let mut hash = DefaultHasher::new();
    for name in [STATE_FILE, "captures", "recording-drafts"] {
        let path = profile.join(name);
        let stamp = |path: &Path| {
            fs::symlink_metadata(path)
                .ok()
                .map(|metadata| (metadata.modified().ok(), metadata.len()))
        };
        stamp(&path).hash(&mut hash);
        // Directory mtimes can coalesce within one filesystem clock tick.
        // Hash the entries too, without reading or decoding the media itself.
        if fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.file_type().is_dir())
            && let Ok(directory) = fs::read_dir(&path)
        {
            let mut files = directory
                .filter_map(Result::ok)
                .map(|entry| (entry.file_name(), stamp(&entry.path())))
                .collect::<Vec<_>>();
            files.sort_by(|left, right| left.0.cmp(&right.0));
            files.hash(&mut hash);
        }
    }
    hash.finish()
}

#[derive(Default, Deserialize, Serialize)]
struct State {
    #[serde(default)]
    entries: HashMap<String, StoredEntry>,
}

#[derive(Deserialize, Serialize)]
struct StoredEntry {
    created_at: SystemTime,
    #[serde(default)]
    dropped_frames: u64,
    #[serde(default)]
    saved_path: Option<PathBuf>,
    #[serde(default)]
    preview: Option<StoredPreview>,
}

#[derive(Deserialize, Serialize)]
struct StoredPreview {
    image: String,
    metadata: captures_media::MediaMetadata,
    modified: SystemTime,
}

fn cached_preview(profile: &Path, stored: &StoredEntry) -> Option<Preview> {
    let preview = stored.preview.as_ref()?;
    if !plain_filename(&preview.image) {
        return None;
    }
    let directory = if preview.metadata.kind == captures_media::MediaKind::Screenshot {
        "captures"
    } else {
        "preview-posters"
    };
    let root = profile.join(directory);
    if !fs::symlink_metadata(&root).ok()?.file_type().is_dir() {
        return None;
    }
    let image = root.join(&preview.image);
    if !fs::symlink_metadata(&image).ok()?.file_type().is_file() {
        return None;
    }
    Some(Preview {
        image,
        metadata: preview.metadata.clone(),
        modified: preview.modified,
    })
}

fn plain_filename(name: &str) -> bool {
    Path::new(name).file_name().and_then(|value| value.to_str()) == Some(name)
}

// Shipping recording_media_path prefers recovery media, then the permanent save.
fn media_source(path: &Path, saved: Option<&Path>) -> PathBuf {
    if !super::is_image_path(path)
        && fs::symlink_metadata(path)
            .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
        && let Some(saved) = saved
        && is_supported(saved)
        && fs::symlink_metadata(saved).is_ok_and(|metadata| metadata.file_type().is_file())
    {
        return saved.to_path_buf();
    }
    path.to_path_buf()
}

pub fn is_supported(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|value| value.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("png" | "jpg" | "jpeg" | "webp" | "gif" | "mp4" | "webm")
    )
}

/// Prune private capture sources once during application startup.
///
/// This deliberately considers only direct, non-symlink files under
/// `<profile>/captures`; exported copies and paths outside the profile are never
/// traversed or removed. A capture expires only when it is strictly older than
/// thirty days.
pub fn prune_capture_history(profile: &Path) -> Result<usize> {
    let _guard = HISTORY_LOCK.lock().expect("history lock poisoned");
    prune_at(profile, SystemTime::now())
}

pub fn load(profile: &Path) -> Result<Vec<Entry>> {
    let _guard = HISTORY_LOCK.lock().expect("history lock poisoned");
    load_entries(profile)
}

/// Store capture telemetry before the completed recording journal is retired.
pub fn record_dropped_frames(profile: &Path, source: &Path, count: u64) -> Result<()> {
    let _guard = HISTORY_LOCK.lock().expect("history lock poisoned");
    let name = checked_capture_name(profile, source)?;
    if super::is_image_path(source) || !fs::symlink_metadata(source)?.file_type().is_file() {
        bail!("recording telemetry requires a regular recording in this profile");
    }
    load_entries(profile)?;
    let mut state = read_state(profile)?;
    state
        .entries
        .get_mut(&name)
        .context("recording is not indexed")?
        .dropped_frames = count;
    write_state(profile, &state)
}

/// Opening a permanent export must preserve the same warning as its recovery copy.
pub fn dropped_frames(profile: &Path, source: &Path) -> Result<u64> {
    let _guard = HISTORY_LOCK.lock().expect("history lock poisoned");
    Ok(source_dropped_frames(
        &read_state(profile)?,
        profile,
        source,
    ))
}

fn source_dropped_frames(state: &State, profile: &Path, source: &Path) -> u64 {
    let private = (source.parent() == Some(profile.join("captures").as_path()))
        .then(|| source.file_name().and_then(|name| name.to_str()))
        .flatten()
        .and_then(|name| state.entries.get(name));
    private
        .or_else(|| {
            state
                .entries
                .values()
                .find(|entry| entry.saved_path.as_deref() == Some(source))
        })
        .map_or(0, |entry| entry.dropped_frames)
}

/// A plain preview/ready-notice save links the permanent copy without replacing
/// original recovery pixels (the preferred screenshot format may be lossy).
pub fn link_saved(profile: &Path, source: &Path, destination: &Path) -> Result<()> {
    if source.parent() != Some(profile.join("captures").as_path()) {
        return record_export(profile, Some(source), destination, false).map(|_| ());
    }
    let _guard = HISTORY_LOCK.lock().expect("history lock poisoned");
    if !fs::symlink_metadata(destination)?.file_type().is_file() || !is_supported(destination) {
        bail!("saved export is not supported regular media");
    }
    load_entries(profile)?;
    let name = checked_capture_name(profile, source)?;
    let mut state = read_state(profile)?;
    state
        .entries
        .get_mut(&name)
        .context("capture recovery is unavailable")?
        .saved_path = Some(destination.to_path_buf());
    write_state(profile, &state)
}

/// Index a completed export and keep a private recovery copy.
pub fn record_export(
    profile: &Path,
    source: Option<&Path>,
    destination: &Path,
    new_file: bool,
) -> Result<PathBuf> {
    let _guard = HISTORY_LOCK.lock().expect("history lock poisoned");
    let extension = destination
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .filter(|_| is_supported(destination))
        .context("export has an unsupported extension")?;
    let metadata = fs::symlink_metadata(destination).context("exported media is unavailable")?;
    if !metadata.file_type().is_file() {
        bail!("exported media is not a regular file");
    }
    let root = profile.join("captures");
    if destination.parent() == Some(root.as_path()) {
        bail!("export destination is inside private capture history");
    }
    if let Some(source) = source
        && !fs::symlink_metadata(source)?.file_type().is_file()
    {
        bail!("export source is not a regular file");
    }
    match fs::symlink_metadata(&root) {
        Ok(metadata) if !metadata.file_type().is_dir() => {
            bail!("private capture directory is not a real directory")
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => fs::create_dir_all(&root)?,
        Err(error) => return Err(error.into()),
    }
    if !fs::symlink_metadata(&root)?.file_type().is_dir() {
        bail!("private capture directory is not a real directory");
    }
    if destination.canonicalize()?.parent() == Some(root.canonicalize()?.as_path()) {
        bail!("export destination is inside private capture history");
    }

    // Captures can be exported before the History window has ever been opened.
    load_entries(profile)?;
    let mut state = read_state(profile)?;
    let dropped_frames = source.map_or(0, |source| source_dropped_frames(&state, profile, source));
    let known = (!new_file)
        .then_some(source)
        .flatten()
        .and_then(|path| checked_capture_name(profile, path).ok())
        .filter(|name| {
            Path::new(name)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case(&extension))
        })
        .filter(|name| state.entries.contains_key(name));
    let name = known.unwrap_or_else(|| format!("{}.{}", Uuid::new_v4(), extension));
    let recovery = root.join(&name);
    let temporary = root.join(format!(".{}.tmp", Uuid::new_v4()));
    let replacing = state.entries.contains_key(&name);
    let result = (|| -> Result<()> {
        fs::copy(destination, &temporary)?;
        fs::rename(&temporary, &recovery)?;
        if let Some(entry) = state.entries.get_mut(&name) {
            entry.saved_path = Some(destination.to_path_buf());
            entry.preview = None;
        } else {
            state.entries.insert(
                name.clone(),
                StoredEntry {
                    created_at: SystemTime::now(),
                    dropped_frames,
                    saved_path: Some(destination.to_path_buf()),
                    preview: None,
                },
            );
        }
        write_state(profile, &state)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
        if !replacing {
            let _ = fs::remove_file(&recovery);
        }
    }
    result?;
    Ok(recovery)
}

fn load_entries(profile: &Path) -> Result<Vec<Entry>> {
    let root = profile.join("captures");
    let exists = capture_directory(profile)?;
    let mut state = read_state(profile)?;
    let mut present = HashSet::new();
    let mut entries = Vec::new();
    let mut changed = false;

    let directory = exists
        .then(|| fs::read_dir(&root))
        .transpose()
        .context("read private capture history")?;
    for item in directory.into_iter().flatten() {
        let item = item?;
        let path = item.path();
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if !metadata.file_type().is_file() || !is_supported(&path) {
            continue;
        }
        let Some(name) = item.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        present.insert(name.clone());
        let created = metadata.modified().unwrap_or(SystemTime::now());
        let stored = state.entries.entry(name).or_insert_with(|| {
            changed = true;
            StoredEntry {
                created_at: created,
                dropped_frames: 0,
                saved_path: None,
                preview: None,
            }
        });
        entries.push(Entry {
            source: path.clone(),
            path,
            created_at: stored.created_at,
            dropped_frames: stored.dropped_frames,
            saved_path: stored.saved_path.clone(),
            missing: false,
            preview: cached_preview(profile, stored),
        });
    }
    state.entries.retain(|name, stored| {
        let path = root.join(name);
        // Retain absent recordings, not symlinks, invalid index keys, or missing
        // screenshot recovery pixels. The poster/metadata survives the media.
        let missing = !present.contains(name)
            && plain_filename(name)
            && is_supported(&path)
            && !super::is_image_path(&path)
            && fs::symlink_metadata(&path)
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound);
        if missing {
            let source = media_source(&path, stored.saved_path.as_deref());
            entries.push(Entry {
                missing: source == path,
                source,
                path,
                created_at: stored.created_at,
                dropped_frames: stored.dropped_frames,
                saved_path: stored.saved_path.clone(),
                preview: cached_preview(profile, stored),
            });
            return true;
        }
        let keep = present.contains(name);
        changed |= !keep;
        keep
    });
    if changed {
        write_state(profile, &state)?;
    }
    entries.sort_by(|left, right| right.created_at.cmp(&left.created_at));
    Ok(entries)
}

fn capture_directory(profile: &Path) -> Result<bool> {
    match fs::symlink_metadata(profile.join("captures")) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(true),
        Ok(_) => bail!("private capture directory is not a real directory"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

pub fn delete(profile: &Path, path: &Path) -> Result<()> {
    let _guard = HISTORY_LOCK.lock().expect("history lock poisoned");
    delete_entry(profile, path)
}

/// Clear private recovery files only; permanent exports and recording journals
/// are outside this operation, including when the history filter is narrowed.
pub fn clear(profile: &Path) -> Result<()> {
    let _guard = HISTORY_LOCK.lock().expect("history lock poisoned");
    for entry in load_entries(profile)? {
        delete_entry(profile, &entry.path)?;
    }
    Ok(())
}

fn delete_entry(profile: &Path, path: &Path) -> Result<()> {
    let name = checked_capture_name(profile, path)?;
    if !is_supported(path) {
        bail!("refusing to delete a non-regular capture");
    }
    let mut state = read_state(profile)?;
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => fs::remove_file(path)?,
        Ok(_) => bail!("refusing to delete a non-regular capture"),
        Err(error)
            if error.kind() == std::io::ErrorKind::NotFound
                && state.entries.contains_key(&name) => {}
        Err(error) => return Err(error.into()),
    }
    state.entries.remove(&name);
    write_state(profile, &state)
}

fn prune_at(profile: &Path, now: SystemTime) -> Result<usize> {
    let entries = load_entries(profile)?;
    let mut removed = 0;
    for entry in entries {
        if now
            .duration_since(entry.created_at)
            .is_ok_and(|age| age > RETENTION)
        {
            delete_entry(profile, &entry.path)?;
            removed += 1;
        }
    }
    Ok(removed)
}

fn checked_capture_name(profile: &Path, path: &Path) -> Result<String> {
    capture_directory(profile)?;
    let root = profile.join("captures");
    if path.parent() != Some(root.as_path()) {
        bail!("capture is outside this profile");
    }
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
        .context("capture has no valid file name")
}

fn state_path(profile: &Path) -> PathBuf {
    profile.join(STATE_FILE)
}

fn read_state(profile: &Path) -> Result<State> {
    match fs::read(state_path(profile)) {
        Ok(bytes) => serde_json::from_slice(&bytes).context("invalid capture history state"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
        Err(error) => Err(error.into()),
    }
}

fn write_state(profile: &Path, state: &State) -> Result<()> {
    fs::create_dir_all(profile)?;
    let destination = state_path(profile);
    let temporary = profile.join(format!(".{STATE_FILE}.tmp"));
    fs::write(&temporary, serde_json::to_vec_pretty(state)?)?;
    fs::rename(temporary, destination)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::UNIX_EPOCH;

    #[test]
    fn preview_reads_actual_image_dimensions_without_decoding_on_ui_thread() {
        let directory = tempfile::tempdir().unwrap();
        let image = seed(directory.path(), "fixture.png", SystemTime::now());
        image::RgbaImage::new(73, 29).save(&image).unwrap();
        let preview = preview(directory.path(), &image).unwrap();
        assert_eq!((preview.metadata.width, preview.metadata.height), (73, 29));
        assert_eq!(
            preview.metadata.size_bytes,
            fs::metadata(&image).unwrap().len()
        );
        assert_eq!(preview.image, image);
        assert_eq!(preview.metadata.kind, captures_media::MediaKind::Screenshot);
        let reread = load(directory.path())
            .unwrap()
            .pop()
            .unwrap()
            .preview
            .unwrap();
        assert_eq!(reread.metadata, preview.metadata);
        assert_eq!(reread.image, image);
        assert_eq!(reread.modified, preview.modified);
    }

    #[test]
    fn missing_recording_retains_poster_and_metadata_until_direct_removal() {
        let directory = tempfile::tempdir().unwrap();
        let profile = directory.path();
        let source = seed(profile, "gone.mp4", UNIX_EPOCH + Duration::from_secs(123));
        let screenshot = seed(profile, "gone.png", SystemTime::now());
        let saved = profile.join("saved.mp4");
        fs::write(&saved, b"permanent export").unwrap();
        link_saved(profile, &source, &saved).unwrap();
        let poster = profile.join("preview-posters/poster.png");
        fs::create_dir_all(poster.parent().unwrap()).unwrap();
        image::RgbaImage::new(64, 36).save(&poster).unwrap();
        let metadata = captures_media::MediaMetadata {
            kind: captures_media::MediaKind::Video,
            mime_type: "video/mp4".into(),
            width: 640,
            height: 360,
            duration_ms: Some(12_345),
            size_bytes: 123_456,
        };
        let mut state = read_state(profile).unwrap();
        state.entries.get_mut("gone.mp4").unwrap().preview = Some(StoredPreview {
            image: "poster.png".into(),
            metadata: metadata.clone(),
            modified: UNIX_EPOCH,
        });
        write_state(profile, &state).unwrap();
        let before = revision(profile);
        fs::remove_file(&source).unwrap();
        fs::remove_file(screenshot).unwrap();
        assert_ne!(before, revision(profile));
        let entries = load(profile).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(!entries[0].missing);
        assert_eq!(entries[0].source, saved);
        assert_eq!(entries[0].saved_path, Some(saved.clone()));
        let preview = entries[0].preview.as_ref().unwrap();
        assert_eq!(preview.image, poster);
        assert_eq!(preview.metadata, metadata);
        // Only when both copies disappear is this a missing recording.
        let moved = profile.join("moved-export.mp4");
        fs::rename(&saved, &moved).unwrap();
        assert!(load(profile).unwrap()[0].missing);
        delete(profile, &source).unwrap();
        assert!(load(profile).unwrap().is_empty());
        assert!(read_state(profile).unwrap().entries.is_empty());
        assert_eq!(fs::read(moved).unwrap(), b"permanent export");
    }

    #[test]
    fn removed_capture_directory_keeps_missing_recordings_removable() {
        let directory = tempfile::tempdir().unwrap();
        seed(directory.path(), "gone.mp4", SystemTime::now());
        seed(directory.path(), "gone.png", SystemTime::now());
        fs::remove_dir_all(directory.path().join("captures")).unwrap();
        let entries = load(directory.path()).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].missing);
        clear(directory.path()).unwrap();
        assert!(load(directory.path()).unwrap().is_empty());
        assert!(delete(directory.path(), &directory.path().join("unknown.mp4")).is_err());
    }

    #[test]
    fn missing_index_and_poster_names_cannot_escape_private_directories() {
        let directory = tempfile::tempdir().unwrap();
        let profile = directory.path();
        let source = seed(profile, "gone.gif", SystemTime::now());
        fs::remove_file(source).unwrap();
        let mut state = read_state(profile).unwrap();
        state.entries.insert(
            "../outside.mp4".into(),
            StoredEntry {
                created_at: SystemTime::now(),
                dropped_frames: 0,
                saved_path: None,
                preview: None,
            },
        );
        state.entries.get_mut("gone.gif").unwrap().preview = Some(StoredPreview {
            image: "../outside.png".into(),
            metadata: captures_media::MediaMetadata {
                kind: captures_media::MediaKind::Gif,
                mime_type: "image/gif".into(),
                width: 34,
                height: 12,
                duration_ms: Some(890),
                size_bytes: 123,
            },
            modified: UNIX_EPOCH,
        });
        write_state(profile, &state).unwrap();
        let entries = load(profile).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].missing);
        assert!(entries[0].preview.is_none());
        assert!(
            !read_state(profile)
                .unwrap()
                .entries
                .contains_key("../outside.mp4")
        );
        clear(profile).unwrap();
        assert!(load(profile).unwrap().is_empty());
    }

    #[test]
    fn clear_preserves_saved_exports_and_interrupted_recordings() {
        let directory = tempfile::tempdir().unwrap();
        let source = seed(directory.path(), "shot.png", SystemTime::now());
        let video = seed(directory.path(), "video.mp4", SystemTime::now());
        let saved = directory.path().join("permanent.png");
        fs::write(&saved, b"permanent export").unwrap();
        link_saved(directory.path(), &source, &saved).unwrap();
        let draft = directory.path().join("recording-drafts/pending.mp4");
        fs::create_dir_all(draft.parent().unwrap()).unwrap();
        fs::write(&draft, b"unfinished segment").unwrap();
        clear(directory.path()).unwrap();
        assert!(!source.exists());
        assert!(!video.exists());
        assert!(load(directory.path()).unwrap().is_empty());
        assert_eq!(fs::read(saved).unwrap(), b"permanent export");
        assert_eq!(fs::read(draft).unwrap(), b"unfinished segment");
    }

    fn seed(profile: &Path, name: &str, created: SystemTime) -> PathBuf {
        let root = profile.join("captures");
        fs::create_dir_all(&root).unwrap();
        let path = root.join(name);
        fs::write(&path, b"capture").unwrap();
        let mut state = read_state(profile).unwrap();
        state.entries.insert(
            name.into(),
            StoredEntry {
                created_at: created,
                dropped_frames: 0,
                saved_path: None,
                preview: None,
            },
        );
        write_state(profile, &state).unwrap();
        path
    }

    #[test]
    fn retention_keeps_boundary_and_exports_but_removes_strictly_older_source() {
        let directory = tempfile::tempdir().unwrap();
        let now = UNIX_EPOCH + Duration::from_secs(2_000_000_000) + Duration::from_nanos(9);
        let boundary = seed(directory.path(), "boundary.png", now - RETENTION);
        let expired = seed(
            directory.path(),
            "expired.mp4",
            now - RETENTION - Duration::from_nanos(1),
        );
        let recent = seed(
            directory.path(),
            "recent.webp",
            now - RETENTION + Duration::from_nanos(1),
        );
        let export = directory.path().join("exports/saved.mp4");
        fs::create_dir_all(export.parent().unwrap()).unwrap();
        fs::copy(&expired, &export).unwrap();

        assert_eq!(prune_at(directory.path(), now).unwrap(), 1);
        assert!(boundary.exists());
        assert!(!expired.exists());
        assert!(recent.exists());
        assert!(export.exists());
    }

    #[test]
    fn filtering_and_sorting_are_durable() {
        let directory = tempfile::tempdir().unwrap();
        let old = seed(
            directory.path(),
            "z.png",
            UNIX_EPOCH + Duration::from_secs(10),
        );
        let new = seed(
            directory.path(),
            "a.webm",
            UNIX_EPOCH + Duration::from_secs(20),
        );
        fs::write(directory.path().join("captures/unsupported.txt"), b"no").unwrap();
        assert_eq!(
            load(directory.path())
                .unwrap()
                .iter()
                .map(|e| &e.path)
                .collect::<Vec<_>>(),
            [&new, &old]
        );
        // Editing a source must not reset its original history retention age.
        fs::write(&old, b"edited").unwrap();
        assert_eq!(
            load(directory.path())
                .unwrap()
                .iter()
                .map(|e| &e.path)
                .collect::<Vec<_>>(),
            [&new, &old]
        );
    }

    #[test]
    fn unindexed_capture_overwrite_and_plain_save_keep_identity_and_source_pixels() {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir(directory.path().join("captures")).unwrap();
        let source = directory.path().join("captures/original.png");
        let export = directory.path().join("saved.png");
        fs::write(&source, b"original").unwrap();
        fs::write(&export, b"edited").unwrap();
        assert_eq!(
            record_export(directory.path(), Some(&source), &export, false).unwrap(),
            source
        );
        let jpeg = directory.path().join("saved.jpg");
        fs::write(&jpeg, b"lossy copy").unwrap();
        link_saved(directory.path(), &source, &jpeg).unwrap();
        assert_eq!(fs::read(&source).unwrap(), b"edited");
        let entry = load(directory.path()).unwrap().pop().unwrap();
        assert_eq!(entry.saved_path, Some(jpeg.clone()));
        let converted = record_export(directory.path(), Some(&source), &jpeg, false).unwrap();
        assert_ne!(converted, source);
        assert_eq!(converted.extension().unwrap(), "jpg");
        assert_eq!(fs::read(&source).unwrap(), b"edited");
    }

    #[test]
    fn export_new_overwrite_reread_and_external_lifetime() {
        let directory = tempfile::tempdir().unwrap();
        let export = directory.path().join("saved.PNG");
        fs::write(&export, b"first").unwrap();
        let recovery = record_export(directory.path(), None, &export, true).unwrap();
        let first = load(directory.path()).unwrap().pop().unwrap();
        assert_eq!(first.saved_path.as_deref(), Some(export.as_path()));
        fs::write(&export, b"second").unwrap();
        assert_eq!(
            record_export(directory.path(), Some(&recovery), &export, false).unwrap(),
            recovery
        );
        let reread = load(directory.path()).unwrap();
        assert_eq!(reread[0].created_at, first.created_at);
        assert_eq!(fs::read(&recovery).unwrap(), b"second");
        delete(directory.path(), &recovery).unwrap();
        assert_eq!(fs::read(export).unwrap(), b"second");
    }

    #[test]
    fn recording_drop_counts_survive_export_format_changes_and_missing_recovery() {
        let directory = tempfile::tempdir().unwrap();
        let profile = directory.path();
        let source = seed(profile, "dropped.mp4", SystemTime::now());
        let clean = seed(profile, "clean.mp4", SystemTime::now());
        record_dropped_frames(profile, &source, 17).unwrap();
        assert_eq!(dropped_frames(profile, &clean).unwrap(), 0);
        let saved = profile.join("saved.mp4");
        fs::write(&saved, b"saved video").unwrap();
        link_saved(profile, &source, &saved).unwrap();
        assert_eq!(dropped_frames(profile, &saved).unwrap(), 17);

        let gif = profile.join("edited.gif");
        fs::write(&gif, b"edited animation").unwrap();
        let recovery = record_export(profile, Some(&saved), &gif, true).unwrap();
        assert_eq!(dropped_frames(profile, &recovery).unwrap(), 17);
        assert_eq!(dropped_frames(profile, &gif).unwrap(), 17);
        fs::remove_file(&source).unwrap();
        let entry = load(profile)
            .unwrap()
            .into_iter()
            .find(|entry| entry.path == source)
            .unwrap();
        assert_eq!(entry.source, saved);
        assert_eq!(entry.dropped_frames, 17);
        fs::remove_file(&saved).unwrap();
        let entry = load(profile)
            .unwrap()
            .into_iter()
            .find(|entry| entry.path == source)
            .unwrap();
        assert!(entry.missing);
        assert_eq!(entry.dropped_frames, 17);
    }

    #[test]
    fn telemetry_is_idempotent_and_cannot_attach_to_screenshots_or_external_files() {
        let directory = tempfile::tempdir().unwrap();
        let profile = directory.path();
        let source = seed(profile, "video.mp4", SystemTime::now());
        record_dropped_frames(profile, &source, 8).unwrap();
        record_dropped_frames(profile, &source, 8).unwrap();
        assert_eq!(dropped_frames(profile, &source).unwrap(), 8);
        let screenshot = seed(profile, "image.png", SystemTime::now());
        assert!(record_dropped_frames(profile, &screenshot, 8).is_err());
        let external = profile.join("video.mp4");
        fs::write(&external, b"external").unwrap();
        assert!(record_dropped_frames(profile, &external, 8).is_err());
        assert_eq!(dropped_frames(profile, &external).unwrap(), 0);
    }

    #[test]
    fn unknown_and_new_file_get_distinct_recoveries_and_prune_preserves_export() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("imported.webp");
        let export = directory.path().join("saved.webp");
        fs::write(&source, b"source").unwrap();
        fs::write(&export, b"one").unwrap();
        let first = record_export(directory.path(), Some(&source), &export, false).unwrap();
        let second = record_export(directory.path(), Some(&first), &export, true).unwrap();
        assert_ne!(first, second);
        let name = first.file_name().unwrap().to_str().unwrap().to_owned();
        let mut state = read_state(directory.path()).unwrap();
        state.entries.get_mut(&name).unwrap().created_at = UNIX_EPOCH;
        write_state(directory.path(), &state).unwrap();
        assert_eq!(
            prune_at(
                directory.path(),
                UNIX_EPOCH + RETENTION + Duration::from_secs(1)
            )
            .unwrap(),
            1
        );
        assert!(export.exists());
    }

    #[test]
    fn rejects_invalid_boundaries_and_loads_legacy_state() {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir(directory.path().join("captures")).unwrap();
        fs::write(directory.path().join("captures/legacy.png"), b"legacy").unwrap();
        fs::write(
            state_path(directory.path()),
            br#"{"entries":{"legacy.png":{"created_at":{"secs_since_epoch":10,"nanos_since_epoch":0}}}}"#,
        )
        .unwrap();
        assert!(load(directory.path()).unwrap()[0].saved_path.is_none());
        assert_eq!(load(directory.path()).unwrap()[0].dropped_frames, 0);
        let private = directory.path().join("captures/private.png");
        fs::write(&private, b"private").unwrap();
        assert!(record_export(directory.path(), None, &private, true).is_err());
        let unsupported = directory.path().join("saved.txt");
        fs::write(&unsupported, b"saved").unwrap();
        assert!(record_export(directory.path(), None, &unsupported, true).is_err());
        fs::write(state_path(directory.path()), b"malformed").unwrap();
        let external = directory.path().join("external.png");
        fs::write(&external, b"external").unwrap();
        assert!(record_export(directory.path(), None, &external, true).is_err());
        assert_eq!(fs::read(external).unwrap(), b"external");
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_and_outside_paths_are_never_deleted() {
        use std::os::unix::fs::symlink;
        let directory = tempfile::tempdir().unwrap();
        let outside = directory.path().join("outside.png");
        fs::write(&outside, b"saved").unwrap();
        fs::create_dir_all(directory.path().join("captures")).unwrap();
        let link = directory.path().join("captures/link.png");
        symlink(&outside, &link).unwrap();
        assert!(load(directory.path()).unwrap().is_empty());
        assert!(delete(directory.path(), &link).is_err());
        assert!(delete(directory.path(), &outside).is_err());
        assert_eq!(fs::read(&outside).unwrap(), b"saved");

        let linked_profile = directory.path().join("linked-profile");
        fs::create_dir(&linked_profile).unwrap();
        symlink(directory.path(), linked_profile.join("captures")).unwrap();
        assert!(load(&linked_profile).is_err());
        assert!(
            delete(
                &linked_profile,
                &linked_profile.join("captures/outside.png")
            )
            .is_err()
        );
        assert_eq!(fs::read(outside).unwrap(), b"saved");

        let export = directory.path().join("export.png");
        fs::write(&export, b"export").unwrap();
        assert!(record_export(&linked_profile, None, &export, true).is_err());
    }
}
