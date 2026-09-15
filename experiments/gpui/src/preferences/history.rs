//! Durable index and retention for the GPUI experiment's private capture files.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs,
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
    pub created_at: SystemTime,
    pub saved_path: Option<PathBuf>,
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
    saved_path: Option<PathBuf>,
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
        } else {
            state.entries.insert(
                name.clone(),
                StoredEntry {
                    created_at: SystemTime::now(),
                    saved_path: Some(destination.to_path_buf()),
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
    if !capture_directory(profile)? {
        return Ok(Vec::new());
    }
    let mut state = read_state(profile)?;
    let mut present = HashSet::new();
    let mut entries = Vec::new();
    let mut changed = false;

    let directory = fs::read_dir(&root).context("read private capture history")?;
    for item in directory {
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
                saved_path: None,
            }
        });
        entries.push(Entry {
            path,
            created_at: stored.created_at,
            saved_path: stored.saved_path.clone(),
        });
    }
    state.entries.retain(|name, _| {
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

fn delete_entry(profile: &Path, path: &Path) -> Result<()> {
    let name = checked_capture_name(profile, path)?;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() || !is_supported(path) {
        bail!("refusing to delete a non-regular capture");
    }
    let mut state = read_state(profile)?;
    fs::remove_file(path)?;
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
    if !capture_directory(profile)? {
        bail!("private capture directory is missing");
    }
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
                saved_path: None,
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
