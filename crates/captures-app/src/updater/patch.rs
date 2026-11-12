//! Per-file native update patches: rebuild the new package from the installed one.
//!
//! A patch turns one published native build into the newest one for one target.
//! It lists every directory and file of the new package as its full archive
//! stages (relative to the single package folder), with each file's size,
//! SHA-256 and execute bit. Each file is
//! - copied from an installed file whose bytes did not change (any path),
//! - rebuilt with a bzip2 BSDIFF40 patch of the installed file at a path, or
//! - carried whole, bzip2-compressed.
//!
//! The signed manifest pins the patch's own size and SHA-256, so its contents
//! are as trusted as the full archive. Clients hash every installed file they
//! read, rebuild into the private stage under the same path/entry/size rules as
//! full extraction, then reread the stage and require exactly the listed entries
//! and bytes before normal package validation. Any failure except cancellation
//! falls back to the full archive in the same attempt. Native packages hold no
//! links, so neither patches nor installed bases may use them.
//!
//! Layout: `CAPTURESPATCH1\n`, the JSON header length (u32, little endian), the
//! JSON [`Header`], then the data that file sources point into.
//!
//! Manifest (schema 2), per artifact:
//! `"delta": {"format": "capturespatch1", "patches": [{"base_version", "url",
//! "size", "sha256"}]}`. Clients that predate per-file patches cannot parse this
//! descriptor and use the full artifact. Patches not smaller than half the full
//! artifact are ignored, as are descriptors listing more than [`MAX_OFFERS`].
use super::staging::{self, Extraction};
use super::*;
use bzip2::read::BzDecoder;
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File},
    io::{self, BufWriter},
};

const MAGIC: &[u8; 15] = b"CAPTURESPATCH1\n";
const SCHEMA: u32 = 1;
/// The manifest `delta.format` for per-file patches.
pub const FORMAT: &str = "capturespatch1";
const MAX_HEADER_BYTES: u32 = 4 * 1024 * 1024;
/// Patches per artifact a client considers. Release tooling keeps six; each
/// offer adds about 250 bytes to the signed manifest.
pub const MAX_OFFERS: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    schema: u32,
    target: Target,
    base_version: String,
    version: String,
    /// The archive's single package folder.
    root: String,
    entries: Vec<Entry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
enum Entry {
    Dir {
        path: String,
    },
    File {
        path: String,
        executable: bool,
        size: u64,
        sha256: String,
        source: FileSource,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
enum FileSource {
    /// The installed file at `base` already has this entry's bytes.
    Copy { base: String },
    /// A BSDIFF40 patch from the installed file at `base`.
    Bsdiff {
        base: String,
        base_size: u64,
        base_sha256: String,
        offset: u64,
        length: u64,
    },
    /// The whole file, bzip2-compressed.
    Whole { offset: u64, length: u64 },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Descriptor {
    format: String,
    patches: Vec<Offer>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Offer {
    base_version: String,
    url: String,
    size: u64,
    sha256: String,
}

/// Whether a patch is worth listing: strictly smaller than half the full artifact.
pub fn worthwhile(patch_size: u64, full_size: u64) -> bool {
    patch_size > 0 && patch_size.saturating_mul(2) < full_size
}

/// Where a patch reads unchanged and base files from.
#[derive(Clone, Debug)]
pub(super) enum Base {
    /// The installed package root (the default).
    Installed(PathBuf),
    /// A retained full archive, unpacked privately (diagnostic override).
    Archive(PathBuf),
}

/// The signed patch from the running version, if the manifest lists one.
pub(super) struct Candidate {
    url: Url,
    size: u64,
    hash: [u8; 32],
    base_version: Version,
}

impl Candidate {
    pub(super) fn select(
        value: &serde_json::Value,
        current: &Version,
        full_size: u64,
        loopback: bool,
    ) -> Option<Self> {
        let descriptor: Descriptor = serde_json::from_value(value.clone()).ok()?;
        if descriptor.format != FORMAT || descriptor.patches.len() > MAX_OFFERS {
            return None;
        }
        descriptor.patches.iter().find_map(|offer| {
            let base_version = Version::parse(&offer.base_version).ok()?;
            if base_version != *current || !worthwhile(offer.size, full_size) {
                return None;
            }
            let url = Url::parse(&offer.url).ok()?;
            validate_url(&url, loopback).ok()?;
            Some(Self {
                url,
                size: offer.size,
                hash: decode_hash(&offer.sha256).ok()?,
                base_version,
            })
        })
    }

    /// Download the signed patch and rebuild the package in a private stage.
    pub(super) fn stage(
        &self,
        update: &PendingUpdate,
        base: &Base,
        directory: &Path,
        cancel: &CancelToken,
        progress: &mut impl FnMut(u64, u64),
    ) -> Result<StagedUpdate, Error> {
        check_cancel(cancel)?;
        // No request when there is plainly nothing to patch from.
        let present = match base {
            Base::Installed(path) => path.symlink_metadata()?.is_dir(),
            Base::Archive(path) => path.symlink_metadata()?.is_file(),
        };
        if !present {
            return Err(Error::Package);
        }
        progress(0, self.size);
        check_cancel(cancel)?;
        let response = successful(
            update
                .client
                .get(self.url.clone())
                .send()
                .map_err(|_| Error::Network)?,
        )?;
        if response.content_length().is_some_and(|n| n != self.size) {
            return Err(Error::Size);
        }
        let mut info = update.info.clone();
        info.size = self.size;
        let patch = verify_file(
            response,
            directory,
            &info,
            self.hash,
            cancel,
            &mut *progress,
        )?;
        Retained {
            patch,
            release: update.info.clone(),
            release_hash: update.hash,
            base: base.clone(),
            base_version: self.base_version.clone(),
        }
        .stage(directory, cancel)
    }
}

/// A verified patch kept by its stage, so replacement rebuilds a fresh private
/// copy from the still-installed base instead of trusting exposed staged paths.
pub(super) struct Retained {
    patch: VerifiedUpdate,
    release: ReleaseInfo,
    release_hash: [u8; 32],
    base: Base,
    base_version: Version,
}

impl Retained {
    pub(super) fn info(&self) -> &ReleaseInfo {
        &self.release
    }

    pub(super) fn release_hash(&self) -> &[u8; 32] {
        &self.release_hash
    }

    /// Rehash a private copy of the patch, rebuild, reread and validate.
    /// Failure or cancellation removes everything this created.
    pub(super) fn stage(
        mut self,
        directory: &Path,
        cancel: &CancelToken,
    ) -> Result<StagedUpdate, Error> {
        check_cancel(cancel)?;
        let stage = tempfile::Builder::new()
            .prefix("captures-native-update-")
            .tempdir_in(directory)?;
        self.patch.file.seek(SeekFrom::Start(0))?;
        let mut copy = verify_file(
            self.patch.file.as_file_mut(),
            stage.path(),
            &self.patch.info,
            self.patch.hash,
            cancel,
            |_, _| {},
        )?;
        let target = self.release.target;
        let base = match &self.base {
            Base::Installed(path) => path.clone(),
            Base::Archive(path) => {
                if !path.symlink_metadata()?.is_file() {
                    return Err(Error::Archive);
                }
                let root = stage.path().join("base");
                staging::unpack(target, &mut File::open(path)?, root, cancel)?.0
            }
        };
        let expected = Expected {
            target,
            base_version: &self.base_version,
            version: &self.release.version,
        };
        let (package, executables) = rebuild(
            copy.file.as_file_mut(),
            &expected,
            &base,
            stage.path().join("package"),
            cancel,
        )?;
        if matches!(self.base, Base::Archive(_)) {
            fs::remove_dir_all(stage.path().join("base"))?;
        }
        let executable = staging::validate_package(&package, target, &executables, cancel)?;
        check_cancel(cancel)?;
        Ok(StagedUpdate::new(
            staging::Source::Patch(Self {
                patch: copy,
                release: self.release,
                release_hash: self.release_hash,
                base: self.base,
                base_version: self.base_version,
            }),
            stage,
            package,
            executable,
        ))
    }
}

/// What the signed manifest says the patch must turn into.
struct Expected<'a> {
    target: Target,
    base_version: &'a Version,
    version: &'a str,
}

/// A directory or file as staged, without where its bytes come from.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Node {
    Dir,
    File {
        size: u64,
        sha256: [u8; 32],
        executable: bool,
    },
}

/// Rebuild the package described by `patch` from `base` under `root` (which
/// must not exist) and return the package folder and its executable files.
fn rebuild(
    patch: &mut File,
    expected: &Expected,
    base: &Path,
    root: PathBuf,
    cancel: &CancelToken,
) -> Result<(PathBuf, HashSet<PathBuf>), Error> {
    let (header, data_start) = read_header(patch)?;
    let release = Version::parse(expected.version).map_err(|_| Error::Version)?;
    if header.target != expected.target
        || Version::parse(&header.base_version).ok().as_ref() != Some(expected.base_version)
        || Version::parse(&header.version).ok() != Some(release)
    {
        return Err(Error::Manifest);
    }
    let data_length = patch.metadata()?.len() - data_start;
    // Validate everything before writing: the same path, entry-count and size
    // rules as full extraction, plus base paths and data ranges.
    if header.root.contains('/') {
        return Err(Error::ArchivePath);
    }
    let mut extraction = Extraction::new(root);
    let package = extraction.reserve(&header.root, true, 0)?;
    let mut targets = Vec::with_capacity(header.entries.len());
    let mut wanted = BTreeMap::new();
    for entry in &header.entries {
        check_cancel(cancel)?;
        let (path, node) = match entry {
            Entry::Dir { path } => (path, Node::Dir),
            Entry::File {
                path,
                executable,
                size,
                sha256,
                source,
            } => {
                let range_ok = |offset: u64, length: u64| {
                    length <= staging::MAX_FILE_BYTES
                        && offset
                            .checked_add(length)
                            .is_some_and(|end| end <= data_length)
                };
                let source_ok = match source {
                    FileSource::Copy { base } => safe_path(base),
                    FileSource::Bsdiff {
                        base,
                        base_size,
                        base_sha256,
                        offset,
                        length,
                    } => {
                        safe_path(base)
                            && *base_size <= staging::MAX_FILE_BYTES
                            && decode_hash(base_sha256).is_ok()
                            && range_ok(*offset, *length)
                    }
                    FileSource::Whole { offset, length } => range_ok(*offset, *length),
                };
                if !source_ok {
                    return Err(Error::Archive);
                }
                let node = Node::File {
                    size: *size,
                    sha256: decode_hash(sha256)?,
                    executable: *executable,
                };
                (path, node)
            }
        };
        if path.ends_with('/') {
            return Err(Error::ArchivePath);
        }
        let directory = node == Node::Dir;
        let size = match node {
            Node::File { size, .. } => size,
            Node::Dir => 0,
        };
        targets.push(extraction.reserve(&format!("{}/{path}", header.root), directory, size)?);
        // Folders that paths only imply are created and must exist too.
        let mut parent = path.as_str();
        while let Some((folder, _)) = parent.rsplit_once('/') {
            wanted.entry(folder.to_owned()).or_insert(Node::Dir);
            parent = folder;
        }
        wanted.insert(path.clone(), node);
    }

    fs::create_dir_all(&package)?;
    let mut executables = HashSet::new();
    for (entry, target) in header.entries.iter().zip(&targets) {
        check_cancel(cancel)?;
        match entry {
            Entry::Dir { .. } => fs::create_dir_all(target)?,
            Entry::File {
                executable,
                size,
                sha256,
                source,
                ..
            } => {
                fs::create_dir_all(target.parent().ok_or(Error::ArchivePath)?)?;
                let mut file = File::options().write(true).create_new(true).open(target)?;
                let result = write_file(&mut file, source, *size, base, patch, data_start, cancel);
                check_cancel(cancel)?;
                let (written, digest) = result?;
                if written != *size || digest != decode_hash(sha256)? {
                    return Err(Error::Hash);
                }
                file.sync_all()?;
                set_executable(&file, *executable)?;
                if *executable {
                    executables.insert(target.clone());
                }
            }
        }
    }
    verify_tree(&package, &wanted, cancel)?;
    Ok((package, executables))
}

fn write_file(
    file: &mut File,
    source: &FileSource,
    size: u64,
    base: &Path,
    patch: &mut File,
    data_start: u64,
    cancel: &CancelToken,
) -> Result<(u64, [u8; 32]), Error> {
    let mut output = Output {
        inner: BufWriter::new(file),
        hasher: Sha256::new(),
        written: 0,
        remaining: size,
        cancel,
    };
    match source {
        FileSource::Copy { base: path } => {
            // The installed bytes are verified by the entry's own size and hash.
            let input = open_base(base, path)?;
            io::copy(&mut input.take(size + 1), &mut output)?;
        }
        FileSource::Bsdiff {
            base: path,
            base_size,
            base_sha256,
            offset,
            length,
        } => {
            let source = read_verified(
                open_base(base, path)?,
                *base_size,
                decode_hash(base_sha256)?,
                cancel,
            )?;
            let delta = read_data(patch, data_start, *offset, *length)?;
            validate_controls(&delta, *base_size, size, cancel)?;
            qbsdiff::Bspatch::new(&delta)
                .and_then(|patcher| patcher.apply(&source, &mut output))
                .map_err(|_| Error::Archive)?;
        }
        FileSource::Whole { offset, length } => {
            let compressed = read_data(patch, data_start, *offset, *length)?;
            io::copy(
                &mut BzDecoder::new(&compressed[..]).take(size + 1),
                &mut output,
            )
            .map_err(|_| Error::Archive)?;
        }
    }
    output.inner.flush()?;
    Ok((output.written, output.hasher.finalize().into()))
}

fn read_header(patch: &mut File) -> Result<(Header, u64), Error> {
    patch.seek(SeekFrom::Start(0))?;
    let mut magic = [0; MAGIC.len()];
    patch.read_exact(&mut magic).map_err(|_| Error::Archive)?;
    if &magic != MAGIC {
        return Err(Error::Archive);
    }
    let mut length = [0; 4];
    patch.read_exact(&mut length).map_err(|_| Error::Archive)?;
    let length = u32::from_le_bytes(length);
    if length == 0 || length > MAX_HEADER_BYTES {
        return Err(Error::Archive);
    }
    let mut json = vec![0; length as usize];
    patch.read_exact(&mut json).map_err(|_| Error::Archive)?;
    let header: Header = serde_json::from_slice(&json).map_err(|_| Error::Archive)?;
    if header.schema != SCHEMA {
        return Err(Error::Archive);
    }
    Ok((header, (MAGIC.len() + 4) as u64 + u64::from(length)))
}

fn read_data(patch: &mut File, start: u64, offset: u64, length: u64) -> Result<Vec<u8>, Error> {
    patch.seek(SeekFrom::Start(start + offset))?;
    let mut bytes = vec![0; length as usize];
    patch.read_exact(&mut bytes).map_err(|_| Error::Archive)?;
    Ok(bytes)
}

/// Relative, `/`-separated, with no empty, `.` or `..` parts and no characters
/// Windows reads as separators, drives or streams.
fn safe_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 1024
        && path.split('/').all(|part| {
            !part.is_empty() && part != "." && part != ".." && !part.contains(['\\', ':', '\0'])
        })
}

fn join(root: &Path, path: &str) -> PathBuf {
    path.split('/')
        .fold(root.to_owned(), |path, part| path.join(part))
}

/// An installed regular file; never a link the installed package may hold there.
fn open_base(root: &Path, path: &str) -> Result<File, Error> {
    let path = join(root, path);
    let metadata = path.symlink_metadata()?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(Error::Package);
    }
    Ok(File::open(path)?)
}

fn read_verified(
    reader: impl Read,
    size: u64,
    hash: [u8; 32],
    cancel: &CancelToken,
) -> Result<Vec<u8>, Error> {
    let mut reader = reader.take(size + 1);
    let mut bytes = Vec::new();
    let mut hasher = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        check_cancel(cancel)?;
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        if bytes.len() as u64 + count as u64 > size {
            return Err(Error::Size);
        }
        hasher.update(&buffer[..count]);
        bytes.extend_from_slice(&buffer[..count]);
    }
    if bytes.len() as u64 != size {
        return Err(Error::Size);
    }
    if <[u8; 32]>::from(hasher.finalize()) != hash {
        return Err(Error::Hash);
    }
    Ok(bytes)
}

fn hash_file(path: &Path, cancel: &CancelToken) -> Result<[u8; 32], Error> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        check_cancel(cancel)?;
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hasher.finalize().into())
}

/// A directory or file found on disk, relative to the package folder.
#[derive(Debug)]
struct Found {
    directory: bool,
    size: u64,
    path: PathBuf,
    #[cfg_attr(not(unix), allow(dead_code))]
    mode: u32,
}

/// Every entry beneath `root`, refusing links, other file types and non-UTF-8
/// names, bounded like extraction.
fn walk(root: &Path, cancel: &CancelToken) -> Result<BTreeMap<String, Found>, Error> {
    let mut found = BTreeMap::new();
    let mut pending = vec![(root.to_owned(), String::new())];
    while let Some((folder, prefix)) = pending.pop() {
        for item in fs::read_dir(&folder)? {
            check_cancel(cancel)?;
            let item = item?;
            let name = item
                .file_name()
                .into_string()
                .map_err(|_| Error::ArchivePath)?;
            let relative = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            let metadata = fs::symlink_metadata(item.path())?;
            if metadata.file_type().is_symlink() || !(metadata.is_file() || metadata.is_dir()) {
                return Err(Error::ArchivePath);
            }
            if found.len() >= staging::MAX_ENTRIES {
                return Err(Error::Archive);
            }
            #[cfg(unix)]
            let mode = {
                use std::os::unix::fs::PermissionsExt;
                metadata.permissions().mode() & 0o777
            };
            #[cfg(not(unix))]
            let mode = 0;
            if metadata.is_dir() {
                pending.push((item.path(), relative.clone()));
            }
            found.insert(
                relative,
                Found {
                    directory: metadata.is_dir(),
                    size: metadata.len(),
                    path: item.path(),
                    mode,
                },
            );
        }
    }
    Ok(found)
}

/// `package` must hold exactly `wanted`: no missing or extra entries, every file
/// reread and hashed, and staging's private file modes on Unix.
fn verify_tree(
    package: &Path,
    wanted: &BTreeMap<String, Node>,
    cancel: &CancelToken,
) -> Result<(), Error> {
    let found = walk(package, cancel)?;
    if found.len() != wanted.len() {
        return Err(Error::Package);
    }
    for (path, node) in wanted {
        let actual = found.get(path).ok_or(Error::Package)?;
        let matches = match node {
            Node::Dir => actual.directory,
            Node::File {
                size,
                sha256,
                executable,
            } => {
                !actual.directory
                    && actual.size == *size
                    && (!cfg!(unix) || actual.mode == if *executable { 0o700 } else { 0o600 })
                    && hash_file(&actual.path, cancel)? == *sha256
            }
        };
        if !matches {
            return Err(Error::Package);
        }
    }
    Ok(())
}

/// Same private modes full staging writes.
fn set_executable(file: &File, executable: bool) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(if executable {
            0o700
        } else {
            0o600
        }))
    }
    #[cfg(not(unix))]
    {
        let _ = (file, executable);
        Ok(())
    }
}

/// Bounded, cancellable, counting and hashing output for one rebuilt file.
struct Output<'a, W: Write> {
    inner: W,
    hasher: Sha256,
    written: u64,
    remaining: u64,
    cancel: &'a CancelToken,
}

impl<W: Write> Write for Output<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.cancel.is_cancelled() || bytes.len() as u64 > self.remaining {
            return Err(io::Error::other("cancelled or oversized patch output"));
        }
        let count = self.inner.write(bytes)?;
        self.hasher.update(&bytes[..count]);
        self.written += count as u64;
        self.remaining -= count as u64;
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.cancel.is_cancelled() {
            return Err(io::Error::other("patch cancelled"));
        }
        self.inner.flush()
    }
}

// qbsdiff's size is a hint, not a bound. Validate header lengths before calling
// it (negative lengths otherwise cast to u64), and bound control count and
// output before decoding. A leading zero-output source seek is legitimate.
fn validate_controls(
    patch: &[u8],
    base_size: u64,
    target_size: u64,
    cancel: &CancelToken,
) -> Result<(), Error> {
    if patch.len() < 32 || &patch[..8] != b"BSDIFF40" {
        return Err(Error::Archive);
    }
    let controls_size = usize::try_from(integer(&patch[8..16])).map_err(|_| Error::Archive)?;
    let delta_size = usize::try_from(integer(&patch[16..24])).map_err(|_| Error::Archive)?;
    if u64::try_from(integer(&patch[24..32])).ok() != Some(target_size) {
        return Err(Error::Archive);
    }
    let controls_end = 32_usize.checked_add(controls_size).ok_or(Error::Archive)?;
    let delta_end = controls_end.checked_add(delta_size).ok_or(Error::Archive)?;
    if delta_end > patch.len() {
        return Err(Error::Archive);
    }
    let mut controls = BzDecoder::new(&patch[32..controls_end]);
    let mut record = [0; 24];
    let mut source = 0_i64;
    let mut produced = 0_u64;
    let mut records = 0_u64;
    loop {
        check_cancel(cancel)?;
        if controls.read(&mut record[..1])? == 0 {
            break;
        }
        records += 1;
        if records > target_size + 1 {
            return Err(Error::Archive);
        }
        controls.read_exact(&mut record[1..])?;
        let add = integer(&record[..8]);
        let copy = integer(&record[8..16]);
        let seek = integer(&record[16..]);
        if add < 0 || copy < 0 {
            return Err(Error::Archive);
        }
        produced = produced
            .checked_add(add as u64)
            .and_then(|n| n.checked_add(copy as u64))
            .filter(|n| *n <= target_size)
            .ok_or(Error::Archive)?;
        source = source.checked_add(add).ok_or(Error::Archive)?;
        if add > 0 && source as u64 > base_size {
            return Err(Error::Archive);
        }
        source = source
            .checked_add(seek)
            .filter(|n| *n >= 0)
            .ok_or(Error::Archive)?;
    }
    if produced != target_size {
        return Err(Error::Size);
    }
    Ok(())
}

fn integer(bytes: &[u8]) -> i64 {
    let raw = u64::from_le_bytes(bytes.try_into().unwrap());
    let magnitude = (raw & i64::MAX as u64) as i64;
    if raw >> 63 == 0 {
        magnitude
    } else {
        -magnitude
    }
}

// Offline generation: release tooling, never signing or publishing.

/// What [`generate`] wrote. Add an offer to the manifest only when `listable`.
#[derive(Debug, Serialize)]
pub struct Summary {
    pub size: u64,
    pub sha256: String,
    pub copied: usize,
    pub bsdiff: usize,
    pub whole: usize,
}

/// Writes a new patch file at `output` from `base` (a full archive of
/// `base_version`, or its unpacked package folder) to the full archive
/// `archive` of `version`, and proves it by rebuilding `archive`'s package from
/// the base exactly as a client does before persisting. Never overwrites.
pub fn generate(
    target: Target,
    base: &Path,
    base_version: &Version,
    archive: &Path,
    version: &Version,
    output: &Path,
) -> Result<Summary, Error> {
    if version.cmp_precedence(base_version).is_le() {
        return Err(Error::Version);
    }
    let parent = output.parent().ok_or(Error::Configuration(
        "patch output needs a parent directory",
    ))?;
    let cancel = CancelToken::default();
    let work = tempfile::Builder::new()
        .prefix(".captures-native-patch-")
        .tempdir_in(parent)?;
    let base_root = if base.symlink_metadata()?.is_dir() {
        base.to_owned()
    } else {
        let root = work.path().join("base");
        staging::unpack(target, &mut File::open(base)?, root, &cancel)?.0
    };
    let (target_root, executables) = staging::unpack(
        target,
        &mut File::open(archive)?,
        work.path().join("target"),
        &cancel,
    )?;
    staging::validate_package(&target_root, target, &executables, &cancel)?;
    let base_tree = tree(&base_root, &HashSet::new(), &cancel)?;
    let target_tree = tree(&target_root, &executables, &cancel)?;
    let root = target_root
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(Error::ArchivePath)?
        .to_owned();
    let (bytes, summary) = encode(
        target,
        base_version,
        version,
        root,
        &base_root,
        &base_tree,
        &target_root,
        &target_tree,
    )?;

    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(&bytes)?;
    file.as_file().sync_all()?;
    // The same code path a client runs, against the base as it unpacks.
    let (rebuilt, rebuilt_executables) = rebuild(
        file.as_file_mut(),
        &Expected {
            target,
            base_version,
            version: &version.to_string(),
        },
        &base_root,
        work.path().join("stage"),
        &cancel,
    )?;
    staging::validate_package(&rebuilt, target, &rebuilt_executables, &cancel)?;
    if rebuilt.file_name() != target_root.file_name()
        || tree(&rebuilt, &rebuilt_executables, &cancel)? != target_tree
    {
        return Err(Error::Package);
    }
    file.persist_noclobber(output)
        .map_err(|error| Error::Io(error.error))?;
    Ok(Summary {
        size: bytes.len() as u64,
        sha256: format!("{:x}", Sha256::digest(&bytes)),
        ..summary
    })
}

/// Every directory and file beneath a package folder with sizes and hashes;
/// execute bits come from `executables` (as extraction recorded them).
fn tree(
    root: &Path,
    executables: &HashSet<PathBuf>,
    cancel: &CancelToken,
) -> Result<BTreeMap<String, Node>, Error> {
    walk(root, cancel)?
        .into_iter()
        .map(|(path, found)| {
            let node = if found.directory {
                Node::Dir
            } else {
                if found.size > staging::MAX_FILE_BYTES {
                    return Err(Error::Archive);
                }
                Node::File {
                    size: found.size,
                    sha256: hash_file(&found.path, cancel)?,
                    executable: executables.contains(&found.path),
                }
            };
            Ok((path, node))
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn encode(
    target: Target,
    base_version: &Version,
    version: &Version,
    root: String,
    base_root: &Path,
    base_tree: &BTreeMap<String, Node>,
    target_root: &Path,
    target_tree: &BTreeMap<String, Node>,
) -> Result<(Vec<u8>, Summary), Error> {
    let by_hash: HashMap<[u8; 32], &str> = base_tree
        .iter()
        .filter_map(|(path, node)| match node {
            Node::File { sha256, .. } => Some((*sha256, path.as_str())),
            Node::Dir => None,
        })
        .collect();
    let hex = |hash: &[u8; 32]| hash.iter().map(|b| format!("{b:02x}")).collect::<String>();
    let mut data = Vec::new();
    let mut entries = Vec::new();
    let mut summary = Summary {
        size: 0,
        sha256: String::new(),
        copied: 0,
        bsdiff: 0,
        whole: 0,
    };
    for (path, node) in target_tree {
        let Node::File {
            size,
            sha256,
            executable,
        } = node
        else {
            entries.push(Entry::Dir { path: path.clone() });
            continue;
        };
        let same_path =
            matches!(base_tree.get(path), Some(Node::File { sha256: base, .. }) if base == sha256);
        let source = if same_path {
            summary.copied += 1;
            FileSource::Copy { base: path.clone() }
        } else if let Some(base) = by_hash.get(sha256) {
            summary.copied += 1;
            FileSource::Copy {
                base: (*base).to_owned(),
            }
        } else {
            let bytes = fs::read(join(target_root, path))?;
            let mut whole = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::best());
            whole.write_all(&bytes)?;
            let whole = whole.finish()?;
            let mut diff = None;
            if let Some(Node::File {
                size: base_size,
                sha256: base_sha256,
                ..
            }) = base_tree.get(path)
            {
                let source = fs::read(join(base_root, path))?;
                let mut delta = Vec::new();
                qbsdiff::Bsdiff::new(&source, &bytes)
                    .compression_level(9)
                    .compare(&mut delta)?;
                if delta.len() < whole.len() {
                    diff = Some((delta, *base_size, hex(base_sha256)));
                }
            }
            let offset = data.len() as u64;
            match diff {
                Some((delta, base_size, base_sha256)) => {
                    summary.bsdiff += 1;
                    data.extend_from_slice(&delta);
                    FileSource::Bsdiff {
                        base: path.clone(),
                        base_size,
                        base_sha256,
                        offset,
                        length: delta.len() as u64,
                    }
                }
                None => {
                    summary.whole += 1;
                    data.extend_from_slice(&whole);
                    FileSource::Whole {
                        offset,
                        length: whole.len() as u64,
                    }
                }
            }
        };
        entries.push(Entry::File {
            path: path.clone(),
            executable: *executable,
            size: *size,
            sha256: hex(sha256),
            source,
        });
    }
    let header = Header {
        schema: SCHEMA,
        target,
        base_version: base_version.to_string(),
        version: version.to_string(),
        root,
        entries,
    };
    let json = serde_json::to_vec(&header).map_err(io::Error::other)?;
    let length = u32::try_from(json.len())
        .ok()
        .filter(|length| *length <= MAX_HEADER_BYTES)
        .ok_or(Error::Archive)?;
    let mut bytes = Vec::with_capacity(MAGIC.len() + 4 + json.len() + data.len());
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&length.to_le_bytes());
    bytes.extend_from_slice(&json);
    bytes.extend_from_slice(&data);
    Ok((bytes, summary))
}

#[cfg(test)]
mod tests;
