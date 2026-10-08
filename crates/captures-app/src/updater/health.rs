//! One-shot startup acknowledgement through a helper-owned private file.
use super::Error;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
};

#[derive(Default, Deserialize, Serialize)]
struct RestartIntent {
    restore_preferences: bool,
}

pub(super) fn write_restart_preferences(ready_file: &Path, visible: bool) -> Result<(), Error> {
    let mut file = File::options()
        .write(true)
        .create_new(true)
        .open(ready_file.with_extension("restart.json"))?;
    file.write_all(
        &serde_json::to_vec(&RestartIntent {
            restore_preferences: visible,
        })
        .map_err(|_| Error::Configuration("invalid restart intent"))?,
    )?;
    file.sync_all()?;
    Ok(())
}

/// Consume a helper-owned one-shot visibility snapshot, only after primary
/// election and only for an explicit development health launch. An absent marker
/// preserves ordinary launch routing; an empty legacy marker means tray-only.
/// No installed profile or global marker is discovered. Invalid input is retained.
pub fn take_restart_preferences(ready_file: &Path) -> Result<Option<bool>, Error> {
    if !ready_file.is_absolute() {
        return Err(Error::Configuration(
            "restart intent requires an absolute health file",
        ));
    }
    let path = ready_file.with_extension("restart.json");
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let ready = fs::symlink_metadata(ready_file)?;
    if !metadata.is_file() || metadata.len() > 128 || !ready.is_file() || ready.len() != 0 {
        return Err(Error::Configuration(
            "restart intent requires a small regular marker and empty health file",
        ));
    }
    let mut bytes = Vec::new();
    File::open(&path)?.take(129).read_to_end(&mut bytes)?;
    if bytes.len() > 128 {
        return Err(Error::Configuration(
            "restart intent exceeds its size limit",
        ));
    }
    let intent = if bytes.is_empty() {
        RestartIntent::default()
    } else {
        serde_json::from_slice(&bytes)
            .map_err(|_| Error::Configuration("invalid restart intent"))?
    };
    fs::remove_file(path)?;
    Ok(Some(intent.restore_preferences))
}

/// Used only by an explicitly launched native-development primary after its
/// renderer, persisted settings and packaged media tools initialize. The helper
/// creates an empty regular file in an owned private directory. Neither this
/// acknowledgement nor root-process exit proves that descendants have stopped.
#[derive(Debug)]
pub struct HealthAcknowledgement {
    file: PathBuf,
    token: String,
}

impl HealthAcknowledgement {
    pub fn new(file: PathBuf, token: String) -> Result<Self, Error> {
        let id = uuid::Uuid::parse_str(&token)
            .map_err(|_| Error::Configuration("invalid startup acknowledgement token"))?;
        if !file.is_absolute()
            || id.get_version() != Some(uuid::Version::Random)
            || id.get_variant() != uuid::Variant::RFC4122
            || id.to_string() != token
        {
            return Err(Error::Configuration(
                "startup acknowledgement requires an absolute file and canonical UUIDv4 token",
            ));
        }
        Ok(Self { file, token })
    }

    /// Call only as the elected primary, before deciding which windows to show.
    pub fn take_restart_preferences(&self) -> Result<Option<bool>, Error> {
        take_restart_preferences(&self.file)
    }

    /// Publish exactly UTF-8 `<canonical UUIDv4>\n`, once. The parent retains
    /// its original file handle, so replacing the path cannot acknowledge it.
    pub fn acknowledge(&self) -> Result<(), Error> {
        let metadata = fs::symlink_metadata(&self.file)?;
        if !metadata.is_file() || metadata.len() != 0 {
            return Err(Error::Configuration(
                "startup acknowledgement file must be an empty regular file",
            ));
        }
        let mut file = File::options().write(true).open(&self.file)?;
        if file.metadata()?.len() != 0 {
            return Err(Error::Configuration(
                "startup acknowledgement already written",
            ));
        }
        file.write_all(self.token.as_bytes())?;
        file.write_all(b"\n")?;
        file.flush()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restart_visibility_is_one_shot_and_legacy_empty_means_closed() {
        let directory = tempfile::tempdir().unwrap();
        let ready = directory.path().join("ready");
        fs::write(&ready, []).unwrap();
        for visible in [true, false] {
            write_restart_preferences(&ready, visible).unwrap();
            assert_eq!(
                fs::read(ready.with_extension("restart.json")).unwrap(),
                format!("{{\"restore_preferences\":{visible}}}").as_bytes()
            );
            assert_eq!(take_restart_preferences(&ready).unwrap(), Some(visible));
            assert_eq!(take_restart_preferences(&ready).unwrap(), None);
            assert!(
                fs::read(&ready).unwrap().is_empty(),
                "intent is not health acknowledgement"
            );
        }
        fs::write(ready.with_extension("restart.json"), []).unwrap();
        assert_eq!(take_restart_preferences(&ready).unwrap(), Some(false));
        assert_eq!(take_restart_preferences(&ready).unwrap(), None);
    }

    #[test]
    fn malformed_or_unsafe_restart_intent_is_retained_without_acknowledgement() {
        let directory = tempfile::tempdir().unwrap();
        let ready = directory.path().join("ready");
        fs::write(&ready, []).unwrap();
        let marker = ready.with_extension("restart.json");
        for bytes in [
            b"invalid".to_vec(),
            b"{\"restore_preferences\":1}".to_vec(),
            vec![b' '; 129],
        ] {
            fs::write(&marker, &bytes).unwrap();
            assert!(take_restart_preferences(&ready).is_err());
            assert_eq!(fs::read(&marker).unwrap(), bytes);
            assert!(fs::read(&ready).unwrap().is_empty());
        }
        fs::remove_file(&marker).unwrap();
        fs::create_dir(&marker).unwrap();
        assert!(take_restart_preferences(&ready).is_err());
        assert!(marker.is_dir());
        assert!(take_restart_preferences(Path::new("relative")).is_err());
    }

    #[test]
    fn occupied_health_file_cannot_consume_restart_intent() {
        let directory = tempfile::tempdir().unwrap();
        let ready = directory.path().join("ready");
        fs::write(&ready, b"already acknowledged").unwrap();
        write_restart_preferences(&ready, true).unwrap();
        assert!(take_restart_preferences(&ready).is_err());
        assert_eq!(fs::read(&ready).unwrap(), b"already acknowledged");
        assert_eq!(
            fs::read(ready.with_extension("restart.json")).unwrap(),
            br#"{"restore_preferences":true}"#
        );
    }

    #[cfg(unix)]
    #[test]
    fn linked_marker_or_health_file_is_retained() {
        let directory = tempfile::tempdir().unwrap();
        let ready = directory.path().join("ready");
        let original = directory.path().join("original");
        let marker = ready.with_extension("restart.json");
        fs::write(&ready, []).unwrap();
        fs::write(&original, br#"{"restore_preferences":true}"#).unwrap();
        std::os::unix::fs::symlink(&original, &marker).unwrap();
        assert!(take_restart_preferences(&ready).is_err());
        assert!(fs::symlink_metadata(&marker).unwrap().is_symlink());
        fs::remove_file(&marker).unwrap();
        fs::write(&marker, br#"{"restore_preferences":false}"#).unwrap();
        fs::remove_file(&ready).unwrap();
        fs::write(&original, []).unwrap();
        std::os::unix::fs::symlink(&original, &ready).unwrap();
        assert!(take_restart_preferences(&ready).is_err());
        assert_eq!(
            fs::read(&marker).unwrap(),
            br#"{"restore_preferences":false}"#
        );
        assert!(fs::read(original).unwrap().is_empty());
    }

    #[test]
    fn acknowledgement_is_exact_one_shot_and_rejects_noncanonical_configuration() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("ready");
        fs::write(&path, []).unwrap();
        let token = "73147c85-13e0-4a67-b129-5e7ead486dc1";
        let health = HealthAcknowledgement::new(path.clone(), token.into()).unwrap();
        health.acknowledge().unwrap();
        assert_eq!(fs::read(&path).unwrap(), format!("{token}\n").as_bytes());
        assert!(health.acknowledge().is_err());
        assert_eq!(fs::read(&path).unwrap(), format!("{token}\n").as_bytes());
        assert!(HealthAcknowledgement::new("relative".into(), token.into()).is_err());
        for invalid in [
            token.to_uppercase(),
            token.replace('-', ""),
            token.replace("b129", "7129"),
            uuid::Uuid::nil().to_string(),
        ] {
            assert!(HealthAcknowledgement::new(path.clone(), invalid).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn acknowledgement_never_writes_through_a_link() {
        let directory = tempfile::tempdir().unwrap();
        let original = directory.path().join("original");
        fs::write(&original, []).unwrap();
        let link = directory.path().join("link");
        std::os::unix::fs::symlink(&original, &link).unwrap();
        let health = HealthAcknowledgement::new(link, uuid::Uuid::new_v4().to_string()).unwrap();
        assert!(health.acknowledge().is_err());
        assert!(fs::read(original).unwrap().is_empty());
    }
}
