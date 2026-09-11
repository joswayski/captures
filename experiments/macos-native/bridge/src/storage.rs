use std::{
    fs::{self, OpenOptions},
    io::{Cursor, Write},
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use captures_capture::DisplayDescriptor;
use captures_recording::{
    DraftStore, RecordingDraftManifest, RecordingOptions, RecordingSegmentInfo,
    RecordingSegmentManifest, RecordingState,
};
use image::{ImageFormat, RgbaImage};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::protocol::{BridgeResult, ScreenshotTarget};

const FREEZE_RETENTION_MS: u64 = 24 * 60 * 60 * 1_000;

pub struct Draft {
    pub store: DraftStore,
    pub directory: PathBuf,
    pub manifest: RecordingDraftManifest,
}

pub struct StagedOutput {
    pub path: PathBuf,
    work_directory: PathBuf,
}

impl Drop for StagedOutput {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.work_directory);
    }
}

pub fn data_directory() -> BridgeResult<PathBuf> {
    if let Some(path) = std::env::var_os("CAPTURES_NATIVE_DATA") {
        if path.is_empty() {
            return Err("CAPTURES_NATIVE_DATA is empty".to_owned());
        }
        return Ok(PathBuf::from(path));
    }
    let home = std::env::var_os("HOME").ok_or("HOME is unavailable")?;
    Ok(PathBuf::from(home).join("Library/Application Support/Captures Native Experiment"))
}

pub fn draft_store() -> BridgeResult<DraftStore> {
    let root = data_directory()?.join("recording-drafts");
    create_private_directory(&root)?;
    Ok(DraftStore::new(root))
}

#[derive(Deserialize, Serialize)]
struct FreezeManifest {
    id: String,
    display: DisplayDescriptor,
}

pub fn save_freeze(
    image: &RgbaImage,
    display: DisplayDescriptor,
) -> BridgeResult<(String, PathBuf)> {
    let bytes = png_bytes(image)?;
    let root = freeze_root()?;
    prune_stale_freezes(&root);
    let id = Uuid::new_v4().to_string();
    let directory = root.join(&id);
    fs::create_dir(&directory).map_err(|error| error.to_string())?;
    set_private_permissions(&directory)?;
    let path = directory.join("freeze.png");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| error.to_string())?;
    file.write_all(&bytes).map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    let manifest = serde_json::to_vec(&FreezeManifest {
        id: id.clone(),
        display,
    })
    .map_err(|error| error.to_string())?;
    fs::write(directory.join("freeze.json"), manifest).map_err(|error| error.to_string())?;
    Ok((id, path))
}

pub fn load_freeze(id: &str) -> BridgeResult<(PathBuf, RgbaImage, DisplayDescriptor)> {
    let directory = owned_freeze_directory(id)?;
    let path = checked_draft_file(&directory, "freeze.png")?;
    let manifest_path = checked_draft_file(&directory, "freeze.json")?;
    let manifest: FreezeManifest =
        serde_json::from_slice(&fs::read(manifest_path).map_err(|error| error.to_string())?)
            .map_err(|error| format!("private freeze metadata is invalid: {error}"))?;
    if manifest.id != id {
        return Err("private freeze metadata does not match its directory".to_owned());
    }
    let image = image::open(&path)
        .map_err(|error| format!("private freeze is unreadable: {error}"))?
        .into_rgba8();
    Ok((directory, image, manifest.display))
}

pub fn discard_freeze(id: &str) -> BridgeResult<()> {
    let directory = owned_freeze_directory(id)?;
    match fs::remove_dir_all(directory) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

fn freeze_root() -> BridgeResult<PathBuf> {
    let root = data_directory()?.join("private-freezes");
    create_private_directory(&root)?;
    Ok(root)
}

fn owned_freeze_directory(id: &str) -> BridgeResult<PathBuf> {
    Uuid::parse_str(id).map_err(|_| "private freeze ID is invalid".to_owned())?;
    Ok(freeze_root()?.join(id))
}

fn prune_stale_freezes(root: &Path) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let id = entry.file_name().to_string_lossy().into_owned();
        if Uuid::parse_str(&id).is_err() {
            continue;
        }
        let Ok(metadata) = fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            continue;
        }
        let stale = metadata
            .modified()
            .ok()
            .and_then(|modified| SystemTime::now().duration_since(modified).ok())
            .is_some_and(|age| duration_to_ms(age) > FREEZE_RETENTION_MS);
        if stale {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

pub fn create_draft(options: &RecordingOptions) -> BridgeResult<Draft> {
    let store = draft_store()?;
    let manifest =
        RecordingDraftManifest::new(Uuid::new_v4().to_string(), options.clone(), now_ms());
    let directory = store.create(&manifest).map_err(|error| error.to_string())?;
    set_private_permissions(&directory)?;
    Ok(Draft {
        store,
        directory,
        manifest,
    })
}

pub fn register_pending_segment(draft: &mut Draft, info: &PendingSegment) -> BridgeResult<()> {
    let index = u32::try_from(draft.manifest.segments.len())
        .map_err(|_| "recording has too many segments")?;
    draft.manifest.segments.push(RecordingSegmentManifest {
        index,
        relative_path: relative_owned_path(&draft.directory, &info.path)?,
        system_audio_relative_path: info
            .system_audio
            .as_ref()
            .map(|(path, _)| relative_owned_path(&draft.directory, path))
            .transpose()?,
        system_audio_offset_ms: info.system_audio.as_ref().map_or(0, |(_, offset)| *offset),
        system_audio_warning: None,
        microphone_relative_path: info
            .microphone
            .as_ref()
            .map(|(path, _)| relative_owned_path(&draft.directory, path))
            .transpose()?,
        microphone_offset_ms: info.microphone.as_ref().map_or(0, |(_, offset)| *offset),
        microphone_warning: None,
        started_at_ms: now_ms(),
        duration_ms: 0,
        width: info.width,
        height: info.height,
        size_bytes: 0,
        dropped_frames: 0,
        complete: false,
    });
    draft.manifest.state = RecordingState::Recording;
    save_draft(draft)
}

pub struct PendingSegment {
    pub path: PathBuf,
    pub system_audio: Option<(PathBuf, i64)>,
    pub microphone: Option<(PathBuf, i64)>,
    pub width: u32,
    pub height: u32,
}

pub fn complete_segment(draft: &mut Draft, info: &RecordingSegmentInfo) -> BridgeResult<()> {
    let relative = relative_owned_path(&draft.directory, &info.path)?;
    let pending = draft
        .manifest
        .segments
        .iter_mut()
        .rev()
        .find(|segment| !segment.complete && segment.relative_path == relative)
        .ok_or("recording draft does not contain the active segment")?;
    pending.system_audio_relative_path = info
        .system_audio_path
        .as_ref()
        .map(|path| relative_owned_path(&draft.directory, path))
        .transpose()?;
    pending.system_audio_offset_ms = info.system_audio_offset_ms;
    pending
        .system_audio_warning
        .clone_from(&info.system_audio_warning);
    pending.microphone_relative_path = info
        .microphone_path
        .as_ref()
        .map(|path| relative_owned_path(&draft.directory, path))
        .transpose()?;
    pending.microphone_offset_ms = info.microphone_offset_ms;
    pending
        .microphone_warning
        .clone_from(&info.microphone_warning);
    pending.duration_ms = info.duration_ms;
    pending.width = info.width;
    pending.height = info.height;
    pending.size_bytes = info.size_bytes;
    pending.dropped_frames = info.dropped_frames;
    pending.complete = true;
    draft.manifest.state = RecordingState::Paused;
    save_draft(draft)
}

pub fn save_draft(draft: &mut Draft) -> BridgeResult<()> {
    draft.manifest.updated_at_ms = now_ms();
    draft
        .store
        .save(&draft.manifest)
        .map_err(|error| error.to_string())
}

pub fn checked_draft_file(directory: &Path, relative: &str) -> BridgeResult<PathBuf> {
    let relative = Path::new(relative);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err("recording draft media path escapes its session directory".to_owned());
    }
    let path = directory.join(relative);
    let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("recording draft media must be a regular private file".to_owned());
    }
    Ok(path)
}

pub fn save_screenshot(
    directory: &Path,
    image: &RgbaImage,
    _target: &ScreenshotTarget,
) -> BridgeResult<PathBuf> {
    let bytes = png_bytes(image)?;
    let staged = stage_output(directory, "png")?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staged.path)
        .map_err(|error| error.to_string())?;
    file.write_all(&bytes).map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    publish_unique(&staged, directory, "png")
}

fn png_bytes(image: &RgbaImage) -> BridgeResult<Vec<u8>> {
    let mut bytes = Cursor::new(Vec::new());
    image
        .write_to(&mut bytes, ImageFormat::Png)
        .map_err(|error| error.to_string())?;
    Ok(bytes.into_inner())
}

pub fn stage_output(directory: &Path, extension: &str) -> BridgeResult<StagedOutput> {
    fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let work_directory = unique_private_directory(directory, "publish")?;
    Ok(StagedOutput {
        path: work_directory.join(format!("media.{extension}")),
        work_directory,
    })
}

pub fn publish_unique(
    staged: &StagedOutput,
    directory: &Path,
    extension: &str,
) -> BridgeResult<PathBuf> {
    for sequence in 0_u32.. {
        let path = directory.join(format!("Capture-{}-{sequence}.{extension}", now_ms()));
        match fs::hard_link(&staged.path, &path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    unreachable!()
}

pub fn publish_exact(staged: &StagedOutput, output: &Path) -> BridgeResult<PathBuf> {
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    fs::hard_link(&staged.path, output).map_err(|error| {
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            format!("export destination already exists: {}", output.display())
        } else {
            error.to_string()
        }
    })?;
    Ok(output.to_path_buf())
}

pub fn stage_for_exact_output(output: &Path, extension: &str) -> BridgeResult<StagedOutput> {
    let parent = output.parent().unwrap_or_else(|| Path::new("."));
    stage_output(parent, extension)
}

fn relative_owned_path(directory: &Path, path: &Path) -> BridgeResult<String> {
    let relative = path
        .strip_prefix(directory)
        .map_err(|_| "recording media escaped its private draft directory")?;
    if relative
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err("recording media escaped its private draft directory".to_owned());
    }
    Ok(relative.to_string_lossy().into_owned())
}

fn create_private_directory(path: &Path) -> BridgeResult<()> {
    fs::create_dir_all(path).map_err(|error| error.to_string())?;
    set_private_permissions(path)
}

fn unique_private_directory(parent: &Path, purpose: &str) -> BridgeResult<PathBuf> {
    for _ in 0..100 {
        let path = parent.join(format!(".captures-{purpose}-{}", Uuid::new_v4()));
        match fs::create_dir(&path) {
            Ok(()) => {
                set_private_permissions(&path)?;
                return Ok(path);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    Err("could not allocate a private work directory".to_owned())
}

#[cfg(unix)]
fn set_private_permissions(path: &Path) -> BridgeResult<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|error| error.to_string())
}

#[cfg(not(unix))]
fn set_private_permissions(_path: &Path) -> BridgeResult<()> {
    Ok(())
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn duration_to_ms(duration: std::time::Duration) -> u64 {
    duration.as_millis().try_into().unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use image::RgbaImage;

    use super::{
        checked_draft_file, discard_freeze, load_freeze, publish_exact, save_freeze,
        stage_for_exact_output,
    };

    #[test]
    fn manifest_media_rejects_traversal_and_symlinks() {
        let root = tempdir().unwrap();
        fs::write(root.path().join("segment.mp4"), b"owned").unwrap();
        assert!(checked_draft_file(root.path(), "segment.mp4").is_ok());
        assert!(checked_draft_file(root.path(), "../outside.mp4").is_err());

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(
                root.path().join("segment.mp4"),
                root.path().join("link.mp4"),
            )
            .unwrap();
            assert!(checked_draft_file(root.path(), "link.mp4").is_err());
        }
    }

    #[test]
    fn exact_publication_never_replaces_an_existing_export() {
        let root = tempdir().unwrap();
        let output = root.path().join("edited.mp4");
        fs::write(&output, b"original").unwrap();
        let staged = stage_for_exact_output(&output, "mp4").unwrap();
        fs::write(&staged.path, b"replacement").unwrap();

        assert!(publish_exact(&staged, &output).is_err());
        assert_eq!(fs::read(output).unwrap(), b"original");
    }

    #[test]
    fn freezes_are_addressed_only_by_owned_ids_and_discard_is_idempotent() {
        let root = tempdir().unwrap();
        // SAFETY: this test process uses the variable only for this synchronous
        // test, before the global bridge engine is initialized.
        unsafe { std::env::set_var("CAPTURES_NATIVE_DATA", root.path()) };
        let image = RgbaImage::new(7, 5);
        let display = captures_capture::DisplayDescriptor {
            id: "1".to_owned(),
            name: "test".to_owned(),
            x: 0,
            y: 0,
            width: 7,
            height: 5,
            scale_factor: 1.0,
            is_primary: true,
        };
        let (id, path) = save_freeze(&image, display).unwrap();
        assert!(path.starts_with(root.path().join("private-freezes").join(&id)));
        assert_eq!(load_freeze(&id).unwrap().1.dimensions(), (7, 5));
        assert!(load_freeze("../recording-drafts").is_err());
        discard_freeze(&id).unwrap();
        discard_freeze(&id).unwrap();
    }
}
