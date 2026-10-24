//! Installed Preview acquisition using the existing Tauri-compatible manifest.
//! Unlike development manifests, latest.json is not itself signed. Only verified
//! artifact bytes, including their signed version/target filename, can install.
pub mod recovery;

#[cfg(all(test, windows))]
mod windows_tests;

use super::*;
use std::{
    fs,
    process::{Command, Stdio},
};

const ENDPOINTS: [&str; 2] = [
    "https://captur.es/api/updates/preview",
    "https://github.com/joswayski/captures/releases/download/preview/latest.json",
];

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InstallTarget {
    Mac { app: PathBuf },
    Windows { executable: PathBuf },
    AppImage { path: PathBuf },
    Manual,
}

impl InstallTarget {
    fn current() -> Result<Self, Error> {
        let executable = std::env::current_exe()?;
        #[cfg(target_os = "macos")]
        {
            let app = executable
                .parent()
                .and_then(Path::parent)
                .and_then(Path::parent)
                .filter(|app| app.extension().is_some_and(|ext| ext == "app"))
                .ok_or(Error::Configuration(
                    "Preview updates require an installed app bundle",
                ))?;
            Ok(Self::Mac {
                app: app.to_owned(),
            })
        }
        #[cfg(windows)]
        {
            Ok(Self::Windows { executable })
        }
        #[cfg(target_os = "linux")]
        {
            let _ = executable;
            match std::env::var_os("APPIMAGE") {
                Some(path) if Path::new(&path).is_absolute() => {
                    Ok(Self::AppImage { path: path.into() })
                }
                _ => Ok(Self::Manual),
            }
        }
    }

    pub fn installable(&self) -> bool {
        !matches!(self, Self::Manual)
    }
}

pub struct Client {
    http: reqwest::blocking::Client,
    endpoints: Vec<Url>,
    key: PublicKey,
    renderer: Renderer,
    target: Target,
    current: Version,
    installation: InstallTarget,
}

#[derive(Deserialize)]
struct Manifest {
    version: String,
    notes: Option<String>,
    platforms: BTreeMap<String, Asset>,
}

#[derive(Deserialize)]
struct Asset {
    url: String,
    signature: String,
    size: Option<u64>,
}

pub struct Pending {
    pub info: ReleaseInfo,
    http: reqwest::blocking::Client,
    url: Url,
    signature: String,
    key: PublicKey,
    installation: InstallTarget,
}

pub struct Download {
    directory: tempfile::TempDir,
    request: InstallRequest,
    pub sha256: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InstallRequest {
    version: String,
    target: Target,
    signature: String,
    installation: InstallTarget,
    executable: PathBuf,
    preferences_visible: bool,
    #[serde(default)]
    preview_artifact_ids: Vec<String>,
    #[serde(default)]
    capture_windows: Vec<super::RestartCaptureWindow>,
    #[serde(default)]
    recovery: Option<recovery::InstallationCheck>,
}

pub fn artifact_name(target: Target, version: &str) -> String {
    let suffix = match target {
        Target::MacArm64 | Target::MacX64 => ".app.tar.gz",
        Target::WindowsX64 => "-setup.exe",
        Target::LinuxX64 => ".AppImage",
    };
    format!("Captures-{version}-{}{suffix}", target.as_str())
}

impl Client {
    pub fn installed(renderer: Renderer) -> Result<Self, Error> {
        let current =
            crate::preview_channel::version().ok_or(Error::Configuration("not a Preview build"))?;
        let key = crate::preview_channel::public_key()
            .ok_or(Error::Configuration("Preview public key is missing"))?;
        Self::new(
            &ENDPOINTS,
            key,
            renderer,
            Target::current_host().ok_or(Error::Target)?,
            current,
            InstallTarget::current()?,
        )
    }

    fn new(
        endpoints: &[&str],
        key: &str,
        renderer: Renderer,
        target: Target,
        current: &str,
        installation: InstallTarget,
    ) -> Result<Self, Error> {
        let endpoints = endpoints
            .iter()
            .map(|endpoint| {
                let url = Url::parse(endpoint).map_err(|_| Error::Manifest)?;
                validate_url(&url, cfg!(test) && is_loopback(&url))?;
                Ok(url)
            })
            .collect::<Result<Vec<_>, Error>>()?;
        let http = http_client(Policy::custom(|attempt| {
            if attempt.previous().len() >= 5
                || validate_url(attempt.url(), cfg!(test) && is_loopback(attempt.url())).is_err()
            {
                attempt.error("unsafe Preview redirect")
            } else {
                attempt.follow()
            }
        }))?;
        Ok(Self {
            http,
            endpoints,
            key: PublicKey::decode(key).map_err(|_| Error::Signature)?,
            renderer,
            target,
            current: Version::parse(current).map_err(|_| Error::Version)?,
            installation,
        })
    }

    pub fn current_version(&self) -> String {
        self.current.to_string()
    }
    pub fn installable(&self) -> bool {
        self.installation.installable()
    }

    pub fn check(&self, cancel: &CancelToken) -> Result<Option<Pending>, Error> {
        let mut last_error = Error::Network;
        for endpoint in &self.endpoints {
            let bytes = match fetch_metadata(&self.http, endpoint, MAX_MANIFEST_BYTES, cancel) {
                Ok(bytes) => bytes,
                Err(Error::Cancelled) => return Err(Error::Cancelled),
                Err(error) => {
                    last_error = error;
                    continue;
                }
            };
            let manifest: Manifest = serde_json::from_slice(&bytes).map_err(|_| Error::Manifest)?;
            let version = Version::parse(&manifest.version).map_err(|_| Error::Version)?;
            if version <= self.current {
                return Ok(None);
            }
            let platform = match self.target {
                Target::MacArm64 => "darwin-aarch64",
                Target::MacX64 => "darwin-x86_64",
                Target::WindowsX64 => "windows-x86_64",
                Target::LinuxX64 => "linux-x86_64",
            };
            let asset = manifest.platforms.get(platform).ok_or(Error::Target)?;
            let url = Url::parse(&asset.url).map_err(|_| Error::Manifest)?;
            validate_url(&url, cfg!(test) && is_loopback(&url))?;
            if asset.signature.len() as u64 > MAX_SIGNATURE_BYTES {
                return Err(Error::Signature);
            }
            let size = asset.size.unwrap_or(0);
            if size > MAX_ARTIFACT_BYTES {
                return Err(Error::Size);
            }
            return Ok(Some(Pending {
                info: ReleaseInfo {
                    renderer: self.renderer,
                    target: self.target,
                    version: version.to_string(),
                    notes: manifest.notes,
                    size,
                },
                http: self.http.clone(),
                url,
                signature: asset.signature.clone(),
                key: self.key.clone(),
                installation: self.installation.clone(),
            }));
        }
        Err(last_error)
    }
}

impl Pending {
    pub fn download(
        &self,
        directory: &Path,
        cancel: &CancelToken,
        mut progress: impl FnMut(u64, u64),
    ) -> Result<Download, Error> {
        check_cancel(cancel)?;
        let mut response = successful(
            self.http
                .get(self.url.clone())
                .send()
                .map_err(|_| Error::Network)?,
        )?;
        if response
            .content_length()
            .is_some_and(|size| size > MAX_ARTIFACT_BYTES)
        {
            return Err(Error::Size);
        }
        let total = response.content_length().unwrap_or(self.info.size);
        let mut bytes = Vec::new();
        let mut buffer = [0u8; 64 * 1024];
        loop {
            check_cancel(cancel)?;
            let count = response.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            if bytes.len() as u64 + count as u64 > MAX_ARTIFACT_BYTES {
                return Err(Error::Size);
            }
            bytes.extend_from_slice(&buffer[..count]);
            progress(bytes.len() as u64, total);
        }
        verify(
            &self.key,
            &self.signature,
            &bytes,
            self.info.target,
            &self.info.version,
        )?;
        check_cancel(cancel)?;
        let directory = tempfile::Builder::new()
            .prefix("captures-preview-")
            .tempdir_in(directory)?;
        let mut artifact = fs::File::options()
            .write(true)
            .create_new(true)
            .open(directory.path().join("artifact"))?;
        artifact.write_all(&bytes)?;
        artifact.sync_all()?;
        Ok(Download {
            directory,
            sha256: format!("{:x}", Sha256::digest(&bytes)),
            request: InstallRequest {
                version: self.info.version.clone(),
                target: self.info.target,
                signature: self.signature.clone(),
                installation: self.installation.clone(),
                executable: std::env::current_exe()?,
                preferences_visible: false,
                preview_artifact_ids: Vec::new(),
                capture_windows: Vec::new(),
                recovery: None,
            },
        })
    }
}

fn verify(
    key: &PublicKey,
    signature: &str,
    bytes: &[u8],
    target: Target,
    version: &str,
) -> Result<(), Error> {
    verify_named(key, signature, bytes, &artifact_name(target, version))
}

fn verify_named(
    key: &PublicKey,
    signature: &str,
    bytes: &[u8],
    filename: &str,
) -> Result<(), Error> {
    let decoded = decode_envelope(signature)?;
    let signature = Signature::decode(std::str::from_utf8(&decoded).map_err(|_| Error::Signature)?)
        .map_err(|_| Error::Signature)?;
    key.verify(bytes, &signature, false)
        .map_err(|_| Error::Signature)?;
    // CLI 2.11 signs the filename, not a version field. Production artifacts
    // deliberately put both version and target in that authenticated filename.
    let filename = format!("file:{filename}");
    if !signature
        .trusted_comment()
        .split('\t')
        .any(|field| field == filename)
    {
        return Err(Error::Signature);
    }
    Ok(())
}

// The repository already uses a small RFC 4648 encoder for bridge payloads.
// Decode only a bounded canonical standard-base64 envelope; cryptographic
// parsing and verification remain in minisign-verify, not this transport codec.
fn decode_envelope(value: &str) -> Result<Vec<u8>, Error> {
    let value = value.trim();
    if value.is_empty()
        || !value.len().is_multiple_of(4)
        || value.len() as u64 > MAX_SIGNATURE_BYTES
    {
        return Err(Error::Signature);
    }
    let mut result = Vec::new();
    for (index, group) in value.as_bytes().chunks_exact(4).enumerate() {
        let mut decoded = [0u8; 4];
        let mut padding = 0;
        for (slot, &byte) in group.iter().enumerate() {
            decoded[slot] = match byte {
                b'A'..=b'Z' => byte - b'A',
                b'a'..=b'z' => byte - b'a' + 26,
                b'0'..=b'9' => byte - b'0' + 52,
                b'+' => 62,
                b'/' => 63,
                b'=' if slot >= 2 && (index + 1) * 4 == value.len() => {
                    padding += 1;
                    0
                }
                _ => return Err(Error::Signature),
            };
            if padding > 0 && byte != b'=' {
                return Err(Error::Signature);
            }
        }
        if (padding == 2 && decoded[1] & 15 != 0) || (padding == 1 && decoded[2] & 3 != 0) {
            return Err(Error::Signature);
        }
        result.push((decoded[0] << 2) | (decoded[1] >> 4));
        if padding < 2 {
            result.push((decoded[1] << 4) | (decoded[2] >> 2));
        }
        if padding == 0 {
            result.push((decoded[2] << 6) | decoded[3]);
        }
    }
    Ok(result)
}

impl Download {
    /// Launch only after editor/recording vetoes and before orderly worker drain.
    /// The copied helper waits for the host-lifetime package lock to release, so
    /// even an early spawn can never replace a still-running native application.
    pub fn launch(mut self, restart: super::RestartIntent) -> Result<(), Error> {
        if !self.request.installation.installable() {
            return Err(Error::Installation("this package uses manual updates"));
        }
        self.request.preferences_visible = restart.restore_preferences;
        self.request.preview_artifact_ids = restart.preview_artifact_ids;
        self.request.capture_windows = restart.capture_windows;
        let bytes = serde_json::to_vec(&self.request).map_err(|_| Error::Manifest)?;
        if bytes.len() > 256 * 1024 {
            return Err(Error::MetadataTooLarge);
        }
        let helper_name = if cfg!(windows) {
            "native_preview_update_helper.exe"
        } else {
            "native_preview_update_helper"
        };
        let helper = self
            .request
            .executable
            .parent()
            .ok_or(Error::Package)?
            .join(helper_name);
        let copy = self.directory.path().join(helper_name);
        fs::copy(&helper, &copy)?;
        let request = self.directory.path().join("install.json");
        let mut file = fs::File::options()
            .create_new(true)
            .write(true)
            .open(&request)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        let log = fs::File::options()
            .create_new(true)
            .write(true)
            .open(self.directory.path().join("install.log"))?;
        Command::new(copy)
            .arg("--install")
            .arg(&request)
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .spawn()?;
        // The child now owns the persistent request/artifact/log directory. It
        // retains it on failure so there is actionable recovery evidence.
        let _ = self.directory.keep();
        Ok(())
    }
}

/// Packaging gate: authenticate the final bytes with the compiled shipping key,
/// using the same filename binding as the installed client, before uploading.
pub fn verify_package(version: &str, artifact: &Path, signature: &Path) -> Result<(), Error> {
    Version::parse(version).map_err(|_| Error::Version)?;
    let key = PublicKey::decode(crate::preview_channel::public_key().ok_or(Error::Signature)?)
        .map_err(|_| Error::Signature)?;
    if fs::metadata(artifact)?.len() > MAX_ARTIFACT_BYTES
        || fs::metadata(signature)?.len() > MAX_SIGNATURE_BYTES
    {
        return Err(Error::Size);
    }
    let target = Target::current_host().ok_or(Error::Target)?;
    let mut name = artifact_name(target, version);
    // Only the packaging verifier accepts Debian artifacts, for old Tauri
    // installs' cutover. Native AppImage acquisition remains format-bound.
    if target == Target::LinuxX64 && artifact.extension().is_some_and(|ext| ext == "deb") {
        name = name.replace(".AppImage", ".deb");
    }
    verify_named(
        &key,
        &fs::read_to_string(signature)?,
        &fs::read(artifact)?,
        &name,
    )
}

/// Entry point for the separately copied helper. Never called by a GUI process.
pub fn install(request_path: &Path) -> Result<(), Error> {
    let current =
        crate::preview_channel::version().ok_or(Error::Configuration("not a Preview helper"))?;
    let key = PublicKey::decode(crate::preview_channel::public_key().ok_or(Error::Signature)?)
        .map_err(|_| Error::Signature)?;
    install_with(request_path, current, &key, || {
        crate::preview_channel::package_lock()
            .map_err(|_| Error::Installation("could not open package lock"))
    })
}

fn install_with(
    request_path: &Path,
    current: &str,
    key: &PublicKey,
    package_lock: impl FnOnce() -> Result<fs::File, Error>,
) -> Result<(), Error> {
    let mut bytes = Vec::new();
    fs::File::open(request_path)?
        .take(256 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 256 * 1024 {
        return Err(Error::Manifest);
    }
    let request: InstallRequest = serde_json::from_slice(&bytes).map_err(|_| Error::Manifest)?;
    if request.target != Target::current_host().ok_or(Error::Target)?
        || Version::parse(&request.version).map_err(|_| Error::Version)?
            <= Version::parse(current).map_err(|_| Error::Version)?
    {
        return Err(Error::Version);
    }
    let directory = request_path.parent().ok_or(Error::Package)?;
    let artifact = directory.join("artifact");
    if fs::metadata(&artifact)?.len() > MAX_ARTIFACT_BYTES {
        return Err(Error::Size);
    }
    let payload = fs::read(&artifact)?;
    verify(
        key,
        &request.signature,
        &payload,
        request.target,
        &request.version,
    )?;
    let lock = package_lock()?;
    let start = std::time::Instant::now();
    loop {
        match lock.try_lock() {
            Ok(()) => break,
            Err(std::fs::TryLockError::WouldBlock)
                if start.elapsed() < Duration::from_secs(120) =>
            {
                std::thread::sleep(Duration::from_millis(100))
            }
            _ => {
                return Err(Error::Installation(
                    "Captures did not finish closing; nothing was replaced",
                ));
            }
        }
    }
    // Use the verified in-memory bytes, not an artifact path that could change
    // between verification and installation.
    if let Some(recovery) = &request.recovery {
        recovery.begin(&request.installation, &request.version)?;
    }
    match &request.installation {
        InstallTarget::Windows { executable } => {
            if request.target != Target::WindowsX64
                || executable
                    .file_name()
                    .is_none_or(|name| name != "captures.exe")
            {
                return Err(Error::Package);
            }
            let installer = directory.join("verified-setup.exe");
            let mut file = fs::File::options()
                .create_new(true)
                .write(true)
                .open(&installer)?;
            file.write_all(&payload)?;
            file.sync_all()?;
            drop(file);
            // NSIS owns the installed path and update/relaunch registry identity.
            // Release the cooperative lock before /R launches the new process.
            drop(lock);
            launch_and_confirm(&installer, directory, &request)?;
        }
        InstallTarget::AppImage { path } => {
            if request.target != Target::LinuxX64
                || !path.is_absolute()
                || !fs::symlink_metadata(path)?.is_file()
            {
                return Err(Error::Package);
            }
            let parent = path.parent().ok_or(Error::Package)?;
            let mut replacement = tempfile::NamedTempFile::new_in(parent)?;
            replacement.write_all(&payload)?;
            replacement.as_file().sync_all()?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                replacement
                    .as_file()
                    .set_permissions(fs::Permissions::from_mode(0o755))?;
            }
            let backup = parent.join(format!(
                ".Captures-previous-{}.AppImage",
                uuid::Uuid::new_v4()
            ));
            record_backup(directory, path, &backup)?;
            fs::rename(path, &backup)?;
            if let Err(error) = replacement.persist_noclobber(path) {
                fs::rename(&backup, path)?;
                return Err(Error::Io(error.error));
            }
            drop(lock);
            launch_and_confirm(path, directory, &request)?;
            fs::remove_file(backup)?;
        }
        InstallTarget::Mac { app } => {
            if !matches!(request.target, Target::MacArm64 | Target::MacX64)
                || !app.is_absolute()
                || !fs::symlink_metadata(app)?.is_dir()
                || app.file_name().is_none_or(|name| name != "Captures.app")
            {
                return Err(Error::Package);
            }
            let parent = app.parent().ok_or(Error::Package)?;
            let stage = tempfile::Builder::new()
                .prefix(".captures-preview-")
                .tempdir_in(parent)?;
            extract_app(&payload, stage.path())?;
            let replacement = stage.path().join("Captures.app");
            let info = plist::Value::from_file(replacement.join("Contents/Info.plist"))
                .map_err(|_| Error::Package)?;
            let info = info.as_dictionary().ok_or(Error::Package)?;
            if info
                .get("CFBundleIdentifier")
                .and_then(plist::Value::as_string)
                != Some(crate::preview_channel::IDENTITY)
                || info
                    .get("CFBundleShortVersionString")
                    .and_then(plist::Value::as_string)
                    != Some(&request.version)
                || info
                    .get("CFBundleExecutable")
                    .and_then(plist::Value::as_string)
                    != Some("captures")
            {
                return Err(Error::Package);
            }
            let backup = parent.join(format!(".Captures-previous-{}.app", uuid::Uuid::new_v4()));
            record_backup(directory, app, &backup)?;
            fs::rename(app, &backup)?;
            if let Err(error) = fs::rename(&replacement, app) {
                fs::rename(&backup, app)?;
                return Err(error.into());
            }
            drop(lock);
            launch_and_confirm(&app.join("Contents/MacOS/captures"), directory, &request)?;
            fs::remove_dir_all(backup)?;
        }
        InstallTarget::Manual => {
            return Err(Error::Installation(
                "Debian packages update through a manual installer",
            ));
        }
    }
    fs::write(directory.join("completed"), b"installed\n")?;
    // The copied helper/log may still be open (not removable on Windows).
    // Remove the large payload now; the next ordinary startup prunes the rest.
    fs::remove_file(artifact)?;
    if directory.join("verified-setup.exe").exists() {
        let _ = fs::remove_file(directory.join("verified-setup.exe"));
    }
    Ok(())
}

fn record_backup(directory: &Path, installed: &Path, backup: &Path) -> Result<(), Error> {
    let mut file = fs::File::options()
        .write(true)
        .create_new(true)
        .open(directory.join("backup.json"))?;
    serde_json::to_writer(
        &mut file,
        &serde_json::json!({"installed":installed,"backup":backup}),
    )
    .map_err(|_| Error::Manifest)?;
    file.sync_all()?;
    Ok(())
}

fn launch_and_confirm(
    executable: &Path,
    directory: &Path,
    request: &InstallRequest,
) -> Result<(), Error> {
    use std::io::{Seek, SeekFrom};
    let ready = directory.join("ready");
    let mut file = fs::File::options()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&ready)?;
    let token = uuid::Uuid::new_v4().to_string();
    health::write_restart_intent(
        &ready,
        &super::RestartIntent {
            restore_preferences: request.preferences_visible,
            preview_artifact_ids: request.preview_artifact_ids.clone(),
            capture_windows: request.capture_windows.clone(),
        },
    )?;
    let windows = matches!(request.installation, InstallTarget::Windows { .. });
    let mut child = if windows {
        #[cfg(windows)]
        {
            let status = windows_installer_command(executable, &ready, &token)?
                .stdin(Stdio::null())
                .status()?;
            if !status.success() {
                return Err(Error::Installation("the Windows installer failed"));
            }
            // /R launches asynchronously through RunAsUser. Installer exit is
            // not application readiness, and NSIS does not return a child handle.
            None
        }
        #[cfg(not(windows))]
        return Err(Error::Target);
    } else {
        Some(
            Command::new(executable)
                .env_remove("APPIMAGE")
                .env_remove("APPDIR")
                .arg("--live")
                .arg("--native-update-ready-file")
                .arg(&ready)
                .arg("--native-update-ready-token")
                .arg(&token)
                .stdin(Stdio::null())
                .spawn()?,
        )
    };
    let started = std::time::Instant::now();
    loop {
        file.seek(SeekFrom::Start(0))?;
        let mut bytes = Vec::new();
        Read::by_ref(&mut file).take(128).read_to_end(&mut bytes)?;
        if bytes == format!("{token}\n").as_bytes() {
            return Ok(());
        }
        let exited = child
            .as_mut()
            .map(|child| child.try_wait())
            .transpose()?
            .flatten()
            .is_some();
        if exited || started.elapsed() > Duration::from_secs(120) {
            // Do not overwrite a possibly running process. Keep the old package
            // and recovery journal rather than guessing that a timeout is death.
            return Err(Error::Installation(if windows {
                "Windows installation finished but startup was not confirmed; recovery files retained"
            } else {
                "new Preview did not confirm startup; previous package retained (see backup.json)"
            }));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(windows)]
fn windows_installer_command(
    installer: &Path,
    ready: &Path,
    token: &str,
) -> Result<Command, Error> {
    use std::{
        ffi::OsString,
        os::windows::{ffi::OsStrExt, process::CommandExt},
    };
    // NSIS GetOptions is not a CRT argv parser. Keep /ARGS last, quote only
    // the filename, and preserve UTF-16 rather than using Path::display().
    // The generated filename ends in `ready`, never a quote or trailing slash.
    let mut arguments = OsString::from("/P /UPDATE /R /ARGS --live --native-update-ready-file \"");
    arguments.push(ready);
    arguments.push("\" --native-update-ready-token ");
    arguments.push(token);
    // Tauri's pinned Unicode NSIS 3.11 has 1024-character strings, including
    // NUL. $CMDLINE includes the quoted installer executable and separator.
    if installer.as_os_str().encode_wide().count() + 3 + arguments.encode_wide().count() >= 1024 {
        return Err(Error::Configuration(
            "Windows update command exceeds the NSIS string limit",
        ));
    }
    let mut command = Command::new(installer);
    command.raw_arg(arguments);
    Ok(command)
}

fn extract_app(payload: &[u8], destination: &Path) -> Result<(), Error> {
    use std::path::Component;
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(payload));
    let mut size = 0u64;
    for (count, item) in archive.entries()?.enumerate() {
        if count >= staging::MAX_ENTRIES {
            return Err(Error::Archive);
        }
        let mut item = item?;
        let path = item.path()?.into_owned();
        let mut components = path.components();
        if components.next() != Some(Component::Normal("Captures.app".as_ref()))
            || components.any(|part| !matches!(part, Component::Normal(_)))
        {
            return Err(Error::ArchivePath);
        }
        // Native AppKit packages contain regular files/directories, not framework
        // symlinks. Refuse links and special files rather than following them.
        if !item.header().entry_type().is_file() && !item.header().entry_type().is_dir() {
            return Err(Error::ArchivePath);
        }
        size = size.checked_add(item.size()).ok_or(Error::Archive)?;
        if item.size() > staging::MAX_FILE_BYTES || size > staging::MAX_EXPANDED_BYTES {
            return Err(Error::Archive);
        }
        if !item.unpack_in(destination)? {
            return Err(Error::ArchivePath);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Cursor, net::TcpListener};

    #[test]
    fn preview_http_download_authenticates_bytes_version_and_platform() {
        let pair = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let key = pair.pk.to_box().unwrap().to_string();
        let payload = b"native package fixture";
        // Independent filename contract shared with the release bundler, not
        // derived from artifact_name (which is the implementation under test).
        let comment = "timestamp:1\tfile:Captures-2026.10.1101-x86_64-unknown-linux-gnu.AppImage";
        for (version, platform, downloaded, signed_comment, valid) in [
            (
                "2026.10.1101",
                Target::LinuxX64,
                payload.as_slice(),
                comment,
                true,
            ),
            (
                "2026.10.1102",
                Target::LinuxX64,
                payload.as_slice(),
                comment,
                false,
            ),
            (
                "2026.10.1101",
                Target::WindowsX64,
                payload.as_slice(),
                comment,
                false,
            ),
            (
                "2026.10.1101",
                Target::LinuxX64,
                b"tampered bytes".as_slice(),
                comment,
                false,
            ),
            (
                "2026.10.1101",
                Target::LinuxX64,
                payload.as_slice(),
                "timestamp:1\tfile:Captures-2026.10.1101-x86_64-unknown-linux-gnu.deb",
                false,
            ),
        ] {
            let signature = minisign::sign(
                None,
                &pair.sk,
                Cursor::new(payload),
                Some(signed_comment),
                None,
            )
            .unwrap();
            let signature = crate::editor_layers::base64(signature.into_string().as_bytes());
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let metadata = serde_json::to_vec(&serde_json::json!({
                "version": version,
                "platforms": {
                    "linux-x86_64": {"url":format!("{url}/package"),"signature":signature},
                    "windows-x86_64": {"url":format!("{url}/package"),"signature":signature}
                }
            }))
            .unwrap();
            let server = super::super::tests::serve(
                listener,
                vec![(200, metadata), (200, downloaded.to_vec())],
            );
            let client = Client::new(
                &[&format!("{url}/latest.json")],
                &key,
                Renderer::Wgpu,
                platform,
                "2026.10.1001",
                InstallTarget::Manual,
            )
            .unwrap();
            let cancel = CancelToken::default();
            let pending = client.check(&cancel).unwrap().unwrap();
            let directory = tempfile::tempdir().unwrap();
            let result = pending.download(directory.path(), &cancel, |_, _| {});
            if valid {
                let downloaded = result.unwrap();
                assert_eq!(
                    fs::read(downloaded.directory.path().join("artifact")).unwrap(),
                    payload
                );
            } else {
                assert!(matches!(result, Err(Error::Signature)));
                assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
            }
            assert_eq!(server.join().unwrap(), ["/latest.json", "/package"]);
        }
    }

    #[test]
    fn app_archive_rejects_links_and_unrelated_roots_before_publication() {
        for (path, kind, accepted) in [
            (
                "Captures.app/Contents/MacOS/captures",
                tar::EntryType::Regular,
                true,
            ),
            (
                "Other.app/Contents/MacOS/captures",
                tar::EntryType::Regular,
                false,
            ),
            (
                "Captures.app/Contents/MacOS/captures",
                tar::EntryType::Symlink,
                false,
            ),
        ] {
            let mut archive = tar::Builder::new(flate2::write::GzEncoder::new(
                Vec::new(),
                flate2::Compression::fast(),
            ));
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(kind);
            header.set_mode(0o755);
            header.set_size(if kind.is_file() { 6 } else { 0 });
            if kind.is_symlink() {
                header.set_link_name("/outside").unwrap();
            }
            header.set_cksum();
            let bytes: &[u8] = if kind.is_file() { b"native" } else { b"" };
            archive.append_data(&mut header, path, bytes).unwrap();
            let bytes = archive.into_inner().unwrap().finish().unwrap();
            let stage = tempfile::tempdir().unwrap();
            assert_eq!(extract_app(&bytes, stage.path()).is_ok(), accepted);
            if accepted {
                assert_eq!(fs::read(stage.path().join(path)).unwrap(), b"native");
            } else {
                assert_eq!(fs::read_dir(stage.path()).unwrap().count(), 0);
            }
        }
    }
}
