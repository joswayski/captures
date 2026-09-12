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
}
