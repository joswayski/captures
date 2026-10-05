//! Explicit handoff to a development host using a new disposable profile.
use super::*;
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
        let profile = self.launch_profile(profile)?;
        check_cancel(cancel)?;
        let channel = tempfile::Builder::new()
            .prefix(".captures-native-health-")
            .tempdir_in(self.paths.package.parent().unwrap())?;
        let mut ready = tempfile::NamedTempFile::new_in(channel.path())?;
        let token = uuid::Uuid::new_v4().to_string();
        let expected = format!("{token}\n").into_bytes();
        let log = File::options()
            .write(true)
            .create_new(true)
            .open(profile.join("startup.log"))?;
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

    fn launch_profile(&self, profile: &Path) -> Result<PathBuf, Error> {
        if !profile.is_absolute() || !fs::symlink_metadata(profile)?.is_dir() {
            return Err(Error::Configuration(
                "startup requires an absolute empty profile directory",
            ));
        }
        let profile = fs::canonicalize(profile)?;
        if [
            &self.paths.package,
            &self.paths.transaction,
            &self.paths.garbage(),
        ]
        .iter()
        .any(|tree| profile.starts_with(tree))
            || fs::read_dir(&profile)?.next().is_some()
        {
            return Err(Error::Configuration(
                "startup profile must be empty and outside package trees",
            ));
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
