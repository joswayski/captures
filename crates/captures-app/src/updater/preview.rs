//! Installed Preview acquisition using the existing Tauri-compatible manifest.
//! Unlike development manifests, latest.json is not itself signed. Only verified
//! artifact bytes, including their signed version/target filename, can install.
#[cfg(target_os = "linux")]
pub mod appimage;
pub mod delta;
pub mod recovery;

#[cfg(all(test, windows))]
mod windows_tests;

use super::*;
use std::{
    fs, io,
    process::{Command, Stdio},
};

const ENDPOINTS: [&str; 2] = [
    "https://captur.es/api/updates/preview",
    "https://github.com/joswayski/captures/releases/download/preview/latest.json",
];
const STAGING_ENDPOINTS: [&str; 1] = [match crate::preview_channel::staging_feed() {
    Some(feed) => feed,
    None => "",
}];

/// The installed build's feed: a staging build's own feed, else the public one.
/// Updates and startup repairs both read it.
fn endpoints() -> &'static [&'static str] {
    if crate::preview_channel::staging_feed().is_some() {
        &STAGING_ENDPOINTS
    } else {
        &ENDPOINTS
    }
}

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
                    "updates require an installed app bundle",
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
    #[serde(default)]
    changelog: Vec<ChangelogEntry>,
}

/// One release in `latest.json`'s `changelog`, newest first.
#[derive(Deserialize)]
struct ChangelogEntry {
    version: String,
    #[serde(default)]
    display_version: Option<String>,
    #[serde(default)]
    notes: Option<String>,
}

/// Shipping `stacked_changelog`: the releases after `current` through
/// `latest`, newest first. Entries that fail to parse are skipped.
fn stacked_changelog(
    entries: Vec<ChangelogEntry>,
    current: &Version,
    latest: &Version,
) -> Vec<crate::update_notice::ChangelogEntry> {
    let mut stacked: Vec<_> = entries
        .into_iter()
        .filter_map(|entry| {
            let version = Version::parse(entry.version.trim()).ok()?;
            (version > *current && version <= *latest).then(|| {
                (
                    version,
                    crate::update_notice::ChangelogEntry {
                        display_version: entry
                            .display_version
                            .filter(|value| !value.trim().is_empty())
                            .unwrap_or_else(|| super::display_version(&entry.version)),
                        version: entry.version,
                        notes: entry.notes.filter(|notes| !notes.trim().is_empty()),
                    },
                )
            })
        })
        .collect();
    stacked.sort_by(|a, b| b.0.cmp(&a.0));
    stacked.into_iter().map(|(_, entry)| entry).collect()
}

#[derive(Deserialize)]
struct Asset {
    url: String,
    signature: String,
    size: Option<u64>,
    /// Parsed leniently by [`delta::select`]; see [`delta`].
    #[serde(default)]
    patches: serde_json::Value,
}

pub struct Pending {
    pub info: ReleaseInfo,
    http: reqwest::blocking::Client,
    url: Url,
    signature: String,
    key: PublicKey,
    installation: InstallTarget,
    patch: Option<delta::Offer>,
    full_size: u64,
    current: Version,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    preferences_stacking: Option<super::WindowStacking>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    capture_stacking: BTreeMap<String, super::WindowStacking>,
    #[serde(default)]
    recovery: Option<recovery::InstallationCheck>,
    /// The installed version a verified `patch` in the request folder starts
    /// from; without it, `artifact` holds the full download.
    #[serde(default)]
    patch: Option<String>,
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
        let current = crate::preview_channel::version()
            .ok_or(Error::Configuration("not an installed release build"))?;
        if !crate::preview_channel::updates_enabled() {
            return Err(Error::Configuration("updates are off in test installs"));
        }
        let key = crate::preview_channel::public_key()
            .ok_or(Error::Configuration("updater public key is missing"))?;
        Self::new(
            endpoints(),
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
                attempt.error("unsafe update redirect")
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
        let mut last_error = Error::Network(None);
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
            let patch = delta::select(&asset.patches, &self.current, size, &self.installation);
            return Ok(Some(Pending {
                info: ReleaseInfo {
                    renderer: self.renderer,
                    target: self.target,
                    version: version.to_string(),
                    notes: manifest.notes,
                    // What this update downloads, the patch when one applies.
                    size: patch.as_ref().map_or(size, |patch| patch.size),
                    changelog: stacked_changelog(manifest.changelog, &self.current, &version),
                },
                http: self.http.clone(),
                url,
                signature: asset.signature.clone(),
                key: self.key.clone(),
                installation: self.installation.clone(),
                patch,
                full_size: size,
                current: self.current.clone(),
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
        progress: impl FnMut(u64, u64),
    ) -> Result<Download, Error> {
        self.download_verifying(directory, cancel, progress, || {})
    }

    /// [`Pending::download`], reporting `verifying` once every byte has
    /// arrived and before the signature and hash checks run. A listed patch
    /// from this installation is tried first; any failure except cancellation
    /// downloads the full artifact instead, restarting progress at its size.
    pub fn download_verifying(
        &self,
        directory: &Path,
        cancel: &CancelToken,
        mut progress: impl FnMut(u64, u64),
        verifying: impl FnOnce(),
    ) -> Result<Download, Error> {
        let mut verifying = Some(verifying);
        if let Some(patch) = &self.patch {
            match self.patched(patch, directory, cancel, &mut progress, &mut verifying) {
                Ok(download) => return Ok(download),
                Err(Error::Cancelled) => return Err(Error::Cancelled),
                // Any other patch failure falls back to the full artifact.
                Err(_) => {}
            }
        }
        let bytes = self.fetch(
            self.url.clone(),
            MAX_ARTIFACT_BYTES,
            self.full_size,
            cancel,
            &mut progress,
        )?;
        if let Some(verifying) = verifying.take() {
            verifying();
        }
        verify(
            &self.key,
            &self.signature,
            &bytes,
            self.info.target,
            &self.info.version,
        )?;
        let download = self.download_folder(directory, &bytes, &self.signature, None, cancel)?;
        let mut artifact = fs::File::options()
            .write(true)
            .create_new(true)
            .open(download.directory.path().join("artifact"))?;
        artifact.write_all(&bytes)?;
        artifact.sync_all()?;
        Ok(download)
    }

    /// Download and verify the patch, and rebuild the release from the
    /// installed package to prove it applies before the app quits. The helper
    /// rebuilds again from the installed package (macOS, Windows) or packs this
    /// stage, rechecking every file (AppImage, whose mount ends with the app).
    fn patched(
        &self,
        patch: &delta::Offer,
        directory: &Path,
        cancel: &CancelToken,
        progress: &mut impl FnMut(u64, u64),
        verifying: &mut Option<impl FnOnce()>,
    ) -> Result<Download, Error> {
        let base = delta::base(&self.installation)?;
        let bytes = self.fetch(patch.url()?, patch.size, patch.size, cancel, progress)?;
        if bytes.len() as u64 != patch.size {
            return Err(Error::Size);
        }
        let from = self.current.to_string();
        verify_named(
            &self.key,
            &patch.signature,
            &bytes,
            &delta::patch_name(self.info.target, &self.info.version, &from),
        )?;
        let download =
            self.download_folder(directory, &bytes, &patch.signature, Some(from), cancel)?;
        let path = download.directory.path().join("patch");
        let mut file = fs::File::options()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        let stage = download.directory.path().join("stage");
        delta::rebuild(
            &mut file,
            self.info.target,
            &self.current,
            &self.info.version,
            &base,
            stage.clone(),
            cancel,
        )?;
        if !matches!(self.installation, InstallTarget::AppImage { .. }) {
            fs::remove_dir_all(stage)?;
        }
        // Only now: a fallback's progress must still reach the UI, which stops
        // following progress once it shows Verifying.
        if let Some(verifying) = verifying.take() {
            verifying();
        }
        Ok(download)
    }

    fn fetch(
        &self,
        url: Url,
        limit: u64,
        expected: u64,
        cancel: &CancelToken,
        progress: &mut impl FnMut(u64, u64),
    ) -> Result<Vec<u8>, Error> {
        check_cancel(cancel)?;
        let mut response = successful(self.http.get(url).send()?)?;
        if response.content_length().is_some_and(|size| size > limit) {
            return Err(Error::Size);
        }
        let total = response.content_length().unwrap_or(expected);
        let mut bytes = Vec::new();
        let mut buffer = [0u8; 64 * 1024];
        loop {
            check_cancel(cancel)?;
            let count = response.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            if bytes.len() as u64 + count as u64 > limit {
                return Err(Error::Size);
            }
            bytes.extend_from_slice(&buffer[..count]);
            progress(bytes.len() as u64, total);
        }
        Ok(bytes)
    }

    /// The private request folder for verified bytes and the signature the
    /// helper checks again.
    fn download_folder(
        &self,
        directory: &Path,
        bytes: &[u8],
        signature: &str,
        patch: Option<String>,
        cancel: &CancelToken,
    ) -> Result<Download, Error> {
        check_cancel(cancel)?;
        let directory = tempfile::Builder::new()
            .prefix("captures-preview-")
            .tempdir_in(directory)?;
        Ok(Download {
            directory,
            sha256: format!("{:x}", Sha256::digest(bytes)),
            request: InstallRequest {
                version: self.info.version.clone(),
                target: self.info.target,
                signature: signature.to_owned(),
                installation: self.installation.clone(),
                executable: std::env::current_exe()?,
                preferences_visible: false,
                preview_artifact_ids: Vec::new(),
                capture_windows: Vec::new(),
                preferences_stacking: None,
                capture_stacking: BTreeMap::new(),
                recovery: None,
                patch,
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
        self.request.preferences_stacking = restart.preferences_stacking;
        self.request.capture_stacking = restart.capture_stacking;
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
        spawn_fresh(
            Command::new(copy)
                .arg("--install")
                .arg(&request)
                .stdin(Stdio::null())
                .stdout(log.try_clone()?)
                .stderr(log),
        )?;
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
    // Patch files are named for their target, version and starting version.
    if artifact
        .extension()
        .is_some_and(|ext| ext == "capturespatch")
    {
        let file = artifact
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if !file.starts_with(&format!("Captures-{version}-{}.from-", target.as_str())) {
            return Err(Error::Signature);
        }
        name = file.to_owned();
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
    let current = crate::preview_channel::version()
        .ok_or(Error::Configuration("not a release update helper"))?;
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
    // Either the full artifact or a patch from this installed version, each
    // checked against its signed name before anything is replaced.
    let read = |name: &str| -> Result<Vec<u8>, Error> {
        let path = directory.join(name);
        if fs::metadata(&path)?.len() > MAX_ARTIFACT_BYTES {
            return Err(Error::Size);
        }
        Ok(fs::read(path)?)
    };
    let (payload, patch) = match &request.patch {
        None => {
            let payload = read("artifact")?;
            verify(
                key,
                &request.signature,
                &payload,
                request.target,
                &request.version,
            )?;
            (Some(payload), None)
        }
        Some(from) => {
            let from = Version::parse(from).map_err(|_| Error::Version)?;
            if from != Version::parse(current).map_err(|_| Error::Version)? {
                return Err(Error::Version);
            }
            let bytes = read("patch")?;
            verify_named(
                key,
                &request.signature,
                &bytes,
                &delta::patch_name(request.target, &request.version, &from.to_string()),
            )?;
            (None, Some(from))
        }
    };
    let rebuild = |root: PathBuf, base: &Path| {
        delta::rebuild(
            &mut fs::File::open(directory.join("patch"))?,
            request.target,
            patch.as_ref().ok_or(Error::Package)?,
            &request.version,
            base,
            root,
            &CancelToken::default(),
        )
    };
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
                || executable.file_name().is_none_or(|name| {
                    name.to_str() != Some(&format!("{}.exe", crate::preview_channel::EXECUTABLE))
                })
            {
                return Err(Error::Package);
            }
            if let Some(payload) = &payload {
                let installer = directory.join("verified-setup.exe");
                let mut file = fs::File::options()
                    .create_new(true)
                    .write(true)
                    .open(&installer)?;
                file.write_all(payload)?;
                file.sync_all()?;
                drop(file);
                // NSIS owns the installed path and update/relaunch registry identity.
                // Release the cooperative lock before /R launches the new process.
                drop(lock);
                launch_and_confirm(&installer, directory, &request, true)?;
            } else {
                // Patch: rebuild beside the installed files, swap them in and
                // start the new executable, without the installer.
                let folder = executable.parent().ok_or(Error::Package)?;
                let stage = tempfile::Builder::new()
                    .prefix(".captures-update-")
                    .tempdir_in(folder)?;
                let (package, _) = rebuild(stage.path().join("package"), folder)?;
                let replaced = replace_files(folder, &package)?;
                set_display_version(&request.version);
                drop(lock);
                launch_and_confirm(executable, directory, &request, false)?;
                for (_, previous) in replaced.into_iter() {
                    if let Some(previous) = previous {
                        let _ = fs::remove_file(previous);
                    }
                }
            }
        }
        InstallTarget::AppImage { path } => {
            if request.target != Target::LinuxX64
                || !path.is_absolute()
                || !fs::symlink_metadata(path)?.is_file()
            {
                return Err(Error::Package);
            }
            let parent = path.parent().ok_or(Error::Package)?;
            let payload = match payload {
                Some(payload) => payload,
                // The app rebuilt the AppDir from its own mount; pack it,
                // rechecking every file against the signed patch.
                None => appimage_from_stage(directory, &request, patch.as_ref())?,
            };
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
            launch_and_confirm(path, directory, &request, false)?;
            fs::remove_file(backup)?;
        }
        InstallTarget::Mac { app } => {
            if !matches!(request.target, Target::MacArm64 | Target::MacX64)
                || !app.is_absolute()
                || !fs::symlink_metadata(app)?.is_dir()
                || app
                    .file_name()
                    .is_none_or(|name| name != crate::preview_channel::APP_BUNDLE)
            {
                return Err(Error::Package);
            }
            let parent = app.parent().ok_or(Error::Package)?;
            let stage = tempfile::Builder::new()
                .prefix(".captures-preview-")
                .tempdir_in(parent)?;
            let replacement = match &payload {
                Some(payload) => {
                    extract_app(payload, stage.path())?;
                    stage.path().join(crate::preview_channel::APP_BUNDLE)
                }
                None => {
                    let (package, executables) = rebuild(stage.path().join("patched"), app)?;
                    public_modes(&package, &executables)?;
                    package
                }
            };
            if replacement.file_name() != Some(crate::preview_channel::APP_BUNDLE.as_ref()) {
                return Err(Error::Package);
            }
            let info = plist::Value::from_file(replacement.join("Contents/Info.plist"))
                .map_err(|_| Error::Package)?;
            let info = info.as_dictionary().ok_or(Error::Package)?;
            if info
                .get("CFBundleIdentifier")
                .and_then(plist::Value::as_string)
                != Some(crate::preview_channel::BUNDLE_IDENTIFIER)
                || info
                    .get("CFBundleShortVersionString")
                    .and_then(plist::Value::as_string)
                    != Some(&request.version)
                || info
                    .get("CFBundleExecutable")
                    .and_then(plist::Value::as_string)
                    != Some(crate::preview_channel::EXECUTABLE)
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
            launch_and_confirm(
                &app.join("Contents/MacOS")
                    .join(crate::preview_channel::EXECUTABLE),
                directory,
                &request,
                false,
            )?;
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
    // Remove the large payloads now; the next ordinary startup prunes the rest.
    for name in ["artifact", "verified-setup.exe", "patch"] {
        let _ = fs::remove_file(directory.join(name));
    }
    let _ = fs::remove_dir_all(directory.join("stage"));
    Ok(())
}

/// The AppImage for a patch install: the app's rebuilt stage packed with the
/// runtime it carries, every file rechecked against the signed patch.
#[cfg(target_os = "linux")]
fn appimage_from_stage(
    directory: &Path,
    request: &InstallRequest,
    from: Option<&Version>,
) -> Result<Vec<u8>, Error> {
    let (root, entries) = super::patch::entries(
        &mut fs::File::open(directory.join("patch"))?,
        &super::patch::Expected {
            target: request.target,
            base_version: from.ok_or(Error::Package)?,
            version: &request.version,
        },
    )?;
    appimage::pack(
        &directory.join("stage").join(root),
        &entries,
        appimage::INSTALL_LEVEL,
    )
}

#[cfg(not(target_os = "linux"))]
fn appimage_from_stage(
    _: &Path,
    _: &InstallRequest,
    _: Option<&Version>,
) -> Result<Vec<u8>, Error> {
    Err(Error::Target)
}

/// A rebuilt stage is private (700/600); installed bundles are 755/644.
fn public_modes(
    package: &Path,
    executables: &std::collections::HashSet<PathBuf>,
) -> Result<(), Error> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut pending = vec![package.to_owned()];
        while let Some(folder) = pending.pop() {
            fs::set_permissions(&folder, fs::Permissions::from_mode(0o755))?;
            for item in fs::read_dir(&folder)? {
                let path = item?.path();
                let metadata = fs::symlink_metadata(&path)?;
                if metadata.is_dir() {
                    pending.push(path);
                } else if metadata.is_file() {
                    let mode = if executables.contains(&path) {
                        0o755
                    } else {
                        0o644
                    };
                    fs::set_permissions(&path, fs::Permissions::from_mode(mode))?;
                }
            }
        }
    }
    #[cfg(not(unix))]
    let _ = (package, executables);
    Ok(())
}

/// Move every rebuilt file over the installed one, keeping the replaced file as
/// `<name>.captures-previous` until the new version confirms startup. A failure
/// restores what it already moved. Files the release dropped stay; the
/// uninstaller removes the folder.
fn replace_files(
    installed: &Path,
    package: &Path,
) -> Result<Vec<(PathBuf, Option<PathBuf>)>, Error> {
    let mut files = Vec::new();
    let mut pending = vec![(package.to_owned(), installed.to_owned())];
    while let Some((from, to)) = pending.pop() {
        for item in fs::read_dir(&from)? {
            let item = item?;
            let metadata = fs::symlink_metadata(item.path())?;
            let target = to.join(item.file_name());
            if metadata.is_dir() {
                pending.push((item.path(), target));
            } else if metadata.is_file() {
                files.push((item.path(), target));
            } else {
                return Err(Error::Package);
            }
        }
    }
    let mut replaced: Vec<(PathBuf, Option<PathBuf>)> = Vec::new();
    let result = (|| -> Result<(), Error> {
        for (new, target) in &files {
            fs::create_dir_all(target.parent().ok_or(Error::Package)?)?;
            let previous = target.exists().then(|| {
                let mut name = target.file_name().unwrap_or_default().to_owned();
                name.push(".captures-previous");
                target.with_file_name(name)
            });
            if let Some(previous) = &previous {
                let _ = fs::remove_file(previous);
                retry(|| fs::rename(target, previous))?;
            }
            replaced.push((target.clone(), previous));
            retry(|| fs::rename(new, target))?;
        }
        Ok(())
    })();
    if let Err(error) = result {
        for (target, previous) in replaced.iter().rev() {
            let _ = fs::remove_file(target);
            if let Some(previous) = previous {
                let _ = fs::rename(previous, target);
            }
        }
        return Err(error);
    }
    Ok(replaced)
}

/// Windows may hold a just-closed executable or scanner-opened file briefly.
fn retry(mut operation: impl FnMut() -> io::Result<()>) -> Result<(), Error> {
    let start = std::time::Instant::now();
    loop {
        match operation() {
            Ok(()) => return Ok(()),
            Err(error)
                if cfg!(windows)
                    && matches!(error.raw_os_error(), Some(5 | 32))
                    && start.elapsed() < Duration::from_secs(30) =>
            {
                std::thread::sleep(Duration::from_millis(250))
            }
            Err(error) => return Err(error.into()),
        }
    }
}

/// Keep Settings > Apps showing the installed version after a patch install;
/// the NSIS installer records it under this per-user uninstall key.
#[cfg(windows)]
fn set_display_version(version: &str) {
    let name = crate::preview_channel::APP_BUNDLE.trim_end_matches(".app");
    if let Ok(key) = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey_with_flags(
            format!("Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\{name}"),
            winreg::enums::KEY_SET_VALUE,
        )
    {
        let _ = key.set_value("DisplayVersion", &version);
    }
}

#[cfg(not(windows))]
fn set_display_version(_: &str) {}

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

/// Starts a program this process just wrote or copied. A thread that forks
/// while that file is still open for writing hands its child the descriptor
/// until the child execs, so the exec can briefly fail with ETXTBSY on Linux.
/// Retry that instead of failing an update or recovery whose files are in place.
fn spawn_fresh(command: &mut Command) -> std::io::Result<std::process::Child> {
    let started = std::time::Instant::now();
    loop {
        match command.spawn() {
            Err(error)
                if error.kind() == std::io::ErrorKind::ExecutableFileBusy
                    && started.elapsed() < Duration::from_secs(2) =>
            {
                std::thread::sleep(Duration::from_millis(20))
            }
            result => return result,
        }
    }
}

/// `installer`: `executable` is the verified NSIS installer, which relaunches
/// Captures itself.
fn launch_and_confirm(
    executable: &Path,
    directory: &Path,
    request: &InstallRequest,
    installer: bool,
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
            preferences_stacking: request.preferences_stacking,
            capture_stacking: request.capture_stacking.clone(),
        },
    )?;
    let windows = installer;
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
    } else if let InstallTarget::Mac { app } = &request.installation {
        // Launch through LaunchServices, never as this helper's child. macOS
        // attributes a child's Screen Recording access to its responsible
        // process, here this temp-copied helper with its own code identity,
        // so a directly spawned Captures got only the wallpaper and its own
        // windows from every capture until the user reopened it. `-W` keeps
        // `open` alive while the new app runs, so exiting still means it died.
        Some(
            Command::new("/usr/bin/open")
                .arg("-n")
                .arg("-W")
                .arg("-a")
                .arg(app)
                .arg("--args")
                .arg("--live")
                .arg("--native-update-ready-file")
                .arg(&ready)
                .arg("--native-update-ready-token")
                .arg(&token)
                .stdin(Stdio::null())
                .spawn()?,
        )
    } else {
        Some(spawn_fresh(
            Command::new(executable)
                .env_remove("APPIMAGE")
                .env_remove("APPDIR")
                .arg("--live")
                .arg("--native-update-ready-file")
                .arg(&ready)
                .arg("--native-update-ready-token")
                .arg(&token)
                .stdin(Stdio::null()),
        )?)
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
                "new version did not confirm startup; previous package retained (see backup.json)"
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
        if components.next()
            != Some(Component::Normal(
                crate::preview_channel::APP_BUNDLE.as_ref(),
            ))
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
    fn changelog_keeps_releases_after_the_current_one_newest_first() {
        let entry = |version: &str, display: Option<&str>| ChangelogEntry {
            version: version.into(),
            display_version: display.map(Into::into),
            notes: Some(format!("* {version}")),
        };
        let stacked = stacked_changelog(
            vec![
                entry("2026.11.2201", None),
                entry("2026.11.2203", Some("2026.11.22.3")),
                entry("2026.11.2110", None),
                entry("2026.11.2204", None),
                entry("not a version", None),
                entry("2026.11.2202", None),
            ],
            &Version::parse("2026.11.2110").unwrap(),
            &Version::parse("2026.11.2203").unwrap(),
        );
        assert_eq!(
            stacked
                .iter()
                .map(|e| (e.version.as_str(), e.display_version.as_str()))
                .collect::<Vec<_>>(),
            [
                ("2026.11.2203", "2026.11.22.3"),
                ("2026.11.2202", "2026.11.22.2"),
                ("2026.11.2201", "2026.11.22.1"),
            ]
        );
    }

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
            // Verifying follows the last byte, once, whether or not the
            // package then passes.
            let events = std::cell::RefCell::new(Vec::new());
            let result = pending.download_verifying(
                directory.path(),
                &cancel,
                |downloaded, _| events.borrow_mut().push(Some(downloaded)),
                || events.borrow_mut().push(None),
            );
            let events = events.into_inner();
            assert_eq!(events.last(), Some(&None), "{events:?}");
            assert_eq!(events.iter().filter(|event| event.is_none()).count(), 1);
            assert_eq!(
                events[events.len() - 2],
                Some(downloaded.len() as u64),
                "{events:?}"
            );
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
    fn patch_updates_rebuild_the_installed_package_or_fall_back_to_the_full_artifact() {
        let pair = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let key = pair.pk.to_box().unwrap().to_string();
        let sign = |bytes: &[u8], name: &str| {
            let comment = format!("timestamp:1\tfile:{name}");
            let signature =
                minisign::sign(None, &pair.sk, Cursor::new(bytes), Some(&comment), None).unwrap();
            crate::editor_layers::base64(signature.into_string().as_bytes())
        };
        let root = tempfile::tempdir().unwrap();
        // Incompressible, so the patch is a real saving only through bsdiff.
        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        let old: Vec<u8> = (0..300_000)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state as u8
            })
            .collect();
        let mut new = old.clone();
        new[4096..4105].copy_from_slice(b"new build");
        let full = b"the full updater artifact".to_vec();
        let write = |path: PathBuf, bytes: &[u8]| {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, bytes).unwrap();
        };
        // Independent of artifact_name and patch_name, the code under test.
        for (target, renderer, platform, folder, program, full_name, patch_name) in [
            (
                Target::MacArm64,
                Renderer::Appkit,
                "darwin-aarch64",
                "Captures.app",
                "Contents/MacOS/captures",
                "Captures-2026.11.2110-aarch64-apple-darwin.app.tar.gz",
                "Captures-2026.11.2110-aarch64-apple-darwin.from-2026.11.2109.capturespatch",
            ),
            (
                Target::WindowsX64,
                Renderer::Wgpu,
                "windows-x86_64",
                "Captures",
                "captures.exe",
                "Captures-2026.11.2110-x86_64-pc-windows-msvc-setup.exe",
                "Captures-2026.11.2110-x86_64-pc-windows-msvc.from-2026.11.2109.capturespatch",
            ),
        ] {
            let case = root.path().join(platform);
            let installed = case.join("installed").join(folder);
            let released = case.join("released").join(folder);
            for (package, binary, notice) in [
                (&installed, &old, b"old notice".as_slice()),
                (&released, &new, b"new notice"),
            ] {
                write(package.join(program), binary);
                write(package.join("notices/LICENSE"), b"same license");
                write(package.join("notices/ffmpeg.txt"), notice);
            }
            let output = case.join(patch_name);
            let summary = delta::make(
                target,
                &installed,
                &Version::parse("2026.11.2109").unwrap(),
                &released,
                &Version::parse("2026.11.2110").unwrap(),
                &output,
            )
            .unwrap();
            assert_eq!((summary.copied, summary.bsdiff), (1, 1), "{summary:?}");
            let patch = fs::read(&output).unwrap();
            assert!(patch.len() < 20_000, "{} bytes", patch.len());
            let installation = match target {
                Target::WindowsX64 => InstallTarget::Windows {
                    executable: installed.join(program),
                },
                _ => InstallTarget::Mac {
                    app: installed.clone(),
                },
            };
            // The patch applies; the installed files changed; and a patch
            // signed for another starting version, with a full artifact signed
            // for another release, never installs anything.
            for scenario in ["patch", "changed install", "wrong names"] {
                let listener = TcpListener::bind("127.0.0.1:0").unwrap();
                let url = format!("http://{}", listener.local_addr().unwrap());
                let (full_signed, patch_signed) = if scenario == "wrong names" {
                    (
                        full_name.replace("2110", "2111"),
                        patch_name.replace("from-2026.11.2109", "from-2026.11.2108"),
                    )
                } else {
                    (full_name.to_owned(), patch_name.to_owned())
                };
                let metadata = serde_json::to_vec(&serde_json::json!({
                    "version": "2026.11.2110",
                    "platforms": {platform: {
                        "url": format!("{url}/full"), "signature": sign(&full, &full_signed),
                        "size": 10 * patch.len(),
                        "patches": [
                            {"from": "2026.11.2108", "url": format!("{url}/older"), "size": 1, "signature": "c2ln"},
                            {"from": "2026.11.2109", "url": format!("{url}/patch"),
                                "size": patch.len(), "signature": sign(&patch, &patch_signed)},
                        ]
                    }}
                }))
                .unwrap();
                // Copied unchanged, so the rebuild must read and check it.
                let license = installed.join("notices/LICENSE");
                if scenario == "changed install" {
                    fs::write(&license, b"same license, edited").unwrap();
                }
                let mut responses = vec![(200, metadata), (200, patch.clone())];
                if scenario != "patch" {
                    responses.push((200, full.clone()));
                }
                let server = super::super::tests::serve(listener, responses);
                let client = Client::new(
                    &[&format!("{url}/latest.json")],
                    &key,
                    renderer,
                    target,
                    "2026.11.2109",
                    installation.clone(),
                )
                .unwrap();
                let cancel = CancelToken::default();
                let pending = client.check(&cancel).unwrap().unwrap();
                assert_eq!(pending.info.size, patch.len() as u64);
                let events = std::cell::RefCell::new(Vec::new());
                let directory = tempfile::tempdir().unwrap();
                let result = pending.download_verifying(
                    directory.path(),
                    &cancel,
                    |_, total| events.borrow_mut().push(Some(total)),
                    || events.borrow_mut().push(None),
                );
                // Verifying comes once, last, also after a failed patch.
                let events = events.into_inner();
                assert_eq!(events.iter().filter(|e| e.is_none()).count(), 1);
                assert_eq!(events.last(), Some(&None), "{platform} {scenario}");
                let mut expected = vec!["/latest.json", "/patch"];
                match scenario {
                    "patch" => {
                        let download = result.unwrap();
                        let folder = download.directory.path();
                        assert_eq!(fs::read(folder.join("patch")).unwrap(), patch);
                        assert_eq!(download.request.patch.as_deref(), Some("2026.11.2109"));
                        assert!(!folder.join("artifact").exists());
                        // Only an AppImage keeps the proving rebuild.
                        assert!(!folder.join("stage").exists());
                    }
                    "changed install" => {
                        let download = result.unwrap();
                        let folder = download.directory.path();
                        assert_eq!(fs::read(folder.join("artifact")).unwrap(), full);
                        assert_eq!(download.request.patch, None);
                        assert!(!folder.join("patch").exists());
                        assert_eq!(events[events.len() - 2], Some(full.len() as u64));
                        expected.push("/full");
                        fs::write(&license, b"same license").unwrap();
                    }
                    _ => {
                        assert!(matches!(result, Err(Error::Signature)), "{platform}");
                        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
                        expected.push("/full");
                    }
                }
                assert_eq!(server.join().unwrap(), expected, "{platform} {scenario}");
            }
            // The installed package is only ever read.
            assert_eq!(fs::read(installed.join(program)).unwrap(), old);
        }
    }

    #[test]
    fn replaced_files_keep_the_previous_ones_and_a_failure_restores_them() {
        let root = tempfile::tempdir().unwrap();
        let installed = root.path().join("Captures");
        let package = root.path().join("package");
        for (folder, prefix) in [(&installed, "old"), (&package, "new")] {
            fs::create_dir_all(folder.join("notices")).unwrap();
            fs::write(folder.join("captures.exe"), format!("{prefix} exe")).unwrap();
            fs::write(
                folder.join("notices/ffmpeg.txt"),
                format!("{prefix} notice"),
            )
            .unwrap();
        }
        fs::write(package.join("added.dll"), "added").unwrap();
        fs::write(installed.join("dropped.dll"), "dropped").unwrap();
        let replaced = replace_files(&installed, &package).unwrap();
        assert_eq!(
            fs::read(installed.join("captures.exe")).unwrap(),
            b"new exe"
        );
        assert_eq!(
            fs::read(installed.join("notices/ffmpeg.txt")).unwrap(),
            b"new notice"
        );
        assert_eq!(fs::read(installed.join("added.dll")).unwrap(), b"added");
        assert_eq!(fs::read(installed.join("dropped.dll")).unwrap(), b"dropped");
        assert_eq!(
            fs::read(installed.join("captures.exe.captures-previous")).unwrap(),
            b"old exe"
        );
        assert_eq!(replaced.len(), 3);
        assert_eq!(
            replaced
                .iter()
                .filter(|(_, previous)| previous.is_none())
                .count(),
            1
        );

        // Top-level files move before nested ones, so this fails after
        // replacing them: `notices` is a file where the package has a folder.
        let installed = root.path().join("failing");
        fs::create_dir_all(&installed).unwrap();
        fs::write(installed.join("captures.exe"), "old exe").unwrap();
        fs::write(installed.join("notices"), "a file").unwrap();
        let package = root.path().join("failing-package");
        fs::create_dir_all(package.join("notices")).unwrap();
        fs::write(package.join("captures.exe"), "new exe").unwrap();
        fs::write(package.join("added.dll"), "added").unwrap();
        fs::write(package.join("notices/ffmpeg.txt"), "new notice").unwrap();
        assert!(replace_files(&installed, &package).is_err());
        let mut left: Vec<_> = fs::read_dir(&installed)
            .unwrap()
            .map(|item| item.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(left, ["captures.exe", "notices"]);
        assert_eq!(
            fs::read(installed.join("captures.exe")).unwrap(),
            b"old exe"
        );
        assert_eq!(fs::read(installed.join("notices")).unwrap(), b"a file");
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
