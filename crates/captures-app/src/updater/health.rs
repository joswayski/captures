//! One-shot startup acknowledgement through a helper-owned private file.
use super::Error;
use std::{
    fs::{self, File},
    io::Write,
    path::PathBuf,
};

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
