//! Durable index and retention for the GPUI experiment's private capture files.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

const STATE_FILE: &str = "capture-history.json";
const RETENTION: Duration = Duration::from_secs(30 * 24 * 60 * 60);

#[derive(Clone, Debug)]
pub struct Entry {
    pub path: PathBuf,
    pub created_at: SystemTime,
}

#[derive(Default, Deserialize, Serialize)]
struct State {
    #[serde(default)]
    entries: HashMap<String, StoredEntry>,
}

#[derive(Deserialize, Serialize)]
struct StoredEntry {
    created_at: SystemTime,
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
    prune_at(profile, SystemTime::now())
}

pub fn load(profile: &Path) -> Result<Vec<Entry>> {
    load_entries(profile)
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
            }
        });
        entries.push(Entry {
            path,
            created_at: stored.created_at,
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
            delete(profile, &entry.path)?;
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
    }
}
