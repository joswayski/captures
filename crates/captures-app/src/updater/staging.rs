//! Bounded extraction of the development packages produced by native/package.py.
//! Staging never installs, replaces, executes or changes an existing profile.
use super::{
    DEVELOPMENT_IDENTITY, Error, ReleaseInfo, Target, VerifiedUpdate, check_cancel, decode_hash,
    verify_file,
};
use captures_media::CancelToken;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    fs::{self, File},
    io::{self, Cursor, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};
use tempfile::TempDir;

pub(super) const MAX_ENTRIES: usize = 10_000;
pub(super) const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;
pub(super) const MAX_EXPANDED_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_METADATA_BYTES: u64 = 256 * 1024;
const MAX_CENTRAL_BYTES: u64 = 8 * 1024 * 1024;

/// A validated private package, not an installed update. Drop removes it.
/// Retain this owner while inspecting paths. Replacement re-extracts the retained
/// verified archive; exposed files are not trusted as installation inputs.
pub struct StagedUpdate {
    // Close the archive before TempDir cleanup, including on Windows.
    pub(super) archive: VerifiedUpdate,
    _directory: TempDir,
    package: PathBuf,
    executable: PathBuf,
}

impl StagedUpdate {
    pub fn info(&self) -> &ReleaseInfo {
        self.archive.info()
    }
    pub fn package(&self) -> &Path {
        &self.package
    }
    pub fn executable(&self) -> &Path {
        &self.executable
    }
}

impl VerifiedUpdate {
    /// Consume the verified download and rehash a private copy before parsing.
    /// The caller supplies an existing scratch directory, never an install path.
    /// Failure/cancellation removes all staging files; success owns them until Drop.
    pub fn stage(mut self, directory: &Path, cancel: &CancelToken) -> Result<StagedUpdate, Error> {
        check_cancel(cancel)?;
        let directory = tempfile::Builder::new()
            .prefix("captures-native-update-")
            .tempdir_in(directory)?;
        self.file.seek(SeekFrom::Start(0))?;
        let mut copy = verify_file(
            self.file.as_file_mut(),
            directory.path(),
            &self.info,
            self.hash,
            cancel,
            |_, _| {},
        )?;
        let mut extraction = Extraction::new(directory.path().join("package"));
        match self.info.target {
            Target::LinuxX64 => extract_tar(copy.file.as_file_mut(), &mut extraction, cancel)?,
            _ => extract_zip(copy.file.as_file_mut(), &mut extraction, cancel)?,
        }
        let package = extraction
            .root
            .join(extraction.package.ok_or(Error::Package)?);
        let executable =
            validate_package(&package, self.info.target, &extraction.executables, cancel)?;
        check_cancel(cancel)?;
        Ok(StagedUpdate {
            archive: copy,
            _directory: directory,
            package,
            executable,
        })
    }
}

struct Extraction {
    root: PathBuf,
    package: Option<String>,
    nodes: BTreeMap<String, (String, bool)>,
    declared: HashSet<String>,
    executables: HashSet<PathBuf>,
    expanded: u64,
}

impl Extraction {
    fn new(root: PathBuf) -> Self {
        Self {
            root,
            package: None,
            nodes: BTreeMap::new(),
            declared: HashSet::new(),
            executables: HashSet::new(),
            expanded: 0,
        }
    }

    fn reserve(&mut self, name: &str, directory: bool, size: u64) -> Result<PathBuf, Error> {
        let name = if directory {
            name.strip_suffix('/').unwrap_or(name)
        } else {
            name
        };
        if name.is_empty() || name.len() > 1024 {
            return Err(Error::ArchivePath);
        }
        let components: Vec<_> = name.split('/').collect();
        for (index, component) in components.iter().enumerate() {
            // Current packaged resource names are portable ASCII. The arbitrary
            // outer package folder may contain Unicode, as package.py supports.
            if component.is_empty()
                || component.len() > 255
                || matches!(*component, "." | "..")
                || component.ends_with(['.', ' '])
                || (index > 0 && !component.is_ascii())
                || component
                    .chars()
                    .any(|c| c.is_control() || "\\:*?\"<>|".contains(c))
            {
                return Err(Error::ArchivePath);
            }
            let device = component.split('.').next().unwrap().to_ascii_uppercase();
            if matches!(device.as_str(), "CON" | "PRN" | "AUX" | "NUL")
                || (device.len() == 4
                    && (device.starts_with("COM") || device.starts_with("LPT"))
                    && matches!(device.as_bytes()[3], b'1'..=b'9'))
            {
                return Err(Error::ArchivePath);
            }
        }
        if self
            .package
            .as_deref()
            .is_some_and(|root| root != components[0])
        {
            return Err(Error::ArchivePath);
        }
        self.package.get_or_insert_with(|| components[0].to_owned());
        if self.declared.len() >= MAX_ENTRIES
            || size > MAX_FILE_BYTES
            || (directory && size != 0)
            || self.expanded.saturating_add(size) > MAX_EXPANDED_BYTES
        {
            return Err(Error::Archive);
        }
        if !self.declared.insert(name.to_ascii_uppercase()) {
            return Err(Error::ArchivePath);
        }
        for end in 1..=components.len() {
            let prefix = components[..end].join("/");
            let is_dir = end < components.len() || directory;
            match self.nodes.get(&prefix.to_ascii_uppercase()) {
                Some((existing, true)) if is_dir && existing == &prefix => {}
                Some(_) => return Err(Error::ArchivePath),
                None => {
                    // Count implicit directories too: short archives must not
                    // create an unbounded tree of path-index allocations.
                    if self.nodes.len() >= MAX_ENTRIES {
                        return Err(Error::Archive);
                    }
                    self.nodes
                        .insert(prefix.to_ascii_uppercase(), (prefix, is_dir));
                }
            }
        }
        self.expanded += size;
        Ok(self.root.join(name))
    }
}

fn copy_entry(
    mut reader: impl Read,
    mut writer: impl Write,
    size: u64,
    cancel: &CancelToken,
) -> Result<(), Error> {
    let mut copied = 0;
    let mut buffer = [0; 64 * 1024];
    // One extra read both detects a lying declaration and observes ZIP CRC at EOF.
    let mut reader = (&mut reader).take(size + 1);
    loop {
        check_cancel(cancel)?;
        let count = reader.read(&mut buffer).map_err(|_| Error::Archive)?;
        check_cancel(cancel)?;
        if count == 0 {
            break;
        }
        copied += count as u64;
        if copied > size {
            return Err(Error::Archive);
        }
        writer.write_all(&buffer[..count])?;
    }
    if copied != size {
        return Err(Error::Archive);
    }
    Ok(())
}

fn write_entry(
    reader: impl Read,
    path: &Path,
    directory: bool,
    size: u64,
    executable: bool,
    cancel: &CancelToken,
) -> Result<(), Error> {
    if directory {
        fs::create_dir_all(path)?;
        return copy_entry(reader, io::sink(), size, cancel);
    }
    fs::create_dir_all(path.parent().ok_or(Error::ArchivePath)?)?;
    let mut file = File::options().write(true).create_new(true).open(path)?;
    copy_entry(reader, &mut file, size, cancel)?;
    file.sync_all()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(if executable {
            0o700
        } else {
            0o600
        }))?;
    }
    #[cfg(not(unix))]
    let _ = executable;
    Ok(())
}

fn extract_tar(
    file: &mut File,
    extraction: &mut Extraction,
    cancel: &CancelToken,
) -> Result<(), Error> {
    let decoder = flate2::read::GzDecoder::new(file);
    // Include raw metadata, padding and headers, not only published file bytes.
    let mut limited = decoder.take(MAX_EXPANDED_BYTES + MAX_ENTRIES as u64 * 1024 + 1);
    let mut archive = tar::Archive::new(&mut limited);
    let mut next_path = None;
    for (index, entry) in archive
        .entries()
        .map_err(|_| Error::Archive)?
        .raw(true)
        .enumerate()
    {
        check_cancel(cancel)?;
        if index >= MAX_ENTRIES {
            return Err(Error::Archive);
        }
        let mut entry = entry.map_err(|_| Error::Archive)?;
        let kind = entry.header().entry_type();
        if kind.is_pax_local_extensions() {
            if next_path.is_some() || entry.size() > MAX_METADATA_BYTES {
                return Err(Error::Archive);
            }
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).map_err(|_| Error::Archive)?;
            for field in tar::PaxExtensions::new(&bytes) {
                let field = field.map_err(|_| Error::Archive)?;
                match field.key().map_err(|_| Error::Archive)? {
                    "path" if next_path.is_none() => {
                        next_path = Some(field.value().map_err(|_| Error::Archive)?.to_owned())
                    }
                    "mtime" | "atime" | "ctime" | "uid" | "gid" | "uname" | "gname" => {}
                    _ => return Err(Error::Archive),
                }
            }
            continue;
        }
        if !kind.is_file() && !kind.is_dir() {
            return Err(Error::Archive);
        }
        let name = match next_path.take() {
            Some(name) => name,
            None => std::str::from_utf8(&entry.path_bytes())
                .map_err(|_| Error::ArchivePath)?
                .to_owned(),
        };
        let size = entry.size();
        let mode = entry.header().mode().map_err(|_| Error::Archive)?;
        if mode & 0o7000 != 0 {
            return Err(Error::Archive);
        }
        let path = extraction.reserve(&name, kind.is_dir(), size)?;
        if mode & 0o111 != 0 && kind.is_file() {
            extraction.executables.insert(path.clone());
        }
        write_entry(
            &mut entry,
            &path,
            kind.is_dir(),
            size,
            mode & 0o111 != 0,
            cancel,
        )?;
    }
    if next_path.is_some() {
        return Err(Error::Archive);
    }
    // Tar stops at its zero blocks. Drain boundedly to observe the gzip trailer
    // checksum, rather than accepting a truncated/corrupted compressed stream.
    let mut buffer = [0; 64 * 1024];
    loop {
        check_cancel(cancel)?;
        if limited.read(&mut buffer).map_err(|_| Error::Archive)? == 0 {
            break;
        }
    }
    if limited.limit() == 0 {
        return Err(Error::Archive);
    }
    Ok(())
}

struct ZipEntry {
    name: String,
    path: PathBuf,
    size: u64,
    compressed: u64,
    crc: u32,
    offset: u64,
    method: u16,
    directory: bool,
    executable: bool,
}

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap())
}
fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

fn zip_entries(
    file: &mut File,
    extraction: &mut Extraction,
    cancel: &CancelToken,
) -> Result<(Vec<ZipEntry>, u64), Error> {
    let length = file.metadata()?.len();
    if length < 22 {
        return Err(Error::Archive);
    }
    file.seek(SeekFrom::End(-22))?;
    let mut end = [0; 22];
    file.read_exact(&mut end).map_err(|_| Error::Archive)?;
    let count = u16_at(&end, 10) as usize;
    let size = u32_at(&end, 12) as u64;
    let start = u32_at(&end, 16) as u64;
    // Current package.py output needs no ZIP64, comments, volumes or SFX prefix.
    if end[..4] != *b"PK\x05\x06"
        || u16_at(&end, 4) != 0
        || u16_at(&end, 6) != 0
        || u16_at(&end, 8) as usize != count
        || u16_at(&end, 20) != 0
        || count == 0
        || count > MAX_ENTRIES
        || size > MAX_CENTRAL_BYTES
        || start + size != length - 22
    {
        return Err(Error::Archive);
    }
    file.seek(SeekFrom::Start(start))?;
    let mut central = vec![0; size as usize];
    file.read_exact(&mut central).map_err(|_| Error::Archive)?;
    let mut entries = Vec::with_capacity(count);
    let mut at = 0;
    for _ in 0..count {
        check_cancel(cancel)?;
        let header = central.get(at..at + 46).ok_or(Error::Archive)?;
        if header[..4] != *b"PK\x01\x02" {
            return Err(Error::Archive);
        }
        let flags = u16_at(header, 8);
        let method = u16_at(header, 10);
        let name_len = u16_at(header, 28) as usize;
        let extra_len = u16_at(header, 30) as usize;
        let comment_len = u16_at(header, 32) as usize;
        let offset = u32_at(header, 42) as u64;
        let compressed = u32_at(header, 20) as u64;
        let plain = u32_at(header, 24) as u64;
        if flags & !0x0800 != 0
            || !matches!(method, 0 | 8)
            || u16_at(header, 34) != 0
            || offset >= start
            || compressed == u32::MAX as u64
            || plain == u32::MAX as u64
        {
            return Err(Error::Archive);
        }
        let raw_name = central
            .get(at + 46..at + 46 + name_len)
            .ok_or(Error::Archive)?;
        let name = std::str::from_utf8(raw_name)
            .map_err(|_| Error::ArchivePath)?
            .to_owned();
        // Native ZIPs have no extra-field interpretation or per-file comments.
        // Refuse these instead of enabling ZIP64/AES or platform metadata.
        if extra_len != 0 || comment_len != 0 {
            return Err(Error::Archive);
        }
        let directory = name.ends_with('/');
        let attributes = u32_at(header, 38);
        let mode = if header[5] == 3 { attributes >> 16 } else { 0 };
        if !matches!(header[5], 0 | 3)
            || mode & 0o7000 != 0
            || !matches!(mode & 0o170000, 0 | 0o100000 | 0o040000)
            || (mode & 0o170000 == 0o040000 && !directory)
            || (mode & 0o170000 == 0o100000 && directory)
            || attributes & 0x08 != 0
            || (attributes & 0x10 != 0 && !directory)
        {
            return Err(Error::Archive);
        }
        let path = extraction.reserve(&name, directory, plain)?;
        entries.push(ZipEntry {
            name,
            path,
            size: plain,
            compressed,
            crc: u32_at(header, 16),
            offset,
            method,
            directory,
            executable: mode & 0o111 != 0,
        });
        at += 46 + name_len;
    }
    if at != central.len() {
        return Err(Error::Archive);
    }
    Ok((entries, start))
}

fn extract_zip(
    file: &mut File,
    extraction: &mut Extraction,
    cancel: &CancelToken,
) -> Result<(), Error> {
    // ZipArchive may fall back to earlier footers and allocates before callers
    // can enforce counts; it also collapses duplicate names. Validate bounded
    // central metadata ourselves, then stream and cross-check every local entry.
    let (entries, central_start) = zip_entries(file, extraction, cancel)?;
    file.seek(SeekFrom::Start(0))?;
    for metadata in entries {
        check_cancel(cancel)?;
        if file.stream_position()? != metadata.offset {
            return Err(Error::Archive);
        }
        // Apply the same restricted format to local headers before zip-rs can
        // interpret extras (including ZIP64) or allocate names from this entry.
        let mut local = [0; 30];
        file.read_exact(&mut local).map_err(|_| Error::Archive)?;
        if local[..4] != *b"PK\x03\x04"
            || u16_at(&local, 6) & !0x0800 != 0
            || u16_at(&local, 26) as usize != metadata.name.len()
            || u16_at(&local, 28) != 0
        {
            return Err(Error::Archive);
        }
        file.seek(SeekFrom::Start(metadata.offset))?;
        let mut entry = zip::read::read_zipfile_from_stream(file)
            .map_err(|_| Error::Archive)?
            .ok_or(Error::Archive)?;
        if entry.name_raw() != metadata.name.as_bytes()
            || entry.size() != metadata.size
            || entry.compressed_size() != metadata.compressed
            || entry.crc32() != metadata.crc
            || entry.compression()
                != if metadata.method == 0 {
                    zip::CompressionMethod::Stored
                } else {
                    zip::CompressionMethod::Deflated
                }
        {
            return Err(Error::Archive);
        }
        if metadata.executable && !metadata.directory {
            extraction.executables.insert(metadata.path.clone());
        }
        write_entry(
            &mut entry,
            &metadata.path,
            metadata.directory,
            metadata.size,
            metadata.executable,
            cancel,
        )?;
    }
    if file.stream_position()? != central_start {
        return Err(Error::Archive);
    }
    Ok(())
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct BuildInfo {
    development: bool,
    platform: String,
    media_target: Option<String>,
    binary: String,
    binary_sha256: String,
    source_commit: Option<String>,
}

fn metadata(path: &Path) -> Result<Vec<u8>, Error> {
    if fs::metadata(path).map_err(|_| Error::Package)?.len() > MAX_METADATA_BYTES {
        return Err(Error::Package);
    }
    fs::read(path).map_err(|_| Error::Package)
}

fn required_file(path: &Path) -> Result<(), Error> {
    if !fs::metadata(path).is_ok_and(|info| info.is_file() && info.len() > 0) {
        return Err(Error::Package);
    }
    Ok(())
}

pub(super) fn validate_package(
    package: &Path,
    target: Target,
    executables: &HashSet<PathBuf>,
    cancel: &CancelToken,
) -> Result<PathBuf, Error> {
    let (platform, binary, resources) = match target {
        Target::MacArm64 | Target::MacX64 => (
            "macos",
            "Captures Native Development.app/Contents/MacOS/CapturesNative",
            "Captures Native Development.app/Contents/Resources",
        ),
        Target::WindowsX64 => ("windows", "CapturesNative.exe", ""),
        Target::LinuxX64 => ("linux", "captures-native", ""),
    };
    let build: BuildInfo = serde_json::from_slice(&metadata(&package.join("BUILD_INFO.json"))?)
        .map_err(|_| Error::Package)?;
    if !build.development
        || build.platform != platform
        || build.binary != binary
        || build.media_target.as_deref() != Some(target.as_str())
        || build.source_commit.as_ref().is_some_and(|commit| {
            commit.len() != 40 || !commit.bytes().all(|c| c.is_ascii_hexdigit())
        })
    {
        return Err(Error::Package);
    }
    for name in ["LICENSE", "TRADEMARKS.md", "TESTING.md"] {
        required_file(&package.join(name))?;
    }
    let executable = package.join(binary);
    required_file(&executable)?;
    if platform != "windows" && !executables.contains(&executable) {
        return Err(Error::Package);
    }
    for name in ["ffmpeg", "ffprobe"] {
        let tool = executable.parent().unwrap().join("binaries").join(format!(
            "{name}-{}{}",
            target.as_str(),
            if platform == "windows" { ".exe" } else { "" }
        ));
        required_file(&tool)?;
        if platform != "windows" && !executables.contains(&tool) {
            return Err(Error::Package);
        }
    }
    let licenses = package.join(resources).join("media-licenses");
    for name in ["LICENSE", "NOTICE.md"] {
        required_file(&licenses.join("openh264").join(name))?;
    }
    let ffmpeg = licenses.join("ffmpeg");
    let sources: Vec<_> = fs::read_dir(&ffmpeg)
        .map_err(|_| Error::Package)?
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.starts_with("ffmpeg-") && name.ends_with(".tar.xz"))
        .collect();
    if sources.len() != 1 {
        return Err(Error::Package);
    }
    let stem = sources[0].strip_suffix(".tar.xz").unwrap();
    for suffix in [
        ".tar.xz",
        ".tar.xz.asc",
        "-BUILD_CONFIG.txt",
        "-COPYING.LGPLv2.1",
        "-NOTICE.md",
    ] {
        required_file(&ffmpeg.join(format!("{stem}{suffix}")))?;
    }
    if platform == "macos" {
        let plist = plist::Value::from_reader(Cursor::new(metadata(
            &package.join("Captures Native Development.app/Contents/Info.plist"),
        )?))
        .map_err(|_| Error::Package)?;
        let identity = plist
            .as_dictionary()
            .and_then(|dictionary| dictionary.get("CFBundleIdentifier"))
            .and_then(plist::Value::as_string);
        if identity != Some(DEVELOPMENT_IDENTITY) {
            return Err(Error::Package);
        }
        required_file(
            &package
                .join(resources)
                .join("CapturesNative_CapturesNative.bundle/tokens.json"),
        )?;
        required_file(&package.join(resources).join("Captures.icns"))?;
    }
    let mut file = File::open(&executable)?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        check_cancel(cancel)?;
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    if <[u8; 32]>::from(hash.finalize())
        != decode_hash(&build.binary_sha256).map_err(|_| Error::Package)?
    {
        return Err(Error::Package);
    }
    check_cancel(cancel)?;
    Ok(executable)
}

#[cfg(test)]
pub(in crate::updater) mod tests;
