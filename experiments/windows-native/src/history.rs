use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    Image,
    Video,
    Gif,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Artifact {
    pub path: PathBuf,
    #[serde(default)]
    pub original_path: Option<PathBuf>,
    pub kind: ArtifactKind,
    pub width: u32,
    pub height: u32,
    pub created_ms: u64,
}

impl Artifact {
    pub fn from_path(path: PathBuf, width: u32, height: u32) -> Self {
        let kind = match path
            .extension()
            .and_then(|v| v.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "gif" => ArtifactKind::Gif,
            "mp4" | "webm" | "mov" => ArtifactKind::Video,
            _ => ArtifactKind::Image,
        };
        Self {
            path,
            original_path: None,
            kind,
            width,
            height,
            created_ms: now_ms(),
        }
    }

    pub fn exists_safely(&self) -> bool {
        fs::symlink_metadata(&self.path)
            .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
    }

    pub fn is_trashed(&self) -> bool {
        self.original_path.is_some()
    }
}

pub struct History {
    path: PathBuf,
    entries: Vec<Artifact>,
}

impl History {
    pub fn load(root: &Path) -> Result<Self, String> {
        let path = root.join("history.json");
        let entries = match fs::read(&path) {
            Ok(bytes) => {
                serde_json::from_slice::<Vec<Artifact>>(&bytes).map_err(|e| e.to_string())?
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(error.to_string()),
        };
        Ok(Self {
            path,
            entries: entries
                .into_iter()
                .filter(Artifact::exists_safely)
                .collect(),
        })
    }

    pub fn entries(&self) -> &[Artifact] {
        &self.entries
    }

    pub fn add(&mut self, artifact: Artifact) -> Result<(), String> {
        self.entries.retain(|entry| entry.path != artifact.path);
        self.entries.insert(0, artifact);
        self.entries.truncate(100);
        self.save()
    }

    pub fn remove_entry(&mut self, path: &Path) -> Result<(), String> {
        self.entries.retain(|entry| entry.path != path);
        self.save()
    }

    pub fn replace_entry(&mut self, old_path: &Path, artifact: Artifact) -> Result<(), String> {
        let entry = self
            .entries
            .iter_mut()
            .find(|entry| entry.path == old_path)
            .ok_or("capture is not in history")?;
        *entry = artifact;
        self.save()
    }

    fn save(&self) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let bytes = serde_json::to_vec_pretty(&self.entries).map_err(|e| e.to_string())?;
        fs::write(&self.path, bytes).map_err(|e| e.to_string())
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

pub fn safe_delete(path: &Path, allowed_root: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("Refusing to delete a non-regular capture".into());
    }
    let parent = path
        .parent()
        .ok_or("Capture has no parent directory")?
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let root = allowed_root.canonicalize().map_err(|e| e.to_string())?;
    if !parent.starts_with(root) {
        return Err("Refusing to delete a capture outside the configured output folder".into());
    }
    fs::remove_file(path).map_err(|e| e.to_string())
}

pub fn move_to_trash(
    artifact: &Artifact,
    allowed_root: &Path,
    trash_root: &Path,
) -> Result<Artifact, String> {
    ensure_regular_child(&artifact.path, allowed_root)?;
    fs::create_dir_all(trash_root).map_err(|e| e.to_string())?;
    let extension = artifact
        .path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("capture");
    let destination = trash_root.join(format!("{}.{}", uuid::Uuid::new_v4(), extension));
    move_file(&artifact.path, &destination)?;
    let mut trashed = artifact.clone();
    trashed.original_path = Some(artifact.path.clone());
    trashed.path = destination;
    Ok(trashed)
}

pub fn restore_from_trash(
    artifact: &Artifact,
    allowed_root: &Path,
    trash_root: &Path,
) -> Result<Artifact, String> {
    ensure_regular_child(&artifact.path, trash_root)?;
    let original = artifact
        .original_path
        .as_ref()
        .ok_or("capture is not in trash")?;
    let parent = original.parent().ok_or("original capture has no parent")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let canonical_parent = parent.canonicalize().map_err(|e| e.to_string())?;
    let canonical_root = allowed_root.canonicalize().map_err(|e| e.to_string())?;
    if !canonical_parent.starts_with(canonical_root) {
        return Err("Refusing to restore outside the configured output folder".into());
    }
    let destination = available_restore_path(original);
    move_file(&artifact.path, &destination)?;
    let mut restored = artifact.clone();
    restored.path = destination;
    restored.original_path = None;
    Ok(restored)
}

fn ensure_regular_child(path: &Path, allowed_root: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("Refusing to move a non-regular capture".into());
    }
    let parent = path
        .parent()
        .ok_or("capture has no parent")?
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let root = allowed_root.canonicalize().map_err(|e| e.to_string())?;
    if !parent.starts_with(root) {
        return Err("Refusing to move a capture outside the allowed folder".into());
    }
    Ok(())
}

fn move_file(source: &Path, destination: &Path) -> Result<(), String> {
    fs::rename(source, destination)
        .or_else(|_| {
            fs::copy(source, destination)?;
            fs::remove_file(source)
        })
        .map_err(|e| e.to_string())
}

fn available_restore_path(original: &Path) -> PathBuf {
    if !original.exists() {
        return original.to_owned();
    }
    let parent = original.parent().unwrap_or_else(|| Path::new("."));
    let stem = original
        .file_stem()
        .and_then(|v| v.to_str())
        .unwrap_or("Capture");
    let extension = original.extension().and_then(|v| v.to_str());
    for suffix in 1..1000 {
        let name = match extension {
            Some(extension) => format!("{stem} restored {suffix}.{extension}"),
            None => format!("{stem} restored {suffix}"),
        };
        let candidate = parent.join(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    parent.join(format!("{stem} restored {}", uuid::Uuid::new_v4()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deletion_is_confined_and_rejects_symlink() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let path = outside.path().join("capture.png");
        fs::write(&path, b"x").unwrap();
        assert!(
            safe_delete(&path, root.path())
                .unwrap_err()
                .contains("outside")
        );
    }

    #[test]
    fn trash_and_restore_never_overwrite_a_replacement() {
        let root = tempfile::tempdir().unwrap();
        let output = root.path().join("captures");
        let trash = root.path().join("trash");
        fs::create_dir_all(&output).unwrap();
        let original = output.join("Capture.png");
        fs::write(&original, b"first").unwrap();
        let artifact = Artifact::from_path(original.clone(), 10, 20);

        let trashed = move_to_trash(&artifact, &output, &trash).unwrap();
        assert!(!original.exists());
        assert_eq!(fs::read(&trashed.path).unwrap(), b"first");
        fs::write(&original, b"replacement").unwrap();

        let restored = restore_from_trash(&trashed, &output, &trash).unwrap();
        assert_ne!(restored.path, original);
        assert_eq!(fs::read(&original).unwrap(), b"replacement");
        assert_eq!(fs::read(&restored.path).unwrap(), b"first");
        assert!(!restored.is_trashed());
    }
}
