//! Explicit, worker-owned native sharing prerequisite. No upload happens on
//! construction or reopen. The host resolves a stable local artifact and owns
//! presentation; this module owns remote state and account-scoped association.
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use reqwest::{
    blocking::{Body, Client, RequestBuilder, Response},
    header::{AUTHORIZATION, ETAG},
    redirect::Policy,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};

use crate::{AccountClient, Error as AccountError, Session, Vault};

const MAX_REPLY: u64 = 2 * 1024 * 1024;
const MAX_FILE: u64 = 5 * 1024 * 1024 * 1024 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Account(AccountError),
    Cancelled,
    MissingFile,
    ChangedFile,
    InvalidInput,
    Offline,
    Unavailable,
    NotFound,
    Protocol,
    Storage,
    /// Create may have committed without a response. Explicit upload retry uses
    /// its durable key; legacy unkeyed records require manual reconciliation.
    CreateUncertain,
    /// Create returned an ID, but local persistence failed. Retry the keyed
    /// upload or use `recover_created` after fixing local storage.
    StoreAfterCreate {
        asset_id: String,
        part_size: u64,
        part_count: u32,
    },
}

impl From<AccountError> for Error {
    fn from(value: AccountError) -> Self {
        Self::Account(value)
    }
}

/// A field omitted from an update retains its remote value. `Clear` sends JSON
/// null; `Set` sends a value. Never derive Debug for password-bearing patches.
pub enum Patch<T> {
    Keep,
    Clear,
    Set(T),
}
pub struct SharePatch {
    pub password: Patch<String>,
    /// RFC3339 timestamp in the future. The API validates exact expiry.
    pub expires_at: Patch<String>,
}
impl Default for SharePatch {
    fn default() -> Self {
        Self {
            password: Patch::Keep,
            expires_at: Patch::Keep,
        }
    }
}

#[derive(Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ShareInfo {
    pub id: String,
    pub password_protected: bool,
    pub expires_at: Option<String>,
    pub shared_at: String,
}

#[derive(Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AssetInfo {
    pub id: String,
    pub name: String,
    pub content_type: String,
    pub byte_size: u64,
    pub created_at: String,
    pub deleted_at: Option<String>,
    pub share: Option<ShareInfo>,
}

#[derive(Deserialize)]
struct Created {
    id: String,
    #[serde(rename = "partSize")]
    part_size: u64,
    #[serde(rename = "partCount")]
    part_count: u32,
}
#[derive(Deserialize)]
struct SignedPart {
    url: String,
    headers: BTreeMap<String, String>,
}
#[derive(Deserialize)]
struct Shared {
    share: Option<ShareInfo>,
}
#[derive(Deserialize)]
struct Listed {
    assets: Vec<AssetInfo>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Record {
    name: String,
    content_type: String,
    byte_size: u64,
    sha256: String,
    /// Missing only on legacy, non-idempotent creates, which cannot be retried.
    #[serde(default)]
    create_key: Option<String>,
    phase: Phase,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum Phase {
    Creating,
    Uploading {
        id: String,
        part_size: u64,
        etags: Vec<Option<String>>,
    },
    Ready {
        id: String,
    },
}
#[derive(Default, Serialize, Deserialize)]
struct Associations {
    version: u8,
    accounts: BTreeMap<String, BTreeMap<String, Record>>,
}

/// A file in the caller's disposable/native profile root, never installed
/// Tauri preferences. Reads and writes are explicit, atomic per replacement.
/// Serialize operations for one profile; separate processes need host election.
pub struct AssociationStore {
    path: PathBuf,
}
impl AssociationStore {
    pub fn new(profile_root: impl AsRef<Path>) -> Self {
        Self {
            path: profile_root.as_ref().join("native-share-associations.json"),
        }
    }
    fn read(&self) -> Result<Associations, Error> {
        match fs::read(&self.path) {
            Ok(bytes) => {
                let value: Associations =
                    serde_json::from_slice(&bytes).map_err(|_| Error::Storage)?;
                if value.version != 1 {
                    return Err(Error::Storage);
                }
                Ok(value)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Associations {
                version: 1,
                ..Associations::default()
            }),
            Err(_) => Err(Error::Storage),
        }
    }
    fn get(&self, user: &str, artifact: &str) -> Result<Option<Record>, Error> {
        Ok(self
            .read()?
            .accounts
            .get(user)
            .and_then(|a| a.get(artifact))
            .cloned())
    }
    fn put(&self, user: &str, artifact: &str, record: Record) -> Result<(), Error> {
        let mut all = self.read()?;
        all.accounts
            .entry(user.to_owned())
            .or_default()
            .insert(artifact.to_owned(), record);
        let parent = self.path.parent().ok_or(Error::Storage)?;
        fs::create_dir_all(parent).map_err(|_| Error::Storage)?;
        let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|_| Error::Storage)?;
        serde_json::to_writer(&mut temp, &all).map_err(|_| Error::Storage)?;
        temp.flush().map_err(|_| Error::Storage)?;
        temp.as_file().sync_all().map_err(|_| Error::Storage)?;
        temp.persist(&self.path).map_err(|_| Error::Storage)?;
        #[cfg(unix)]
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| Error::Storage)?;
        Ok(())
    }
}

pub enum Opened {
    Unassociated,
    Pending,
    Asset(Box<AssetInfo>),
}

/// `progress` reports bytes read for the current attempt plus acknowledged
/// parts, not server-confirmed throughput. It may retreat on a part retry.
pub type Progress = Arc<dyn Fn(u64, u64) + Send + Sync>;

struct PartSpec<'a> {
    id: &'a str,
    number: u32,
    start: u64,
    length: u64,
    committed: u64,
    total: u64,
}

pub struct SharingCoordinator<'a, V: Vault> {
    account: &'a mut AccountClient<V>,
    store: AssociationStore,
    object_http: Client,
}
impl<'a, V: Vault> SharingCoordinator<'a, V> {
    pub fn new(account: &'a mut AccountClient<V>, store: AssociationStore) -> Result<Self, Error> {
        let object_http = Client::builder()
            .timeout(Duration::from_secs(120))
            .redirect(Policy::none())
            .build()
            .map_err(|_| Error::Protocol)?;
        Ok(Self {
            account,
            store,
            object_http,
        })
    }

    /// Fetch the account's remote asset. Never creates or uploads on reopen.
    pub fn open(&mut self, artifact: &str) -> Result<Opened, Error> {
        check_id(artifact)?;
        let user = self.account.me()?.id;
        let Some(record) = self.store.get(&user, artifact)? else {
            return Ok(Opened::Unassociated);
        };
        let Phase::Ready { id } = record.phase else {
            return Ok(Opened::Pending);
        };
        let active: Listed = self.api(
            self.account
                .http
                .get(self.account.url("api/assets"))
                .header(AUTHORIZATION, self.bearer()?),
            200,
        )?;
        if let Some(asset) = active.assets.into_iter().find(|a| a.id == id) {
            return Ok(Opened::Asset(Box::new(asset)));
        }
        let trashed: Listed = self.api(
            self.account
                .http
                .get(self.account.url("api/assets?deleted=true"))
                .header(AUTHORIZATION, self.bearer()?),
            200,
        )?;
        trashed
            .assets
            .into_iter()
            .find(|a| a.id == id)
            .map(|asset| Opened::Asset(Box::new(asset)))
            .ok_or(Error::NotFound)
    }

    /// Explicit upload. Retry keyed creates after an ambiguous outcome using the
    /// persisted key; acknowledged uploads resume from saved ETags on restart.
    pub fn upload(
        &mut self,
        artifact: &str,
        file: &Path,
        name: &str,
        content_type: &str,
        cancelled: Arc<AtomicBool>,
        progress: Progress,
    ) -> Result<AssetInfo, Error> {
        check_id(artifact)?;
        if name.trim().is_empty()
            || name.len() > 1024
            || content_type.is_empty()
            || content_type.len() > 255
        {
            return Err(Error::InvalidInput);
        }
        let user = self.account.me()?.id;
        let (size, hash) = fingerprint(file, &cancelled)?;
        let mut record = if let Some(record) = self.store.get(&user, artifact)? {
            if record.byte_size != size
                || record.sha256 != hash
                || record.name != name
                || record.content_type != content_type
            {
                return Err(Error::ChangedFile);
            }
            record
        } else {
            let record = Record {
                name: name.to_owned(),
                content_type: content_type.to_owned(),
                byte_size: size,
                sha256: hash,
                create_key: Some(uuid::Uuid::new_v4().to_string()),
                phase: Phase::Creating,
            };
            if cancelled.load(Ordering::Relaxed) {
                return Err(Error::Cancelled);
            }
            self.store.put(&user, artifact, record.clone())?;
            record
        };
        if matches!(record.phase, Phase::Creating) {
            let key = record.create_key.as_ref().ok_or(Error::CreateUncertain)?;
            if cancelled.load(Ordering::Relaxed) {
                return Err(Error::Cancelled);
            }
            let response = self
                .account
                .http
                .put(self.account.url(&format!("api/asset-uploads/{key}")))
                .header(AUTHORIZATION, self.bearer()?)
                .json(&serde_json::json!({"name":name,"contentType":content_type,"byteSize":size}))
                .send();
            let created: Created = match response {
                Ok(response) => match self.decode_api(response, 201) {
                    Ok(created) => created,
                    Err(Error::Account(error)) => return Err(Error::Account(error)),
                    Err(_) => return Err(Error::CreateUncertain),
                },
                Err(_) => return Err(Error::CreateUncertain),
            };
            check_id(&created.id).map_err(|_| Error::CreateUncertain)?;
            validate_parts(size, created.part_size, created.part_count)?;
            record.phase = Phase::Uploading {
                id: created.id.clone(),
                part_size: created.part_size,
                etags: vec![None; created.part_count as usize],
            };
            self.store
                .put(&user, artifact, record.clone())
                .map_err(|_| Error::StoreAfterCreate {
                    asset_id: created.id,
                    part_size: created.part_size,
                    part_count: created.part_count,
                })?;
        }
        self.transfer(&user, artifact, file, &cancelled, &progress, &mut record)
    }

    /// Only for an ID returned in `StoreAfterCreate` after local storage is
    /// repaired. Never guesses an ID from a failed/ambiguous create request.
    pub fn recover_created(
        &mut self,
        artifact: &str,
        id: &str,
        part_size: u64,
        part_count: u32,
    ) -> Result<(), Error> {
        check_id(artifact)?;
        check_id(id)?;
        let user = self.account.me()?.id;
        let mut record = self.store.get(&user, artifact)?.ok_or(Error::NotFound)?;
        if !matches!(record.phase, Phase::Creating) {
            return Err(Error::InvalidInput);
        }
        validate_parts(record.byte_size, part_size, part_count)?;
        record.phase = Phase::Uploading {
            id: id.to_owned(),
            part_size,
            etags: vec![None; part_count as usize],
        };
        self.store.put(&user, artifact, record)
    }

    fn transfer(
        &mut self,
        user: &str,
        artifact: &str,
        file: &Path,
        cancelled: &Arc<AtomicBool>,
        progress: &Progress,
        record: &mut Record,
    ) -> Result<AssetInfo, Error> {
        let Phase::Uploading {
            id,
            part_size,
            etags,
        } = &record.phase
        else {
            return Err(Error::InvalidInput);
        };
        let (id, part_size, count) = (id.clone(), *part_size, etags.len());
        for index in 0..count {
            let Phase::Uploading { etags, .. } = &record.phase else {
                unreachable!()
            };
            if etags[index].is_some() {
                continue;
            }
            if cancelled.load(Ordering::Relaxed) {
                return Err(Error::Cancelled);
            }
            let start = (index as u64)
                .checked_mul(part_size)
                .ok_or(Error::Protocol)?;
            let length = part_size.min(record.byte_size.saturating_sub(start));
            let committed: u64 = etags
                .iter()
                .enumerate()
                .filter(|(_, e)| e.is_some())
                .map(|(i, _)| part_size.min(record.byte_size.saturating_sub(i as u64 * part_size)))
                .sum();
            let etag = self.put_part(
                file,
                PartSpec {
                    id: &id,
                    number: index as u32 + 1,
                    start,
                    length,
                    committed,
                    total: record.byte_size,
                },
                cancelled,
                progress,
            )?;
            let Phase::Uploading { etags, .. } = &mut record.phase else {
                unreachable!()
            };
            etags[index] = Some(etag);
            self.store.put(user, artifact, record.clone())?;
            progress(committed + length, record.byte_size);
        }
        if cancelled.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let (size, hash) = fingerprint(file, cancelled)?;
        if size != record.byte_size || hash != record.sha256 {
            return Err(Error::ChangedFile);
        }
        let Phase::Uploading { etags, .. } = &record.phase else {
            unreachable!()
        };
        let parts: Vec<_> = etags.iter().enumerate().map(|(i, etag)| serde_json::json!({"partNumber":i+1,"etag":etag.as_deref().expect("all parts uploaded")})).collect();
        let request = self
            .account
            .http
            .post(self.account.url(&format!("api/assets/{id}/complete")))
            .header(AUTHORIZATION, self.bearer()?)
            .json(&serde_json::json!({"parts":parts}));
        let asset: AssetInfo = self.api(request, 200)?;
        if asset.id != id || asset.byte_size != record.byte_size {
            return Err(Error::Protocol);
        }
        record.phase = Phase::Ready { id };
        self.store.put(user, artifact, record.clone())?;
        Ok(asset)
    }

    fn put_part(
        &mut self,
        file: &Path,
        spec: PartSpec<'_>,
        cancelled: &Arc<AtomicBool>,
        progress: &Progress,
    ) -> Result<String, Error> {
        for attempt in 0..2 {
            if cancelled.load(Ordering::Relaxed) {
                return Err(Error::Cancelled);
            }
            let request = self
                .account
                .http
                .post(self.account.url(&format!("api/assets/{}/parts", spec.id)))
                .header(AUTHORIZATION, self.bearer()?)
                .json(&serde_json::json!({"partNumber":spec.number}));
            let signed: SignedPart = self.api(request, 200)?;
            let url = reqwest::Url::parse(&signed.url).map_err(|_| Error::Protocol)?;
            let loopback = url.host_str().is_some_and(|host| {
                host == "localhost"
                    || host
                        .trim_matches(['[', ']'])
                        .parse::<std::net::IpAddr>()
                        .is_ok_and(|ip| ip.is_loopback())
            });
            if (url.scheme() != "https" && !(url.scheme() == "http" && loopback))
                || !url.username().is_empty()
                || url.password().is_some()
                || url.fragment().is_some()
            {
                return Err(Error::Protocol);
            }
            let mut request = self.object_http.put(url);
            for (name, value) in signed.headers {
                if ["authorization", "cookie", "proxy-authorization", "host"]
                    .contains(&name.to_ascii_lowercase().as_str())
                {
                    return Err(Error::Protocol);
                }
                request = request.header(name, value);
            }
            let mut source = File::open(file).map_err(|_| Error::MissingFile)?;
            source
                .seek(SeekFrom::Start(spec.start))
                .map_err(|_| Error::MissingFile)?;
            let reader = PartReader {
                file: source,
                remaining: spec.length,
                sent: 0,
                committed: spec.committed,
                total: spec.total,
                cancelled: Arc::clone(cancelled),
                progress: Arc::clone(progress),
            };
            let response = request.body(Body::sized(reader, spec.length)).send();
            if cancelled.load(Ordering::Relaxed) {
                return Err(Error::Cancelled);
            }
            match response {
                Ok(response) if response.status().is_success() => {
                    let etag = response
                        .headers()
                        .get(ETAG)
                        .and_then(|v| v.to_str().ok())
                        .filter(|s| !s.is_empty() && s.len() <= 512)
                        .ok_or(Error::Protocol)?;
                    return Ok(etag.to_owned());
                }
                Ok(response) if attempt == 0 && response.status().as_u16() == 403 => continue,
                Err(_) if attempt == 0 => continue, // Same multipart part number and bytes: safe to retry.
                Err(_) => return Err(Error::Offline),
                Ok(response) if response.status().as_u16() == 503 => {
                    return Err(Error::Unavailable);
                }
                Ok(_) => return Err(Error::Protocol),
            }
        }
        Err(Error::Protocol)
    }

    /// Called separately after completion. A failed/ambiguous PUT retains the
    /// ready association and can be retried without uploading the original.
    pub fn configure_share(
        &mut self,
        artifact: &str,
        enabled: bool,
        patch: SharePatch,
    ) -> Result<Option<ShareInfo>, Error> {
        let id = self.ready_id(artifact)?;
        let mut body = serde_json::json!({"enabled":enabled});
        if enabled {
            field(&mut body, "password", patch.password);
            field(&mut body, "expiresAt", patch.expires_at);
        }
        let request = self
            .account
            .http
            .put(self.account.url(&format!("api/assets/{id}/share")))
            .header(AUTHORIZATION, self.bearer()?)
            .json(&body);
        let result: Shared = self.api(request, 200)?;
        if result
            .share
            .as_ref()
            .is_some_and(|share| check_id(&share.id).is_err())
            || (enabled && result.share.is_none())
            || (!enabled && result.share.is_some())
        {
            return Err(Error::Protocol);
        }
        Ok(result.share)
    }

    pub fn trash(&mut self, artifact: &str) -> Result<(), Error> {
        let id = self.ready_id(artifact)?;
        let request = self
            .account
            .http
            .delete(self.account.url(&format!("api/assets/{id}")))
            .header(AUTHORIZATION, self.bearer()?);
        self.api::<serde_json::Value>(request, 204).map(|_| ())
    }

    pub fn restore(&mut self, artifact: &str) -> Result<AssetInfo, Error> {
        let id = self.ready_id(artifact)?;
        let request = self
            .account
            .http
            .post(self.account.url(&format!("api/assets/{id}/restore")))
            .header(AUTHORIZATION, self.bearer()?);
        let asset: AssetInfo = self.api(request, 200)?;
        if asset.id != id || asset.deleted_at.is_some() || asset.share.is_some() {
            return Err(Error::Protocol);
        }
        Ok(asset)
    }

    fn ready_id(&mut self, artifact: &str) -> Result<String, Error> {
        check_id(artifact)?;
        let user = self.account.me()?.id;
        let record = self.store.get(&user, artifact)?.ok_or(Error::NotFound)?;
        match record.phase {
            Phase::Ready { id } => Ok(id),
            _ => Err(Error::InvalidInput),
        }
    }
    fn bearer(&self) -> Result<String, Error> {
        let Session::Active { token, .. } = &self.account.session else {
            return Err(Error::Account(AccountError::InvalidSession));
        };
        Ok(format!("Bearer {token}"))
    }
    fn api<T: DeserializeOwned>(
        &mut self,
        request: RequestBuilder,
        expected: u16,
    ) -> Result<T, Error> {
        let response = request.send().map_err(|_| Error::Offline)?;
        self.decode_api(response, expected)
    }
    fn decode_api<T: DeserializeOwned>(
        &mut self,
        response: Response,
        expected: u16,
    ) -> Result<T, Error> {
        let status = response.status().as_u16();
        if status == 401 {
            self.account.session = Session::Invalid;
            self.account.clear_invalid()?;
            return Err(Error::Account(AccountError::InvalidSession));
        }
        let mut body = Vec::new();
        response
            .take(MAX_REPLY + 1)
            .read_to_end(&mut body)
            .map_err(|_| Error::Protocol)?;
        if body.len() as u64 > MAX_REPLY {
            return Err(Error::Protocol);
        }
        if status == 503 {
            return Err(Error::Unavailable);
        }
        if status == 404 {
            return Err(Error::NotFound);
        }
        if status != expected {
            return Err(Error::Protocol);
        }
        if expected == 204 {
            return serde_json::from_str("null").map_err(|_| Error::Protocol);
        }
        serde_json::from_slice(&body).map_err(|_| Error::Protocol)
    }
}

fn field(body: &mut serde_json::Value, name: &str, patch: Patch<String>) {
    match patch {
        Patch::Keep => (),
        Patch::Clear => body[name] = serde_json::Value::Null,
        Patch::Set(value) => body[name] = serde_json::Value::String(value),
    }
}
fn check_id(id: &str) -> Result<(), Error> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        Err(Error::InvalidInput)
    } else {
        Ok(())
    }
}
fn validate_parts(size: u64, part_size: u64, count: u32) -> Result<(), Error> {
    if part_size == 0
        || count == 0
        || count > 10_000
        || (size.max(1) - 1) / part_size + 1 != u64::from(count)
    {
        Err(Error::Protocol)
    } else {
        Ok(())
    }
}
fn fingerprint(path: &Path, cancelled: &AtomicBool) -> Result<(u64, String), Error> {
    let mut file = File::open(path).map_err(|_| Error::MissingFile)?;
    let size = file.metadata().map_err(|_| Error::MissingFile)?.len();
    if size > MAX_FILE {
        return Err(Error::InvalidInput);
    }
    let mut hash = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    let mut read = 0_u64;
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let count = file.read(&mut buffer).map_err(|_| Error::MissingFile)?;
        if count == 0 {
            break;
        }
        read += count as u64;
        hash.update(&buffer[..count]);
    }
    if read != size {
        return Err(Error::ChangedFile);
    }
    Ok((size, format!("{:x}", hash.finalize())))
}
struct PartReader {
    file: File,
    remaining: u64,
    sent: u64,
    committed: u64,
    total: u64,
    cancelled: Arc<AtomicBool>,
    progress: Progress,
}
impl Read for PartReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if self.cancelled.load(Ordering::Relaxed) {
            return Err(io::ErrorKind::Interrupted.into());
        }
        if self.remaining == 0 {
            return Ok(0);
        }
        let limit = output.len().min(64 * 1024).min(self.remaining as usize);
        let count = self.file.read(&mut output[..limit])?;
        if count == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        self.remaining -= count as u64;
        self.sent += count as u64;
        (self.progress)(self.committed + self.sent, self.total);
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn part_reader_bounds_each_read_and_stops_on_cancellation() {
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(&vec![0x5a; 200_000]).unwrap();
        file.rewind().unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut reader = PartReader {
            file,
            remaining: 200_000,
            sent: 0,
            committed: 0,
            total: 200_000,
            cancelled: Arc::clone(&cancelled),
            progress: Arc::new(|_, _| {}),
        };
        let mut buffer = vec![0; 200_000];
        assert_eq!(reader.read(&mut buffer).unwrap(), 64 * 1024);
        assert!(buffer[..64 * 1024].iter().all(|byte| *byte == 0x5a));
        cancelled.store(true, Ordering::Relaxed);
        assert_eq!(
            reader.read(&mut buffer).unwrap_err().kind(),
            io::ErrorKind::Interrupted
        );
    }
}
