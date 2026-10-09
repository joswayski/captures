//! Development-only post-drain GUI intent. A supervised installation request
//! additionally binds its parent's private per-launch nonce. Ordinary Quit never
//! authorizes replacement; the external helper authenticates the artifact again.
use super::{
    DEVELOPMENT_IDENTITY, Error, PendingUpdate, ReleaseInfo, checks::CheckStatus, decode_hash,
};
use crate::profile_import::validate_development_profile;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

const LIMIT: u64 = 256 * 1024;

#[derive(Debug)]
pub struct ShutdownIntentDestination {
    path: PathBuf,
    profile: PathBuf,
}

impl ShutdownIntentDestination {
    /// Explicit opt-in, outside an enrolled profile, with its exact settings and
    /// History roots. Reads bounded metadata only; never enrolls/migrates settings.
    /// The output must not already exist. Parents must remain trusted.
    pub fn new(path: &Path, history: &Path, settings: &Path) -> Result<Self, Error> {
        let invalid = || {
            Error::Configuration(
                "shutdown intent requires a new absolute file outside an enrolled development profile with its exact settings/History paths",
            )
        };
        if !path.is_absolute() || !history.is_absolute() || !settings.is_absolute() {
            return Err(invalid());
        }
        let profile = validate_development_profile(history.parent().ok_or_else(invalid)?)
            .map_err(|_| invalid())?;
        // Windows canonical roots use the extended path prefix. Compare the
        // actual parent roots, not their caller-provided spelling. History can
        // legitimately be absent before the first capture.
        if history.file_name() != Some(std::ffi::OsStr::new("history"))
            || settings.file_name() != Some(std::ffi::OsStr::new("settings.json"))
            || settings.parent().ok_or_else(invalid)?.canonicalize()? != profile
        {
            return Err(invalid());
        }
        let parent = path.parent().ok_or_else(invalid)?;
        if !fs::symlink_metadata(parent)?.is_dir() {
            return Err(invalid());
        }
        let path = parent
            .canonicalize()?
            .join(path.file_name().ok_or_else(invalid)?);
        if path.starts_with(&profile) {
            return Err(invalid());
        }
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
            Ok(_) => return Err(invalid()),
        }
        Ok(Self { path, profile })
    }

    /// Publish only AFTER all accepted work/settings and update scratch drain,
    /// before releasing profile election. The host snapshots visibility/status
    /// after an accepted editor drain and before hiding/cancelling its windows.
    /// Non-staged states publish nothing. Atomic no-clobber, private on Unix.
    pub fn write(
        &self,
        status: &CheckStatus,
        preferences_visible: bool,
        install_session: Option<uuid::Uuid>,
    ) -> Result<bool, Error> {
        let CheckStatus::Staged { release, sha256 } = status else {
            return Ok(false);
        };
        decode_hash(sha256)?;
        if validate_development_profile(&self.profile).map_err(|_| {
            Error::Configuration("development profile changed before shutdown intent publication")
        })? != self.profile
        {
            return Err(Error::Configuration(
                "development profile changed before shutdown intent publication",
            ));
        }
        let record = ShutdownIntent {
            schema: 1,
            identity: DEVELOPMENT_IDENTITY.into(),
            profile: self.profile.clone(),
            release: release.clone(),
            sha256: sha256.clone(),
            preferences_visible,
            install_session,
        };
        let bytes = serde_json::to_vec(&record).map_err(std::io::Error::other)?;
        if bytes.len() as u64 > LIMIT {
            return Err(Error::MetadataTooLarge);
        }
        use std::io::Write;
        let mut file = tempfile::NamedTempFile::new_in(self.path.parent().unwrap())?;
        file.write_all(&bytes)?;
        file.as_file().sync_all()?;
        file.persist_noclobber(&self.path)
            .map_err(|error| Error::Io(error.error))?;
        Ok(true)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShutdownIntent {
    schema: u8,
    identity: String,
    profile: PathBuf,
    release: ReleaseInfo,
    sha256: String,
    preferences_visible: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    install_session: Option<uuid::Uuid>,
}

impl ShutdownIntent {
    /// An ordinary Quit, legacy record or missing record makes no installation
    /// request. This checks only private operator intent, never authenticates it.
    pub fn install_requested(
        path: &Path,
        profile: &Path,
        session: uuid::Uuid,
    ) -> Result<bool, Error> {
        match fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            result => {
                result?;
            }
        }
        let record = Self::read(path, profile)?;
        match record.install_session {
            None => Ok(false),
            Some(actual) if actual == session => Ok(true),
            Some(_) => Err(Error::Configuration(
                "installation request belongs to another GUI launch",
            )),
        }
    }

    /// Consume once, only for the explicitly selected enrolled profile and the
    /// freshly AUTHENTICATED full target digest/size/version/renderer/host target.
    /// The file is operator intent, NOT authentication or proof of stopped writers.
    /// Mismatches stay untouched and must never trigger replacement/launch.
    pub fn take_matching(
        path: &Path,
        profile: &Path,
        update: &PendingUpdate,
    ) -> Result<bool, Error> {
        Self::take(path, profile, update, None)
    }

    /// The actual supervising parent calls only after a successful root wait
    /// and a complete installation drain. A normal visibility record is rejected.
    pub fn take_install_matching(
        path: &Path,
        profile: &Path,
        update: &PendingUpdate,
        session: uuid::Uuid,
    ) -> Result<bool, Error> {
        Self::take(path, profile, update, Some(session))
    }

    fn take(
        path: &Path,
        profile: &Path,
        update: &PendingUpdate,
        session: Option<uuid::Uuid>,
    ) -> Result<bool, Error> {
        let record = Self::read(path, profile)?;
        let info = update.info();
        if session.is_some_and(|expected| record.install_session != Some(expected))
            || record.release.renderer != info.renderer
            || record.release.target != info.target
            || record.release.version != info.version
            || record.release.size != info.size
            || decode_hash(&record.sha256)? != update.hash
        {
            return Err(Error::Configuration(
                "shutdown intent does not match this authenticated update and GUI launch",
            ));
        }
        fs::remove_file(path)?;
        Ok(record.preferences_visible)
    }

    fn read(path: &Path, profile: &Path) -> Result<Self, Error> {
        let invalid = || {
            Error::Configuration("shutdown intent does not match this enrolled development profile")
        };
        if !path.is_absolute() {
            return Err(invalid());
        }
        let metadata = fs::symlink_metadata(path)?;
        if !metadata.is_file() || metadata.len() > LIMIT {
            return Err(invalid());
        }
        let mut bytes = Vec::new();
        fs::File::open(path)?
            .take(LIMIT + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > LIMIT {
            return Err(invalid());
        }
        let record: Self = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
        if record.schema != 1
            || record.identity != DEVELOPMENT_IDENTITY
            || record.profile != validate_development_profile(profile).map_err(|_| invalid())?
        {
            return Err(invalid());
        }
        Ok(record)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::updater::{
        Renderer, Target, UpdateClient,
        tests::{manifest, signed},
    };

    #[test]
    fn staged_shutdown_intent_is_private_no_clobber_exact_target_and_one_shot() {
        let root = tempfile::tempdir().unwrap();
        let profile = root.path().join("profile");
        fs::create_dir(&profile).unwrap();
        let profile = profile.canonicalize().unwrap();
        fs::write(
            profile.join("settings.json"),
            serde_json::to_vec(&captures_settings::AppSettings::default()).unwrap(),
        )
        .unwrap();
        crate::profile_import::mark_development_profile(&profile, &profile).unwrap();
        let output = root.path().join("intent.json");
        let destination = ShutdownIntentDestination::new(
            &output,
            &root.path().join("profile/history"),
            &root.path().join("profile/settings.json"),
        )
        .unwrap();
        let value = manifest("https://example.invalid/package");
        let (key, bytes, signature) = signed(&value);
        let client = UpdateClient::new(
            "https://example.invalid/manifest",
            &key,
            Renderer::Wgpu,
            Target::LinuxX64,
            "2026.9.99",
        )
        .unwrap();
        let update = client.authenticate(&bytes, &signature).unwrap().unwrap();
        let status = CheckStatus::Staged {
            release: update.info().clone(),
            sha256: update.sha256(),
        };
        for status in [
            CheckStatus::Idle,
            CheckStatus::Available {
                release: update.info().clone(),
            },
            CheckStatus::Downloading {
                release: update.info().clone(),
                downloaded: 0,
                total: update.info().size,
            },
            CheckStatus::Verifying {
                release: update.info().clone(),
            },
            CheckStatus::Cancelling {
                release: update.info().clone(),
            },
            CheckStatus::DownloadError {
                release: update.info().clone(),
                message: "failed".into(),
            },
        ] {
            assert!(!destination.write(&status, true, None).unwrap());
            assert!(!output.exists());
        }
        for visible in [true, false] {
            assert!(destination.write(&status, visible, None).unwrap());
            let original = fs::read(&output).unwrap();
            assert!(destination.write(&status, !visible, None).is_err());
            assert_eq!(fs::read(&output).unwrap(), original);
            let session = uuid::Uuid::new_v4();
            assert!(!ShutdownIntent::install_requested(&output, &profile, session).unwrap());
            assert!(
                ShutdownIntent::take_install_matching(&output, &profile, &update, session).is_err()
            );
            assert_eq!(
                fs::read(&output).unwrap(),
                original,
                "ordinary Quit cannot authorize installation"
            );
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    fs::metadata(&output).unwrap().permissions().mode() & 0o777,
                    0o600
                );
            }
            for (field, changed) in [
                ("/schema", serde_json::json!(2)),
                ("/identity", serde_json::json!("es.captur.installed")),
                ("/profile", serde_json::json!(root.path())),
                ("/sha256", serde_json::json!("01".repeat(32))),
                ("/release/size", serde_json::json!(update.info().size + 1)),
                ("/release/version", serde_json::json!("2026.10.51")),
                ("/release/renderer", serde_json::json!("appkit")),
                (
                    "/release/target",
                    serde_json::json!("x86_64-pc-windows-msvc"),
                ),
            ] {
                let mut altered: serde_json::Value = serde_json::from_slice(&original).unwrap();
                *altered.pointer_mut(field).unwrap() = changed;
                fs::write(&output, serde_json::to_vec(&altered).unwrap()).unwrap();
                assert!(ShutdownIntent::take_matching(&output, &profile, &update).is_err());
                assert!(output.exists(), "mismatch must remain inspectable");
            }
            fs::write(&output, &original).unwrap();
            assert_eq!(
                ShutdownIntent::take_matching(&output, &profile, &update).unwrap(),
                visible
            );
            assert!(ShutdownIntent::take_matching(&output, &profile, &update).is_err());
        }
        let session = uuid::Uuid::new_v4();
        assert!(!ShutdownIntent::install_requested(&output, &profile, session).unwrap());
        assert!(destination.write(&status, false, Some(session)).unwrap());
        let original = fs::read(&output).unwrap();
        let other = uuid::Uuid::new_v4();
        assert!(ShutdownIntent::install_requested(&output, &profile, other).is_err());
        assert!(ShutdownIntent::take_install_matching(&output, &profile, &update, other).is_err());
        assert_eq!(fs::read(&output).unwrap(), original);
        assert!(ShutdownIntent::install_requested(&output, &profile, session).unwrap());
        let mut altered: serde_json::Value = serde_json::from_slice(&original).unwrap();
        altered["sha256"] = serde_json::json!("02".repeat(32));
        fs::write(&output, serde_json::to_vec(&altered).unwrap()).unwrap();
        assert!(
            ShutdownIntent::take_install_matching(&output, &profile, &update, session).is_err()
        );
        assert!(output.exists());
        fs::write(&output, original).unwrap();
        assert!(
            !ShutdownIntent::take_install_matching(&output, &profile, &update, session).unwrap()
        );
        assert!(!output.exists());
        assert!(
            ShutdownIntentDestination::new(
                &profile.join("intent"),
                &profile.join("history"),
                &profile.join("settings.json")
            )
            .is_err()
        );
        assert!(
            ShutdownIntentDestination::new(
                &output,
                &profile.join("history"),
                &root.path().join("shipping-settings.json")
            )
            .is_err()
        );
        fs::remove_file(profile.join(".captures-native-development-profile.json")).unwrap();
        assert!(
            ShutdownIntentDestination::new(
                &output,
                &profile.join("history"),
                &profile.join("settings.json")
            )
            .is_err()
        );
    }
}
