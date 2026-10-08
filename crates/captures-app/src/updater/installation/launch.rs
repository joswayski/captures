//! Explicit handoff to a development host using an isolated development profile.
use super::*;
use crate::profile_import::PreparedDevelopmentProfile;
use std::{
    io::{Seek, SeekFrom},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

/// Failed handoff with ownership of the root process, if it was started.
/// An exited or killed root does not prove descendant quiescence. The package
/// transaction is retained; stop ALL app processes before explicit recovery.
#[derive(Debug, thiserror::Error)]
#[error("{error}")]
pub struct LaunchFailure {
    pub error: Error,
    pub process: Option<Child>,
}

impl From<Error> for LaunchFailure {
    fn from(error: Error) -> Self {
        Self {
            error,
            process: None,
        }
    }
}

impl From<std::io::Error> for LaunchFailure {
    fn from(error: std::io::Error) -> Self {
        Self::from(Error::Io(error))
    }
}

impl PendingInstallation {
    /// Launch the validated replacement directly, with a new empty disposable
    /// profile outside every package/transaction tree, and confirm only after
    /// its private one-shot startup acknowledgement. The helper must run outside
    /// the package and the caller must exclude other app launches throughout.
    /// This does not reuse/import an existing profile or enable a release channel.
    ///
    /// The replacement lock stays held through acknowledgement and confirmation.
    /// Startup failure retains the backup. Once a target has started, no automatic
    /// rollback occurs: even successful root exit cannot prove children stopped.
    /// A late acknowledgement after failure cannot confirm the installation.
    /// Confirmation/cleanup errors can leave a running host and an already
    /// committed cleanup decision, not a restorable backup. Inspect the transaction
    /// and stop all app processes before recovery. Dropping a returned
    /// Child does not stop it. Its stdout/stderr go to `profile/startup.log`.
    pub fn launch(
        self,
        profile: &Path,
        timeout: Duration,
        cancel: &CancelToken,
    ) -> Result<Child, LaunchFailure> {
        self.launch_with_sources(profile, None, None, timeout, cancel)
    }

    /// Import explicit shipping sources into a NONEXISTENT development profile,
    /// then use the same startup health protocol as the empty-profile handoff.
    /// All sources/profile paths must stay outside package/transaction trees.
    /// All source writers must be stopped and excluded throughout. No installed
    /// path is discovered and the shipping sources remain unchanged.
    ///
    /// A published copy is retained even if startup fails. Package recovery never
    /// deletes/restores profile data; the source snapshot and unchanged shipping
    /// profile remain available. A repeated attempt requires another new profile.
    pub fn launch_importing(
        self,
        source_settings_file: &Path,
        source_data_directory: &Path,
        new_profile: &Path,
        timeout: Duration,
        cancel: &CancelToken,
    ) -> Result<Child, LaunchFailure> {
        self.launch_with_sources(
            new_profile,
            Some((source_settings_file, source_data_directory)),
            None,
            timeout,
            cancel,
        )
    }

    /// Reuse an explicitly prepared development profile, preserving its working
    /// settings/data. Prepare its retained snapshot BEFORE package replacement;
    /// exclude every profile writer and app launch throughout. This never enrolls
    /// arbitrary/installed profiles. Recheck both working and snapshot bytes
    /// before spawning, then require the same one-shot live health acknowledgement.
    /// Failure retains the snapshot and package transaction; package recovery
    /// does not roll back data migrations. Logs use a fresh `startup-UUID.log`.
    pub fn launch_existing(
        self,
        profile: &PreparedDevelopmentProfile,
        timeout: Duration,
        cancel: &CancelToken,
    ) -> Result<Child, LaunchFailure> {
        self.launch_with_sources(profile.root(), None, Some(profile), timeout, cancel)
    }

    fn launch_with_sources(
        self,
        profile: &Path,
        sources: Option<(&Path, &Path)>,
        existing: Option<&PreparedDevelopmentProfile>,
        timeout: Duration,
        cancel: &CancelToken,
    ) -> Result<Child, LaunchFailure> {
        let _lock = self.paths.lock()?;
        let receipt = self.paths.receipt()?;
        require_id(&receipt, Some(self.id))?;
        require_fingerprint(&self.paths.package, receipt.target, &receipt.new_hash)?;
        if self.paths.confirmed()? {
            return Err(Error::Installation("replacement already confirmed").into());
        }
        if timeout.is_zero() || timeout > Duration::from_secs(120) {
            return Err(Error::Configuration("startup deadline must be within 120 seconds").into());
        }
        check_cancel(cancel)?;
        let profile = self.launch_profile(profile, sources, existing, cancel)?;
        check_cancel(cancel)?;
        let channel = tempfile::Builder::new()
            .prefix(".captures-native-health-")
            .tempdir_in(self.paths.package.parent().unwrap())?;
        let mut ready = tempfile::NamedTempFile::new_in(channel.path())?;
        let token = uuid::Uuid::new_v4().to_string();
        let expected = format!("{token}\n").into_bytes();
        if let Some(visible) = self.restore_preferences {
            super::super::health::write_restart_preferences(ready.path(), visible)?;
        }
        let log = File::options()
            .write(true)
            .create_new(true)
            .open(profile.join(if existing.is_some() {
                format!("startup-{}.log", uuid::Uuid::new_v4())
            } else {
                "startup.log".into()
            }))?;
        let mut child = Command::new(
            self.paths
                .package
                .join(self.info.target.package_executable()),
        )
        .arg("--live")
        .arg("--history-root")
        .arg(profile.join("history"))
        .arg("--settings-file")
        .arg(profile.join("settings.json"))
        .arg("--native-update-ready-file")
        .arg(ready.path())
        .arg("--native-update-ready-token")
        .arg(token)
        .env_remove("CAPTURES_FFMPEG")
        .env_remove("CAPTURES_FFPROBE")
        // A disposable handoff must never take over the desktop's OS keys.
        .env("CAPTURES_NATIVE_SKIP_SYSTEM_SHORTCUT_TAKEOVER", "1")
        .current_dir(&profile)
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .spawn()
        .map_err(Error::Io)?;
        let result = self
            .wait_for_startup(&mut child, ready.as_file_mut(), &expected, timeout, cancel)
            .and_then(|()| {
                if self.restore_preferences.is_some() {
                    match fs::symlink_metadata(ready.path().with_extension("restart.json")) {
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                        Err(error) => return Err(error.into()),
                        Ok(_) => {
                            return Err(Error::Installation("host did not consume restart intent"));
                        }
                    }
                }
                self.confirm_locked(|| {
                    check_cancel(cancel)?;
                    if child.try_wait()?.is_some() {
                        Err(Error::Installation(
                            "host exited before startup confirmation committed",
                        ))
                    } else {
                        Ok(())
                    }
                })
            });
        match result {
            Ok(()) => Ok(child),
            Err(error) => Err(LaunchFailure {
                error,
                process: Some(child),
            }),
        }
    }

    fn launch_profile(
        &self,
        profile: &Path,
        sources: Option<(&Path, &Path)>,
        existing: Option<&PreparedDevelopmentProfile>,
        cancel: &CancelToken,
    ) -> Result<PathBuf, Error> {
        if !profile.is_absolute() {
            return Err(Error::Configuration(
                "startup requires an explicit absolute development profile",
            ));
        }
        let profile = if sources.is_some() {
            let name = profile
                .file_name()
                .ok_or(Error::Configuration("invalid new profile path"))?;
            profile
                .parent()
                .ok_or(Error::Configuration("invalid new profile parent"))?
                .canonicalize()?
                .join(name)
        } else {
            if !fs::symlink_metadata(profile)?.is_dir() {
                return Err(Error::Configuration(
                    "startup requires an empty profile directory",
                ));
            }
            fs::canonicalize(profile)?
        };
        let trees = [
            &self.paths.package,
            &self.paths.transaction,
            &self.paths.garbage(),
        ];
        if trees.iter().any(|tree| {
            profile.starts_with(tree)
                || existing.is_some_and(|existing| {
                    tree.starts_with(&profile)
                        || existing.snapshot().starts_with(tree)
                        || tree.starts_with(existing.snapshot())
                })
        }) {
            return Err(Error::Configuration(
                "startup profile must be outside package trees",
            ));
        }
        if let Some(existing) = existing {
            existing.verify(cancel)?;
        } else if let Some((settings, data)) = sources {
            for source in [settings, data] {
                if !source.is_absolute() {
                    return Err(Error::Configuration(
                        "import sources must be explicit absolute paths",
                    ));
                }
                let source = source.canonicalize()?;
                if trees
                    .iter()
                    .any(|tree| source.starts_with(tree) || tree.starts_with(&source))
                {
                    return Err(Error::Configuration(
                        "import sources must be outside package trees",
                    ));
                }
            }
            crate::profile_import::import_shipping_profile(settings, data, &profile, cancel)?;
        } else if fs::read_dir(&profile)?.next().is_some() {
            return Err(Error::Configuration("startup profile must be empty"));
        } else {
            crate::profile_import::mark_development_profile(&profile, &profile)?;
        }
        Ok(profile)
    }

    fn wait_for_startup(
        &self,
        child: &mut Child,
        ready: &mut File,
        expected: &[u8],
        timeout: Duration,
        cancel: &CancelToken,
    ) -> Result<(), Error> {
        let deadline = Instant::now() + timeout;
        loop {
            check_cancel(cancel)?;
            if Instant::now() >= deadline {
                return Err(Error::Installation(
                    "host startup was not confirmed before its deadline",
                ));
            }
            if child.try_wait()?.is_some() {
                return Err(Error::Installation(
                    "host exited without a live startup acknowledgement",
                ));
            }
            ready.seek(SeekFrom::Start(0))?;
            let mut bytes = Vec::new();
            (&mut *ready)
                .take(expected.len() as u64 + 1)
                .read_to_end(&mut bytes)?;
            if bytes.len() > expected.len() || !expected.starts_with(&bytes) {
                return Err(Error::Installation("invalid startup acknowledgement"));
            }
            if bytes == expected {
                // Readiness is not a license to confirm an already-exited host.
                if child.try_wait()?.is_none() && Instant::now() < deadline {
                    return Ok(());
                }
                return Err(Error::Installation(
                    "host exited during startup acknowledgement",
                ));
            }
            thread::sleep(
                Duration::from_millis(20).min(deadline.saturating_duration_since(Instant::now())),
            );
        }
    }
}
