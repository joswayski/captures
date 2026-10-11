//! Startup-only companion: no renderer, settings load, History or migration.
//! The GUI keeps its OS app identity. Only this copied helper survives a failed
//! launch; it never launches a second GUI while any package-use guard is held.
use super::*;
use std::{fs::File, time::Instant};

#[cfg(test)]
mod tests;

const STARTUP_TIMEOUT: Duration = Duration::from_secs(120);
const EXIT_TIMEOUT: Duration = Duration::from_secs(600);
const RETRY_INTERVAL: Duration = Duration::from_secs(60);
const ATTEMPTS: usize = 30;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    version: String,
    installation: InstallTarget,
    fingerprint: Option<String>,
    health: HealthAcknowledgement,
}

/// Retain until normal shutdown. Abandoning this owner without `finish` is a
/// startup failure unless the real settings/renderer/tools gate acknowledged.
pub struct Startup {
    _lifetime: File,
    directory: PathBuf,
    pub acknowledgement: HealthAcknowledgement,
}

impl Startup {
    /// Called by installed hosts before profile import. Failures here must not
    /// prevent an otherwise working app from starting. Development is a no-op.
    pub fn start(health: Option<HealthAcknowledgement>) -> Result<Option<Self>, Error> {
        let Some(version) = crate::preview_channel::version() else {
            return Ok(None);
        };
        let root = root()?;
        fs::create_dir_all(&root)?;
        prune_completed(&root);
        let directory = tempfile::Builder::new().prefix("watch-").tempdir_in(root)?;
        let lifetime = new_file(&directory.path().join("host.lock"))?;
        lifetime.try_lock().map_err(|_| Error::Package)?;
        let health = match health {
            Some(health) => health,
            None => {
                let ready = directory.path().join("ready");
                new_file(&ready)?;
                HealthAcknowledgement::new(ready, uuid::Uuid::new_v4().to_string())?
            }
        };
        let installation = InstallTarget::current()?;
        let request = Request {
            version: version.into(),
            fingerprint: fingerprint(&installation)?,
            installation,
            health: health.clone(),
        };
        let request_path = directory.path().join("watch.json");
        write_json(&request_path, &request)?;
        let name = if cfg!(windows) {
            "native_preview_update_helper.exe"
        } else {
            "native_preview_update_helper"
        };
        let helper = directory.path().join(name);
        fs::copy(std::env::current_exe()?.with_file_name(name), &helper)?;
        let log = new_file(&directory.path().join("recovery.log"))?;
        let mut child = Command::new(helper)
            .arg("--watch-startup")
            .arg(request_path)
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .spawn()?;
        // Confirm the separate process has retained the original lock/ready
        // handles before allowing migration to fail or the GUI to crash.
        let started = Instant::now();
        while !directory.path().join("watching").is_file() {
            if child.try_wait()?.is_some() || started.elapsed() > Duration::from_secs(5) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Error::Installation("startup recovery helper did not start"));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(Some(Self {
            _lifetime: lifetime,
            directory: directory.keep(),
            acknowledgement: health,
        }))
    }

    /// Successful secondary forwarding and normal Quit are not failed launches.
    pub fn finish(&self) {
        let _ = fs::write(self.directory.join("finished"), b"normal exit\n");
    }
}

fn root() -> Result<PathBuf, Error> {
    Ok(crate::preview_channel::profile_path()
        .map_err(|_| Error::Package)?
        .parent()
        .ok_or(Error::Package)?
        .join("startup-recovery"))
}

fn new_file(path: &Path) -> Result<File, Error> {
    Ok(File::options()
        .read(true)
        .write(true)
        .create_new(true)
        .open(path)?)
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), Error> {
    let mut file = new_file(path)?;
    serde_json::to_writer(&mut file, value).map_err(|_| Error::Manifest)?;
    file.sync_all()?;
    Ok(())
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, Error> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.len() > 32 * 1024 {
        return Err(Error::Manifest);
    }
    let bytes = fs::read(path)?;
    serde_json::from_slice(&bytes).map_err(|_| Error::Manifest)
}

fn prune_completed(root: &Path) {
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().starts_with("watch-")
                && entry.file_type().is_ok_and(|kind| kind.is_dir())
                && entry.path().join("completed").is_file()
            {
                let _ = fs::remove_dir_all(entry.path());
            }
        }
    }
}

struct Monitor {
    lifetime: File,
    ready: File,
    expected: Vec<u8>,
    directory: PathBuf,
    exited: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum State {
    Starting,
    Finished,
    Failed,
}

impl Monitor {
    fn open(directory: &Path, health: &HealthAcknowledgement) -> Result<Self, Error> {
        HealthAcknowledgement::new(health.file.clone(), health.token.clone())?;
        for path in [directory.join("host.lock"), health.file.clone()] {
            if !fs::symlink_metadata(path)?.is_file() {
                return Err(Error::Package);
            }
        }
        Ok(Self {
            lifetime: File::options()
                .read(true)
                .write(true)
                .open(directory.join("host.lock"))?,
            ready: File::open(&health.file)?,
            expected: format!("{}\n", health.token).into_bytes(),
            directory: directory.into(),
            exited: false,
        })
    }

    fn state(&mut self) -> Result<State, Error> {
        self.ready.seek(SeekFrom::Start(0))?;
        let mut bytes = Vec::new();
        Read::by_ref(&mut self.ready)
            .take(128)
            .read_to_end(&mut bytes)?;
        if bytes == self.expected || self.directory.join("finished").is_file() {
            return Ok(State::Finished);
        }
        if self.exited {
            return Ok(State::Failed);
        }
        match self.lifetime.try_lock() {
            Ok(()) => {
                self.exited = true;
                Ok(State::Failed)
            }
            Err(fs::TryLockError::WouldBlock) => Ok(State::Starting),
            Err(fs::TryLockError::Error(error)) => Err(error.into()),
        }
    }
}

/// Helper entry point, separate from the application and copied out of its
/// install directory. Configuration never accepts an alternate endpoint/key.
pub fn watch(request_path: &Path) -> Result<(), Error> {
    let request: Request = read_json(request_path)?;
    if Some(request.version.as_str()) != crate::preview_channel::version() {
        return Err(Error::Version);
    }
    let directory = request_path.parent().ok_or(Error::Package)?;
    let mut monitor = Monitor::open(directory, &request.health)?;
    fs::write(directory.join("watching"), b"watching\n")?;
    let started = Instant::now();
    loop {
        match monitor.state()? {
            State::Finished => return complete(directory),
            State::Failed => break,
            State::Starting if started.elapsed() < STARTUP_TIMEOUT => {}
            State::Starting => {
                notice(
                    "Captures has not finished starting.\n\nQuit or Force Quit Captures to let automatic repair continue.\nYour captures will not be deleted.",
                );
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let result = repair(&request, &mut monitor, directory);
    if let Err(error) = &result {
        eprintln!("Startup recovery: {error}");
        status(
            directory,
            "manual-repair-required",
            Some(&error.to_string()),
        )?;
        notice(
            "Captures could not repair this startup.\n\nDownload the latest Preview from https://captur.es\nand replace the app.\n\nKeep your settings and capture history; do not delete them.",
        );
    }
    result
}

fn repair(request: &Request, monitor: &mut Monitor, directory: &Path) -> Result<(), Error> {
    let root = directory.parent().ok_or(Error::Package)?;
    let lock_path = root.join("repair.lock");
    if lock_path.exists() && !fs::symlink_metadata(&lock_path)?.is_file() {
        return Err(Error::Package);
    }
    let lock = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)?;
    let started = Instant::now();
    loop {
        if monitor.state()? == State::Finished {
            return complete(directory);
        }
        match lock.try_lock() {
            Ok(()) => break,
            Err(fs::TryLockError::WouldBlock) if started.elapsed() < EXIT_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(100));
            }
            _ => return Err(Error::Installation("another recovery is still running")),
        }
    }
    // Never terminate an importing or hung process: it could be writing data.
    // Never launch a second GUI. OS file locks, not PID reuse or elapsed time,
    // decide when the failed host is actually gone.
    while monitor.state()? == State::Starting {
        status(directory, "waiting-for-app-exit", None)?;
        if started.elapsed() > EXIT_TIMEOUT {
            return Err(Error::Installation(
                "Captures is still running; nothing was replaced",
            ));
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    if monitor.state()? == State::Finished {
        return complete(directory);
    }
    if !request.installation.installable() {
        return Err(Error::Installation(
            "Debian packages require the manual installer",
        ));
    }
    let client = Client::new(
        &ENDPOINTS,
        crate::preview_channel::public_key().ok_or(Error::Signature)?,
        if cfg!(target_os = "macos") {
            Renderer::Appkit
        } else {
            Renderer::Wgpu
        },
        Target::current_host().ok_or(Error::Target)?,
        &request.version,
        request.installation.clone(),
    )?;
    acquire_fix(
        request,
        directory,
        &client,
        ATTEMPTS,
        RETRY_INTERVAL,
        install,
    )
}

fn acquire_fix(
    request: &Request,
    directory: &Path,
    client: &Client,
    attempts: usize,
    retry_interval: Duration,
    install: impl Fn(&Path) -> Result<(), Error>,
) -> Result<(), Error> {
    let root = directory.parent().ok_or(Error::Package)?;
    let expected = request.fingerprint.as_deref().ok_or(Error::Package)?;
    let cancel = CancelToken::default();
    for attempt in 0..attempts {
        require_unchanged(&request.installation, expected)?;
        status(directory, "checking-for-fix", None)?;
        match client.check(&cancel) {
            Ok(Some(pending)) => {
                let attempted = root.join("attempted-version.json");
                if !new_attempt(&attempted, &pending.info.version)? {
                    return Err(Error::Installation(
                        "this repair version was already attempted",
                    ));
                }
                status(directory, "downloading-fix", Some(&pending.info.version))?;
                match pending.download(directory, &cancel, |_, _| {}) {
                    Ok(mut download) => {
                        download.request.recovery = Some(InstallationCheck {
                            fingerprint: expected.into(),
                            attempted,
                        });
                        let install_request = download.directory.path().join("install.json");
                        write_json(&install_request, &download.request)?;
                        let _ = download.directory.keep();
                        status(directory, "installing-fix", Some(&pending.info.version))?;
                        install(&install_request)?;
                        status(directory, "installed", Some(&pending.info.version))?;
                        return complete(directory);
                    }
                    Err(error) => status(directory, "download-failed", Some(&error.to_string()))?,
                }
            }
            Ok(None) => status(directory, "waiting-for-newer-preview", None)?,
            Err(error) => status(directory, "check-failed", Some(&error.to_string()))?,
        }
        if attempt + 1 < attempts {
            std::thread::sleep(retry_interval);
        }
    }
    Err(Error::Installation(
        "no installable fix found within the retry budget; reopen Captures to retry",
    ))
}

fn complete(directory: &Path) -> Result<(), Error> {
    fs::write(directory.join("completed"), b"finished\n")?;
    Ok(())
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct InstallationCheck {
    fingerprint: String,
    attempted: PathBuf,
}

impl InstallationCheck {
    /// Called only after signature verification and exclusive package ownership.
    /// A busy app or changed package must not consume the repair attempt.
    pub(super) fn begin(&self, installation: &InstallTarget, version: &str) -> Result<(), Error> {
        require_unchanged(installation, &self.fingerprint)?;
        if !new_attempt(&self.attempted, version)? {
            return Err(Error::Installation(
                "this repair version was already attempted",
            ));
        }
        replace_json(&self.attempted, &version)
    }
}

fn new_attempt(path: &Path, version: &str) -> Result<bool, Error> {
    let next = Version::parse(version).map_err(|_| Error::Version)?;
    if !path.try_exists()? {
        return Ok(true);
    }
    let previous: String = read_json(path)?;
    Ok(next
        .cmp_precedence(&Version::parse(&previous).map_err(|_| Error::Version)?)
        .is_gt())
}

fn replace_json(path: &Path, value: &impl Serialize) -> Result<(), Error> {
    let mut temporary = tempfile::NamedTempFile::new_in(path.parent().ok_or(Error::Package)?)?;
    serde_json::to_writer(&mut temporary, value).map_err(|_| Error::Manifest)?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(path)
        .map_err(|error| Error::Io(error.error))?;
    Ok(())
}

fn status(directory: &Path, phase: &str, detail: Option<&str>) -> Result<(), Error> {
    eprintln!("Startup recovery: {phase} {}", detail.unwrap_or_default());
    replace_json(
        &directory.join("status.json"),
        &serde_json::json!({"phase": phase, "detail": detail}),
    )
}

fn fingerprint(installation: &InstallTarget) -> Result<Option<String>, Error> {
    let path = match installation {
        InstallTarget::Mac { app } => app.join("Contents/MacOS/captures"),
        InstallTarget::Windows { executable } => executable.clone(),
        InstallTarget::AppImage { path } => path.clone(),
        InstallTarget::Manual => return Ok(None),
    };
    let metadata = fs::symlink_metadata(&path)?;
    if !path.is_absolute() || !metadata.is_file() || metadata.len() > MAX_ARTIFACT_BYTES {
        return Err(Error::Package);
    }
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    std::io::copy(&mut file, &mut hash)?;
    Ok(Some(format!("{:x}", hash.finalize())))
}

pub(super) fn require_unchanged(installation: &InstallTarget, expected: &str) -> Result<(), Error> {
    if fingerprint(installation)?.as_deref() != Some(expected) {
        return Err(Error::Installation(
            "the installed app changed; this recovery cannot replace it",
        ));
    }
    Ok(())
}

fn notice(message: &str) {
    // Only failure paths show an OS dialog, never another Captures window/tray.
    // Static text, passed as arguments (not interpolated into shell programs).
    #[cfg(target_os = "macos")]
    let result = Command::new("/usr/bin/osascript")
        .args(["-e", "on run argv\n display alert \"Captures startup recovery\" message (item 1 of argv) buttons {\"OK\"} default button \"OK\"\nend run", message])
        .spawn();
    #[cfg(windows)]
    let result = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", "Add-Type -AssemblyName PresentationFramework; [System.Windows.MessageBox]::Show($env:CAPTURES_RECOVERY_MESSAGE, 'Captures startup recovery') | Out-Null"])
        .env("CAPTURES_RECOVERY_MESSAGE", message)
        .spawn();
    #[cfg(target_os = "linux")]
    let result = Command::new("zenity")
        .args([
            "--warning",
            "--title=Captures startup recovery",
            "--text",
            message,
        ])
        .spawn()
        .or_else(|_| {
            Command::new("kdialog")
                .args(["--title", "Captures startup recovery", "--sorry", message])
                .spawn()
        })
        .or_else(|_| {
            Command::new("xmessage")
                .args([
                    "-center",
                    "-title",
                    "Captures startup recovery",
                    "-buttons",
                    "OK:0",
                    "-default",
                    "OK",
                    message,
                ])
                .spawn()
        });
    match result {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(_) => eprintln!("{message}"),
    }
}
