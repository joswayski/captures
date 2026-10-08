//! Native-development update acquisition, independent of Tauri and host windows.
//!
//! A pinned Minisign key authenticates the exact manifest bytes *before* parsing.
//! The signed manifest binds the development identity, renderer, target, version,
//! artifact URL, byte count and SHA-256. Downloads remain private temporary files
//! until all checks succeed. Staging validates packages in private temporary
//! directories. Explicit replacement retains the previous development package
//! until confirmation, with interruption recovery. An opt-in external helper can
//! launch it with a new empty or explicitly imported disposable profile and
//! confirm its private readiness. No GUI calls replacement, discovers installed
//! data, registers it or activates a channel.
//! No endpoint/key is enabled by default; construct and call on a worker thread.
pub mod checks;
mod delta;
mod health;
mod installation;
mod staging;
pub use health::{HealthAcknowledgement, take_restart_preferences};
pub use installation::{LaunchFailure, PackageUse, PendingInstallation, recover_installation};
pub use staging::StagedUpdate;

use std::{
    collections::BTreeMap,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use captures_media::CancelToken;
use minisign_verify::{PublicKey, Signature};
use reqwest::{
    Url,
    blocking::{Client, Response},
    redirect::Policy,
};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;

pub const DEVELOPMENT_IDENTITY: &str = "es.captur.native-development";
const MAX_MANIFEST_BYTES: u64 = 256 * 1024;
const MAX_SIGNATURE_BYTES: u64 = 8 * 1024;
const MAX_ARTIFACT_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Renderer {
    Appkit,
    Wgpu,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Target {
    #[serde(rename = "aarch64-apple-darwin")]
    MacArm64,
    #[serde(rename = "x86_64-apple-darwin")]
    MacX64,
    #[serde(rename = "x86_64-pc-windows-msvc")]
    WindowsX64,
    #[serde(rename = "x86_64-unknown-linux-gnu")]
    LinuxX64,
}

impl Target {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MacArm64 => "aarch64-apple-darwin",
            Self::MacX64 => "x86_64-apple-darwin",
            Self::WindowsX64 => "x86_64-pc-windows-msvc",
            Self::LinuxX64 => "x86_64-unknown-linux-gnu",
        }
    }

    pub fn current_host() -> Option<Self> {
        match (std::env::consts::OS, std::env::consts::ARCH) {
            ("macos", "aarch64") => Some(Self::MacArm64),
            ("macos", "x86_64") => Some(Self::MacX64),
            ("windows", "x86_64") => Some(Self::WindowsX64),
            ("linux", "x86_64") => Some(Self::LinuxX64),
            _ => None,
        }
    }

    fn package_executable(self) -> &'static str {
        match self {
            Self::MacArm64 | Self::MacX64 => {
                "Captures Native Development.app/Contents/MacOS/CapturesNative"
            }
            Self::WindowsX64 => "CapturesNative.exe",
            Self::LinuxX64 => "captures-native",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Invalid native update configuration: {0}")]
    Configuration(&'static str),
    #[error("Could not reach the native update service.")]
    Network,
    #[error("Native update service returned HTTP {0}.")]
    Http(u16),
    #[error("Native update metadata exceeds its size limit.")]
    MetadataTooLarge,
    #[error("Native update signature verification failed.")]
    Signature,
    #[error("Invalid native update manifest.")]
    Manifest,
    #[error("Unsupported native update manifest schema.")]
    Schema,
    #[error("This update is not for Captures Native Development.")]
    Identity,
    #[error("This update is for a different native renderer.")]
    Renderer,
    #[error("This update has no artifact for the current platform.")]
    Target,
    #[error("Invalid native update version.")]
    Version,
    #[error("Native update artifact size does not match its signed manifest.")]
    Size,
    #[error("Native update artifact hash does not match its signed manifest.")]
    Hash,
    #[error("Native update archive is invalid or exceeds its limits.")]
    Archive,
    #[error("Native update archive contains an unsafe or conflicting path.")]
    ArchivePath,
    #[error("Native update package is incomplete or has the wrong development identity.")]
    Package,
    #[error("Native update replacement cannot proceed: {0}")]
    Installation(&'static str),
    #[error("Native update cancelled.")]
    Cancelled,
    #[error("Native update file operation failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("Native development profile import failed: {0}")]
    ProfileImport(#[from] crate::profile_import::Error),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema: u32,
    identity: String,
    renderer: Renderer,
    version: String,
    notes: Option<String>,
    artifacts: BTreeMap<String, Artifact>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Artifact {
    url: String,
    size: u64,
    sha256: String,
    delta: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ReleaseInfo {
    pub renderer: Renderer,
    pub target: Target,
    pub version: String,
    pub notes: Option<String>,
    pub size: u64,
}

pub struct UpdateClient {
    metadata: Client,
    download: Client,
    endpoint: Url,
    signature_endpoint: Url,
    key: PublicKey,
    renderer: Renderer,
    target: Target,
    current_version: Version,
    loopback: bool,
    base_archive: Option<PathBuf>,
}

impl std::fmt::Debug for UpdateClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UpdateClient")
            .field("renderer", &self.renderer)
            .field("target", &self.target)
            .field("current_version", &self.current_version)
            .finish_non_exhaustive()
    }
}

/// Only a successfully authenticated, newer, matching manifest creates this.
pub struct PendingUpdate {
    client: Client,
    info: ReleaseInfo,
    url: Url,
    hash: [u8; 32],
    delta: Option<delta::Candidate>,
    base_archive: Option<PathBuf>,
}

/// Verified bytes, not an installed update. Dropping removes the private file.
/// Installers must retain ownership and verify again if they reopen by path.
pub struct VerifiedUpdate {
    info: ReleaseInfo,
    file: NamedTempFile,
    hash: [u8; 32],
}

impl UpdateClient {
    /// `public_key` is standard two-line Minisign text, not Tauri's outer base64.
    /// The detached signature lives at `<manifest path>.minisig`. HTTP is allowed
    /// only for loopback diagnostics; real endpoints and redirects require HTTPS.
    /// No request occurs during construction and no system/profile state changes.
    pub fn new(
        endpoint: &str,
        public_key: &str,
        renderer: Renderer,
        target: Target,
        current_version: &str,
    ) -> Result<Self, Error> {
        if renderer == Renderer::Appkit && !matches!(target, Target::MacArm64 | Target::MacX64) {
            return Err(Error::Configuration("AppKit requires a macOS target"));
        }
        let endpoint = Url::parse(endpoint).map_err(|_| Error::Configuration("invalid URL"))?;
        let loopback = is_loopback(&endpoint);
        validate_url(&endpoint, loopback)?;
        let mut signature_endpoint = endpoint.clone();
        signature_endpoint.set_path(&format!("{}.minisig", endpoint.path()));
        let key = PublicKey::decode(public_key)
            .map_err(|_| Error::Configuration("invalid pinned public key"))?;
        let current_version = Version::parse(current_version)
            .map_err(|_| Error::Configuration("invalid current version"))?;
        let metadata = http_client(Policy::none())?;
        let download = http_client(Policy::custom(move |attempt| {
            if attempt.previous().len() >= 5 {
                attempt.error("too many native update redirects")
            } else if validate_url(attempt.url(), loopback).is_err() {
                attempt.error("native update redirect requires HTTPS")
            } else {
                attempt.follow()
            }
        }))?;
        Ok(Self {
            metadata,
            download,
            endpoint,
            signature_endpoint,
            key,
            renderer,
            target,
            current_version,
            loopback,
            base_archive: None,
        })
    }

    /// An explicit retained archive, never an installed package/profile discovered
    /// by the updater. Construction performs no file access. Missing, changed or
    /// incompatible bytes fall back to the authenticated full download.
    pub fn with_base_archive(mut self, path: PathBuf) -> Result<Self, Error> {
        if !path.is_absolute() {
            return Err(Error::Configuration("delta base archive must be absolute"));
        }
        self.base_archive = Some(path);
        Ok(self)
    }

    pub fn check(&self, cancel: &CancelToken) -> Result<Option<PendingUpdate>, Error> {
        let manifest = fetch_metadata(&self.metadata, &self.endpoint, MAX_MANIFEST_BYTES, cancel)?;
        let signature = fetch_metadata(
            &self.metadata,
            &self.signature_endpoint,
            MAX_SIGNATURE_BYTES,
            cancel,
        )?;
        check_cancel(cancel)?;
        self.authenticate(&manifest, &signature)
    }

    fn authenticate(&self, bytes: &[u8], signature: &[u8]) -> Result<Option<PendingUpdate>, Error> {
        if bytes.len() as u64 > MAX_MANIFEST_BYTES || signature.len() as u64 > MAX_SIGNATURE_BYTES {
            return Err(Error::MetadataTooLarge);
        }
        let signature = std::str::from_utf8(signature)
            .ok()
            .and_then(|text| Signature::decode(text).ok())
            .ok_or(Error::Signature)?;
        self.key
            .verify(bytes, &signature, false)
            .map_err(|_| Error::Signature)?;
        let manifest: Manifest = serde_json::from_slice(bytes).map_err(|_| Error::Manifest)?;
        if !matches!(manifest.schema, 1 | 2) {
            return Err(Error::Schema);
        }
        if manifest.identity != DEVELOPMENT_IDENTITY {
            return Err(Error::Identity);
        }
        if manifest.renderer != self.renderer {
            return Err(Error::Renderer);
        }
        let version = Version::parse(&manifest.version).map_err(|_| Error::Version)?;
        let artifact = manifest
            .artifacts
            .get(self.target.as_str())
            .ok_or(Error::Target)?;
        if artifact.size == 0 || artifact.size > MAX_ARTIFACT_BYTES {
            return Err(Error::Size);
        }
        let hash = decode_hash(&artifact.sha256)?;
        let url = Url::parse(&artifact.url).map_err(|_| Error::Manifest)?;
        validate_url(&url, self.loopback)?;
        // SemVer ignores build metadata; equal/older versions never install.
        if version.cmp_precedence(&self.current_version).is_le() {
            return Ok(None);
        }
        // Schema 1 remains full-only. Unsupported/incompatible delta descriptors do
        // not remove the independently authenticated full-artifact capability.
        let delta = (manifest.schema == 2)
            .then(|| {
                artifact.delta.as_ref().and_then(|description| {
                    delta::Candidate::new(
                        description,
                        &self.current_version,
                        artifact.size,
                        self.loopback,
                    )
                    .ok()
                })
            })
            .flatten();
        Ok(Some(PendingUpdate {
            client: self.download.clone(),
            info: ReleaseInfo {
                renderer: self.renderer,
                target: self.target,
                version: manifest.version,
                notes: manifest.notes,
                size: artifact.size,
            },
            url,
            hash,
            delta,
            base_archive: self.base_archive.clone(),
        }))
    }
}

impl PendingUpdate {
    pub fn info(&self) -> &ReleaseInfo {
        &self.info
    }

    /// Retrying uses the same authenticated metadata and a fresh private file.
    /// Cancellation is checked at I/O boundaries; blocked HTTP is bounded by the
    /// client's 60-second total request timeout, not an immediate interrupt.
    pub fn download(
        &self,
        directory: &Path,
        cancel: &CancelToken,
        mut progress: impl FnMut(u64, u64),
    ) -> Result<VerifiedUpdate, Error> {
        check_cancel(cancel)?;
        if let (Some(delta), Some(base)) = (&self.delta, &self.base_archive) {
            match delta.download(self, base, directory, cancel, &mut progress) {
                Ok(verified) => return Ok(verified),
                Err(Error::Cancelled) => return Err(Error::Cancelled),
                Err(_) => check_cancel(cancel)?,
            }
        }
        // Also resets byte progress and total after an unsuccessful patch.
        progress(0, self.info.size);
        check_cancel(cancel)?;
        let response = self
            .client
            .get(self.url.clone())
            .send()
            .map_err(|_| Error::Network)?;
        let response = successful(response)?;
        if response
            .content_length()
            .is_some_and(|size| size != self.info.size)
        {
            return Err(Error::Size);
        }
        self.verify_download(response, directory, cancel, progress)
    }

    fn verify_download(
        &self,
        reader: impl Read,
        directory: &Path,
        cancel: &CancelToken,
        progress: impl FnMut(u64, u64),
    ) -> Result<VerifiedUpdate, Error> {
        verify_file(reader, directory, &self.info, self.hash, cancel, progress)
    }
}

impl VerifiedUpdate {
    pub fn info(&self) -> &ReleaseInfo {
        &self.info
    }

    pub fn file(&self) -> &std::fs::File {
        self.file.as_file()
    }
}

fn verify_file(
    reader: impl Read,
    directory: &Path,
    info: &ReleaseInfo,
    hash: [u8; 32],
    cancel: &CancelToken,
    mut progress: impl FnMut(u64, u64),
) -> Result<VerifiedUpdate, Error> {
    check_cancel(cancel)?;
    let mut file = NamedTempFile::new_in(directory)?;
    // Acquisition and staging both use the signed byte limit, even without
    // Content-Length or after another holder mutates the downloaded handle.
    let mut reader = reader.take(info.size + 1);
    let mut hasher = Sha256::new();
    let mut downloaded = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        check_cancel(cancel)?;
        let count = reader.read(&mut buffer)?;
        check_cancel(cancel)?;
        if count == 0 {
            break;
        }
        downloaded += count as u64;
        if downloaded > info.size {
            return Err(Error::Size);
        }
        file.write_all(&buffer[..count])?;
        hasher.update(&buffer[..count]);
        progress(downloaded, info.size);
    }
    if downloaded != info.size {
        return Err(Error::Size);
    }
    if <[u8; 32]>::from(hasher.finalize()) != hash {
        return Err(Error::Hash);
    }
    file.as_file_mut().sync_all()?;
    file.seek(SeekFrom::Start(0))?;
    check_cancel(cancel)?;
    Ok(VerifiedUpdate {
        info: info.clone(),
        file,
        hash,
    })
}

fn http_client(redirect: Policy) -> Result<Client, Error> {
    Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(60))
        .redirect(redirect)
        .user_agent(concat!("Captures-Native/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|_| Error::Network)
}

fn is_loopback(url: &Url) -> bool {
    url.host_str().is_some_and(|host| {
        host == "localhost"
            || host
                .trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .is_ok_and(|address| address.is_loopback())
    })
}

fn validate_url(url: &Url, allow_loopback: bool) -> Result<(), Error> {
    if (url.scheme() != "https" && !(allow_loopback && url.scheme() == "http" && is_loopback(url)))
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(Error::Configuration(
            "updates require credential-free HTTPS URLs",
        ));
    }
    Ok(())
}

fn decode_hash(text: &str) -> Result<[u8; 32], Error> {
    if text.len() != 64 || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(Error::Manifest);
    }
    let mut hash = [0; 32];
    for (index, byte) in hash.iter_mut().enumerate() {
        *byte =
            u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).map_err(|_| Error::Manifest)?;
    }
    Ok(hash)
}

fn check_cancel(cancel: &CancelToken) -> Result<(), Error> {
    if cancel.is_cancelled() {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}

fn successful(response: Response) -> Result<Response, Error> {
    if response.status().is_success() {
        Ok(response)
    } else {
        Err(Error::Http(response.status().as_u16()))
    }
}

fn fetch_metadata(
    client: &Client,
    url: &Url,
    limit: u64,
    cancel: &CancelToken,
) -> Result<Vec<u8>, Error> {
    check_cancel(cancel)?;
    let response = successful(client.get(url.clone()).send().map_err(|_| Error::Network)?)?;
    if response
        .content_length()
        .is_some_and(|length| length > limit)
    {
        return Err(Error::MetadataTooLarge);
    }
    let mut bytes = Vec::new();
    response.take(limit + 1).read_to_end(&mut bytes)?;
    check_cancel(cancel)?;
    if bytes.len() as u64 > limit {
        return Err(Error::MetadataTooLarge);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::{
        io::{BufRead, BufReader, Cursor},
        net::TcpListener,
        thread,
    };

    const PAYLOAD: &[u8] = b"native fixture bytes\0\xff\x01";
    // SHA-256 independently calculated with Python hashlib, not the verifier.
    const PAYLOAD_HASH: &str = "d2820340a902904952ed3ce50313a4867f8717eadfbbc24378a863d52d2009cf";

    pub(super) fn manifest(url: &str) -> Value {
        json!({
            "schema": 1,
            "identity": DEVELOPMENT_IDENTITY,
            "renderer": "wgpu",
            "version": "2026.10.50",
            "notes": "Native development fixture only.",
            "artifacts": {"x86_64-unknown-linux-gnu": {
                "url": url, "size": PAYLOAD.len(), "sha256": PAYLOAD_HASH
            }}
        })
    }

    pub(super) fn signed(value: &Value) -> (String, Vec<u8>, Vec<u8>) {
        let pair = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let bytes = serde_json::to_vec(value).unwrap();
        let signature = minisign::sign(None, &pair.sk, Cursor::new(&bytes), None, None).unwrap();
        (
            pair.pk.to_box().unwrap().to_string(),
            bytes,
            signature.into_string().into_bytes(),
        )
    }

    fn client(key: &str, current: &str) -> UpdateClient {
        UpdateClient::new(
            "http://127.0.0.1:1/native.json",
            key,
            Renderer::Wgpu,
            Target::LinuxX64,
            current,
        )
        .unwrap()
    }

    fn pending() -> PendingUpdate {
        let (key, bytes, signature) = signed(&manifest("http://127.0.0.1:1/artifact"));
        client(&key, "2026.9.99")
            .authenticate(&bytes, &signature)
            .unwrap()
            .unwrap()
    }

    #[test]
    fn signature_authenticates_exact_bytes_before_json_parsing() {
        let (key, mut bytes, signature) = signed(&manifest("https://example.invalid/native.tar"));
        let client = client(&key, "2026.9.99");
        assert_eq!(
            client
                .authenticate(&bytes, &signature)
                .unwrap()
                .unwrap()
                .info()
                .version,
            "2026.10.50"
        );
        bytes.push(b' '); // Still valid JSON, but not the signed message.
        assert!(matches!(
            client.authenticate(&bytes, &signature),
            Err(Error::Signature)
        ));
        assert!(matches!(
            client.authenticate(b"not JSON", &signature),
            Err(Error::Signature)
        ));
        let (other_key, _, _) = signed(&manifest("https://example.invalid/native.tar"));
        assert!(matches!(
            super::UpdateClient::new(
                "https://example.invalid/native.json",
                &other_key,
                Renderer::Wgpu,
                Target::LinuxX64,
                "2026.9.99"
            )
            .unwrap()
            .authenticate(&bytes[..bytes.len() - 1], &signature),
            Err(Error::Signature)
        ));
    }

    #[test]
    fn signed_metadata_must_match_identity_renderer_target_and_schema() {
        for (field, replacement) in [
            ("identity", json!("es.captur.app")),
            ("renderer", json!("appkit")),
            ("schema", json!(3)),
            ("version", json!("not a version")),
        ] {
            let mut value = manifest("https://example.invalid/native.tar");
            value[field] = replacement;
            let (key, bytes, signature) = signed(&value);
            let result = client(&key, "2026.9.99").authenticate(&bytes, &signature);
            assert!(
                match field {
                    "identity" => matches!(result, Err(Error::Identity)),
                    "renderer" => matches!(result, Err(Error::Renderer)),
                    "schema" => matches!(result, Err(Error::Schema)),
                    "version" => matches!(result, Err(Error::Version)),
                    _ => unreachable!(),
                },
                "{field}"
            );
        }
        let (key, bytes, signature) = signed(&manifest("https://example.invalid/native.tar"));
        let windows = UpdateClient::new(
            "https://example.invalid/native.json",
            &key,
            Renderer::Wgpu,
            Target::WindowsX64,
            "2026.9.99",
        )
        .unwrap();
        assert!(matches!(
            windows.authenticate(&bytes, &signature),
            Err(Error::Target)
        ));
    }

    #[test]
    fn every_supported_renderer_and_architecture_selects_its_own_signed_artifact() {
        for (renderer, target) in [
            (Renderer::Appkit, Target::MacArm64),
            (Renderer::Appkit, Target::MacX64),
            (Renderer::Wgpu, Target::MacArm64),
            (Renderer::Wgpu, Target::MacX64),
            (Renderer::Wgpu, Target::WindowsX64),
            (Renderer::Wgpu, Target::LinuxX64),
        ] {
            let mut value = manifest("https://example.invalid/wrong-artifact");
            value["renderer"] = json!(renderer);
            value["artifacts"][target.as_str()] = json!({
                "url": "https://example.invalid/correct-artifact", "size": 37,
                "sha256": "a".repeat(64),
            });
            let (key, bytes, signature) = signed(&value);
            let update = UpdateClient::new(
                "https://example.invalid/native.json",
                &key,
                renderer,
                target,
                "2026.9.99",
            )
            .unwrap()
            .authenticate(&bytes, &signature)
            .unwrap()
            .unwrap();
            assert_eq!(update.info().renderer, renderer);
            assert_eq!(update.info().target, target);
            assert_eq!(update.info().size, 37);
            assert_eq!(update.url.path(), "/correct-artifact");
        }
    }

    #[test]
    fn legacy_minisign_signatures_are_rejected_not_treated_as_unsigned_json() {
        // Upstream minisign-verify's independently signed legacy `b"test"` vector.
        let key = "untrusted comment: minisign public key E7620F1842B4E81F\nRWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3";
        let signature = b"untrusted comment: signature from minisign secret key\nRWQf6LRCGA9i59SLOFxz6NxvASXDJeRtuZykwQepbDEGt87ig1BNpWaVWuNrm73YiIiJbq71Wi+dP9eKL8OC351vwIasSSbXxwA=\ntrusted comment: timestamp:1555779966\tfile:test\nQtKMXWyYcwdpZAlPF7tE2ENJkRd1ujvKjlj1m9RtHTBnZPa5WKU5uWRs5GoP5M/VqE81QFuMKI5k/SfNQUaOAA==";
        PublicKey::decode(key)
            .unwrap()
            .verify(
                b"test",
                &Signature::decode(std::str::from_utf8(signature).unwrap()).unwrap(),
                true,
            )
            .unwrap();
        assert!(matches!(
            client(key, "2026.9.99").authenticate(b"test", signature),
            Err(Error::Signature)
        ));
    }

    #[test]
    fn semantic_versions_reject_downgrades_equal_versions_and_build_only_changes() {
        let (key, bytes, signature) = signed(&manifest("https://example.invalid/native.tar"));
        for current in [
            "2026.10.50",
            "2026.10.50+different",
            "2026.10.51",
            "2026.11.1",
        ] {
            assert!(
                client(&key, current)
                    .authenticate(&bytes, &signature)
                    .unwrap()
                    .is_none(),
                "{current}"
            );
        }
        assert!(
            client(&key, "2026.9.999")
                .authenticate(&bytes, &signature)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn signed_artifact_metadata_is_bounded_and_transport_is_not_downgraded() {
        for replacement in [json!(0), json!(MAX_ARTIFACT_BYTES + 1)] {
            let mut value = manifest("https://example.invalid/native.tar");
            value["artifacts"][Target::LinuxX64.as_str()]["size"] = replacement;
            let (key, bytes, signature) = signed(&value);
            assert!(matches!(
                client(&key, "2026.9.99").authenticate(&bytes, &signature),
                Err(Error::Size)
            ));
        }
        for url in [
            "http://example.invalid/artifact",
            "file:///tmp/artifact",
            "https://user:pass@example.invalid/artifact",
            "https://example.invalid/artifact#fragment",
        ] {
            let (key, bytes, signature) = signed(&manifest(url));
            assert!(
                matches!(
                    client(&key, "2026.9.99").authenticate(&bytes, &signature),
                    Err(Error::Configuration(_))
                ),
                "{url}"
            );
        }
        let (key, bytes, signature) = signed(&manifest("http://127.0.0.1/artifact"));
        let production = UpdateClient::new(
            "https://example.invalid/native.json",
            &key,
            Renderer::Wgpu,
            Target::LinuxX64,
            "2026.9.99",
        )
        .unwrap();
        assert!(matches!(
            production.authenticate(&bytes, &signature),
            Err(Error::Configuration(_))
        ));
        assert!(
            UpdateClient::new(
                "https://example.invalid/native.json",
                &key,
                Renderer::Appkit,
                Target::WindowsX64,
                "2026.9.99"
            )
            .is_err()
        );
    }

    #[test]
    fn verified_file_has_exact_bytes_progress_and_is_removed_on_drop() {
        let directory = tempfile::tempdir().unwrap();
        let mut progress = Vec::new();
        let verified = pending()
            .verify_download(
                Cursor::new(PAYLOAD),
                directory.path(),
                &CancelToken::default(),
                |done, total| progress.push((done, total)),
            )
            .unwrap();
        assert_eq!(progress, [(PAYLOAD.len() as u64, PAYLOAD.len() as u64)]);
        assert_eq!(verified.info().version, "2026.10.50");
        let mut actual = Vec::new();
        verified.file().read_to_end(&mut actual).unwrap();
        assert_eq!(actual, PAYLOAD);
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
        drop(verified);
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    #[test]
    fn wrong_hash_truncated_and_extra_bytes_never_leave_a_file() {
        let directory = tempfile::tempdir().unwrap();
        let mut changed = PAYLOAD.to_vec();
        changed[3] ^= 1;
        let mut extra = PAYLOAD.to_vec();
        extra.push(0);
        for (body, wrong_hash) in [
            (changed.as_slice(), true),
            (&PAYLOAD[..PAYLOAD.len() - 1], false),
            (extra.as_slice(), false),
        ] {
            let result = pending().verify_download(
                Cursor::new(body),
                directory.path(),
                &CancelToken::default(),
                |_, _| {},
            );
            assert!(if wrong_hash {
                matches!(result, Err(Error::Hash))
            } else {
                matches!(result, Err(Error::Size))
            });
            assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
        }
    }

    #[test]
    fn cancellation_during_progress_and_before_download_never_publish() {
        let directory = tempfile::tempdir().unwrap();
        let cancel = CancelToken::default();
        let result =
            pending().verify_download(Cursor::new(PAYLOAD), directory.path(), &cancel, |_, _| {
                cancel.cancel()
            });
        assert!(matches!(result, Err(Error::Cancelled)));
        assert!(matches!(
            pending().download(directory.path(), &cancel, |_, _| panic!("cancelled")),
            Err(Error::Cancelled)
        ));
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    #[test]
    fn streaming_hash_progress_cancellation_and_oversize_guard_cross_chunk_boundaries() {
        let body = PAYLOAD.repeat(6001); // 138,023 bytes: two full chunks plus a tail.
        let mut value = manifest("https://example.invalid/native.tar");
        value["artifacts"][Target::LinuxX64.as_str()]["size"] = json!(138_023);
        // Independently calculated with Python hashlib over PAYLOAD repeated 6001 times.
        value["artifacts"][Target::LinuxX64.as_str()]["sha256"] =
            json!("549d4d177a4538eb1d87d4edd2fde009ffb25f8e8f7b58ac099ae32766ef6c6c");
        let (key, bytes, signature) = signed(&value);
        let update = client(&key, "2026.9.99")
            .authenticate(&bytes, &signature)
            .unwrap()
            .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let mut progress = Vec::new();
        let verified = update
            .verify_download(
                Cursor::new(&body),
                directory.path(),
                &CancelToken::default(),
                |done, total| progress.push((done, total)),
            )
            .unwrap();
        assert_eq!(
            progress,
            [(65_536, 138_023), (131_072, 138_023), (138_023, 138_023)]
        );
        let mut actual = Vec::new();
        verified.file().read_to_end(&mut actual).unwrap();
        assert_eq!(actual, body);
        drop(verified);
        let cancel = CancelToken::default();
        let mut callbacks = 0;
        assert!(matches!(
            update.verify_download(Cursor::new(&body), directory.path(), &cancel, |_, _| {
                callbacks += 1;
                cancel.cancel();
            }),
            Err(Error::Cancelled)
        ));
        assert_eq!(callbacks, 1);
        let mut reader = Cursor::new(&body);
        assert!(matches!(
            pending().verify_download(
                &mut reader,
                directory.path(),
                &CancelToken::default(),
                |_, _| {}
            ),
            Err(Error::Size)
        ));
        assert_eq!(
            reader.position(),
            24,
            "consume only the signed 23 bytes plus one overflow byte"
        );
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    #[test]
    fn bounded_signature_and_manifest_and_malformed_hash_are_rejected() {
        let (key, bytes, signature) = signed(&manifest("https://example.invalid/native.tar"));
        let client = client(&key, "2026.9.99");
        assert!(matches!(
            client.authenticate(&vec![b' '; MAX_MANIFEST_BYTES as usize + 1], &signature),
            Err(Error::MetadataTooLarge)
        ));
        assert!(matches!(
            client.authenticate(&bytes, &vec![b' '; MAX_SIGNATURE_BYTES as usize + 1]),
            Err(Error::MetadataTooLarge)
        ));
        for hash in ["", "😀", &"x".repeat(64)] {
            let mut value = manifest("https://example.invalid/native.tar");
            value["artifacts"][Target::LinuxX64.as_str()]["sha256"] = json!(hash);
            let (key, bytes, signature) = signed(&value);
            assert!(matches!(
                super::UpdateClient::new(
                    "https://example.invalid/native.json",
                    &key,
                    Renderer::Wgpu,
                    Target::LinuxX64,
                    "2026.9.99"
                )
                .unwrap()
                .authenticate(&bytes, &signature),
                Err(Error::Manifest)
            ));
        }
    }

    pub(super) fn serve(
        listener: TcpListener,
        responses: Vec<(u16, Vec<u8>)>,
    ) -> thread::JoinHandle<Vec<String>> {
        thread::spawn(move || {
            let mut paths = Vec::new();
            for (status, body) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(&mut stream);
                let mut first = String::new();
                reader.read_line(&mut first).unwrap();
                paths.push(first.split_whitespace().nth(1).unwrap().to_owned());
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" || line.is_empty() {
                        break;
                    }
                }
                write!(
                    stream,
                    "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                // Size/header rejection can close the client before body delivery.
                // Successful downloads still assert every received byte below.
                if let Err(error) = stream.write_all(&body) {
                    assert!(matches!(
                        error.kind(),
                        std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset
                    ));
                }
            }
            paths
        })
    }

    #[test]
    fn real_http_check_and_download_use_the_signed_target_not_server_order() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let mut value = manifest(&format!("{base}/artifact"));
        value["artifacts"]["x86_64-pc-windows-msvc"] =
            json!({"url": format!("{base}/wrong"), "size": 99, "sha256": "f".repeat(64)});
        let (key, bytes, signature) = signed(&value);
        let server = serve(
            listener,
            vec![(200, bytes), (200, signature), (200, PAYLOAD.to_vec())],
        );
        let client = UpdateClient::new(
            &format!("{base}/native.json"),
            &key,
            Renderer::Wgpu,
            Target::LinuxX64,
            "2026.9.99",
        )
        .unwrap();
        let cancel = CancelToken::default();
        let directory = tempfile::tempdir().unwrap();
        let verified = client
            .check(&cancel)
            .unwrap()
            .unwrap()
            .download(directory.path(), &cancel, |_, _| {})
            .unwrap();
        let mut actual = Vec::new();
        verified.file().read_to_end(&mut actual).unwrap();
        assert_eq!(actual, PAYLOAD);
        assert_eq!(
            server.join().unwrap(),
            ["/native.json", "/native.json.minisig", "/artifact"]
        );
    }

    #[test]
    fn failed_http_artifacts_leave_no_file_and_retry_the_same_authenticated_release() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (key, bytes, signature) = signed(&manifest(&format!("{base}/artifact")));
        let mut changed = PAYLOAD.to_vec();
        changed[3] ^= 1;
        let server = serve(
            listener,
            vec![
                (200, bytes),
                (200, signature),
                (200, changed),
                (200, PAYLOAD[..PAYLOAD.len() - 1].to_vec()),
                (200, PAYLOAD.to_vec()),
            ],
        );
        let update = UpdateClient::new(
            &format!("{base}/native.json"),
            &key,
            Renderer::Wgpu,
            Target::LinuxX64,
            "2026.9.99",
        )
        .unwrap()
        .check(&CancelToken::default())
        .unwrap()
        .unwrap();
        let directory = tempfile::tempdir().unwrap();
        assert!(matches!(
            update.download(directory.path(), &CancelToken::default(), |_, _| {}),
            Err(Error::Hash)
        ));
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
        assert!(matches!(
            update.download(directory.path(), &CancelToken::default(), |_, _| {}),
            Err(Error::Size)
        ));
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
        let verified = update
            .download(directory.path(), &CancelToken::default(), |_, _| {})
            .unwrap();
        let mut actual = Vec::new();
        verified.file().read_to_end(&mut actual).unwrap();
        assert_eq!(actual, PAYLOAD);
        drop(verified);
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
        assert_eq!(
            server.join().unwrap(),
            [
                "/native.json",
                "/native.json.minisig",
                "/artifact",
                "/artifact",
                "/artifact"
            ]
        );
    }

    #[test]
    fn http_errors_and_oversized_metadata_are_not_up_to_date() {
        for (status, body, oversized) in [
            (404, vec![], false),
            (503, vec![], false),
            (200, vec![b' '; MAX_MANIFEST_BYTES as usize + 1], true),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let endpoint = format!("http://{}/native.json", listener.local_addr().unwrap());
            let (key, _, _) = signed(&manifest("https://example.invalid/artifact"));
            let server = serve(listener, vec![(status, body)]);
            let result = UpdateClient::new(
                &endpoint,
                &key,
                Renderer::Wgpu,
                Target::LinuxX64,
                "2026.9.99",
            )
            .unwrap()
            .check(&CancelToken::default());
            assert!(if oversized {
                matches!(result, Err(Error::MetadataTooLarge))
            } else {
                matches!(result, Err(Error::Http(code)) if code == status)
            });
            assert_eq!(server.join().unwrap(), ["/native.json"]);
        }
    }
}
