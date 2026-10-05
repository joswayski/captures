//! Replacement of an explicitly selected, stopped development package.
//! The caller owns the trusted parent directory and must keep profiles outside
//! the package. Only explicit development handoff launches a host; no registration
//! or production installation occurs.
use super::{Error, ReleaseInfo, StagedUpdate, Target, check_cancel, decode_hash, staging};
use captures_media::CancelToken;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
};

mod launch;
pub use launch::LaunchFailure;

/// An activated development package whose previous package is still retained.
/// Drop deliberately does nothing: another process can recover an interruption.
/// Confirm only after the host's health check; otherwise explicitly roll back.
pub struct PendingInstallation {
    paths: Paths,
    info: ReleaseInfo,
    id: uuid::Uuid,
}

impl PendingInstallation {
    pub fn info(&self) -> &ReleaseInfo {
        &self.info
    }

    /// Accept the replacement and remove the retained previous package.
    /// This does not launch or perform a health check on the new executable.
    pub fn confirm(self) -> Result<(), Error> {
        let _lock = self.paths.lock()?;
        self.confirm_locked(|| Ok(()))
    }

    fn confirm_locked(
        &self,
        before_commit: impl FnOnce() -> Result<(), Error>,
    ) -> Result<(), Error> {
        let receipt = self.paths.receipt()?;
        require_id(&receipt, Some(self.id))?;
        require_fingerprint(&self.paths.package, receipt.target, &receipt.new_hash)?;
        if !self.paths.confirmed()? {
            require_fingerprint(&self.paths.previous(), receipt.target, &receipt.old_hash)?;
            // Rehashing large packages can take time after acknowledgement.
            // Let the handoff veto confirmation immediately before its marker.
            before_commit()?;
            let mut marker = tempfile::NamedTempFile::new_in(&self.paths.transaction)?;
            marker.write_all(b"confirmed\n")?;
            marker.as_file().sync_all()?;
            marker
                .persist_noclobber(self.paths.transaction.join("confirmed"))
                .map_err(|error| Error::Io(error.error))?;
            sync_directory(&self.paths.transaction)?;
        }
        // The marker distinguishes partial backup deletion from a pending update.
        self.paths.cleanup()
    }

    /// Restore exact previous package bytes without executing either version.
    pub fn rollback(self) -> Result<(), Error> {
        recover(self.paths, Some(self.id)).map(|_| ())
    }
}

impl StagedUpdate {
    /// Replace an existing native DEVELOPMENT PACKAGE ROOT, not an .app bundle,
    /// installed Preview, executable or profile directory. The absolute destination
    /// must be on this host's target, with an externally managed/trusted parent.
    /// All app processes must be stopped. This process must run outside the package.
    /// Rehash/re-extract the retained signed archive rather than trust exposed paths.
    /// Cancellation is observed before the two-rename activation, not between them.
    /// No host calls this yet. It does not launch, register or migrate profile data.
    pub fn replace(
        self,
        destination: &Path,
        cancel: &CancelToken,
    ) -> Result<PendingInstallation, Error> {
        check_cancel(cancel)?;
        require_host_target(self.info().target)?;
        let paths = Paths::new(destination)?;
        let _lock = paths.lock()?;
        if directory_present(&paths.transaction)? || directory_present(&paths.garbage())? {
            return Err(Error::Installation("recover the pending replacement first"));
        }
        let info = self.info().clone();
        let id = uuid::Uuid::new_v4();
        let old_hash = fingerprint(&paths.package, info.target, cancel)?;
        let builder = fs::DirBuilder::new();
        #[cfg(unix)]
        let builder = {
            use std::os::unix::fs::DirBuilderExt;
            let mut builder = builder;
            builder.mode(0o700);
            builder
        };
        builder.create(&paths.transaction)?;
        let mut old_moved = false;
        let result = (|| {
            // Staged files may have been mutated after their paths were exposed.
            // Only the retained archive's signed digest is authoritative.
            let fresh = self.archive.stage(&paths.transaction, cancel)?;
            let new_hash = fingerprint(fresh.package(), info.target, cancel)?;
            let receipt = Receipt {
                schema: 1,
                id,
                destination: paths.name().to_owned(),
                target: info.target,
                old_hash,
                new_hash,
            };
            let mut file = File::options()
                .write(true)
                .create_new(true)
                .open(paths.transaction.join("receipt.json"))?;
            serde_json::to_writer(&mut file, &receipt).map_err(std::io::Error::other)?;
            file.sync_all()?;
            sync_directory(&paths.transaction)?;
            check_cancel(cancel)?;
            require_fingerprint(&paths.package, info.target, &receipt.old_hash)?;
            check_cancel(cancel)?;
            fs::rename(&paths.package, paths.previous())?;
            old_moved = true;
            if let Err(error) = fs::rename(fresh.package(), &paths.package) {
                // Best-effort restoration. Never clean up a retained backup if
                // its rename fails; the receipt allows recovery by another process.
                if fs::rename(paths.previous(), &paths.package).is_ok() {
                    old_moved = false;
                }
                return Err(Error::Io(error));
            }
            sync_directory(paths.package.parent().unwrap())?;
            Ok(())
        })();
        if let Err(error) = result {
            if !old_moved {
                paths.cleanup()?;
            }
            return Err(error);
        }
        Ok(PendingInstallation { paths, info, id })
    }
}

/// Recover the known absolute development-package destination after interruption.
/// Unconfirmed replacements roll back. Confirmed replacements finish cleanup.
/// Returns false when no transaction exists. Unknown/changed packages, bad receipts
/// and links are left untouched for manual recovery. This never executes an app.
/// Parents must remain trusted and profiles must be outside the package. This is
/// process-interruption recovery, not a cross-platform power-loss durability claim.
pub fn recover_installation(destination: &Path) -> Result<bool, Error> {
    recover(Paths::new(destination)?, None)
}

fn recover(paths: Paths, expected_id: Option<uuid::Uuid>) -> Result<bool, Error> {
    let _lock = paths.lock()?;
    if directory_present(&paths.garbage())? {
        if expected_id.is_some() || directory_present(&paths.transaction)? {
            return Err(Error::Installation("unexpected replacement cleanup state"));
        }
        // The atomic rename into garbage already committed the decision. An
        // interruption while deleting it must not require a now-deleted receipt.
        fs::remove_dir_all(paths.garbage())?;
        sync_directory(paths.package.parent().unwrap())?;
        return Ok(true);
    }
    if !directory_present(&paths.transaction)? {
        if expected_id.is_some() {
            return Err(Error::Installation("replacement is no longer pending"));
        }
        return Ok(false);
    }
    let receipt = paths.receipt()?;
    require_id(&receipt, expected_id)?;
    if paths.confirmed()? {
        require_fingerprint(&paths.package, receipt.target, &receipt.new_hash)?;
        paths.cleanup()?;
        return Ok(true);
    }
    let previous = paths.previous();
    let rejected = paths.transaction.join("rejected");
    if directory_present(&previous)? {
        require_fingerprint(&previous, receipt.target, &receipt.old_hash)?;
        if directory_present(&rejected)? {
            require_fingerprint(&rejected, receipt.target, &receipt.new_hash)?;
            if directory_present(&paths.package)? {
                return Err(Error::Installation("unexpected package during rollback"));
            }
        } else if directory_present(&paths.package)? {
            require_fingerprint(&paths.package, receipt.target, &receipt.new_hash)?;
            fs::rename(&paths.package, &rejected)?;
        }
        // Handles interruptions after either the first activation rename or the
        // first rollback rename. A failure leaves both package trees retained.
        fs::rename(&previous, &paths.package)?;
        sync_directory(paths.package.parent().unwrap())?;
    } else {
        // Activation never moved the old package, or rollback already restored it.
        require_fingerprint(&paths.package, receipt.target, &receipt.old_hash)?;
        if directory_present(&rejected)? {
            require_fingerprint(&rejected, receipt.target, &receipt.new_hash)?;
        }
    }
    paths.cleanup()?;
    Ok(true)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    schema: u32,
    id: uuid::Uuid,
    destination: String,
    target: Target,
    old_hash: String,
    new_hash: String,
}

struct Paths {
    package: PathBuf,
    transaction: PathBuf,
    lock: PathBuf,
}

struct OperationLock(File);

impl Drop for OperationLock {
    fn drop(&mut self) {
        // Closing only our descriptor is insufficient if a concurrent spawn
        // inherited a dup between fork and exec. Release the shared lock now.
        let _ = self.0.unlock();
    }
}

impl Paths {
    fn new(destination: &Path) -> Result<Self, Error> {
        if !destination.is_absolute() {
            return Err(Error::Configuration(
                "replacement destination must be absolute",
            ));
        }
        let name = destination
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(Error::Configuration("invalid replacement destination"))?;
        let parent = destination
            .parent()
            .ok_or(Error::Configuration("invalid replacement destination"))?
            .canonicalize()?;
        let package = parent.join(name);
        let package = if directory_present(&package)? {
            // Resolve the actual name too, including Windows case aliases.
            package.canonicalize()?
        } else {
            package
        };
        let name = package.file_name().unwrap().to_str().unwrap();
        let parent = package.parent().unwrap();
        // Require the helper to run outside the directory it is going to move.
        if std::env::current_exe()?
            .canonicalize()?
            .starts_with(&package)
        {
            return Err(Error::Installation(
                "the updater must run outside the package",
            ));
        }
        let id = format!("{:x}", Sha256::digest(name.as_bytes()));
        let stem = format!(".captures-native-install-{id}");
        let transaction = parent.join(&stem);
        let lock = parent.join(format!("{stem}.lock"));
        Ok(Self {
            package,
            transaction,
            lock,
        })
    }

    fn name(&self) -> &str {
        self.package.file_name().unwrap().to_str().unwrap()
    }

    fn previous(&self) -> PathBuf {
        self.transaction.join("previous")
    }

    fn garbage(&self) -> PathBuf {
        self.transaction.with_extension("discard")
    }

    fn lock(&self) -> Result<OperationLock, Error> {
        if let Ok(metadata) = fs::symlink_metadata(&self.lock)
            && (!metadata.is_file() || metadata.file_type().is_symlink())
        {
            return Err(Error::Installation("invalid replacement lock"));
        }
        // Keep the empty lock file after cleanup: deleting it would let another
        // process lock a different inode. OS locks release on process termination.
        let lock = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&self.lock)?;
        lock.try_lock()
            .map_err(|_| Error::Installation("another replacement operation is running"))?;
        Ok(OperationLock(lock))
    }

    fn receipt(&self) -> Result<Receipt, Error> {
        let path = self.transaction.join("receipt.json");
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 4096 {
            return Err(Error::Installation("invalid replacement receipt"));
        }
        let receipt: Receipt = serde_json::from_slice(&fs::read(path)?)
            .map_err(|_| Error::Installation("invalid replacement receipt"))?;
        if receipt.schema != 1
            || receipt.destination != self.name()
            || decode_hash(&receipt.old_hash).is_err()
            || decode_hash(&receipt.new_hash).is_err()
        {
            return Err(Error::Installation("invalid replacement receipt"));
        }
        require_host_target(receipt.target)?;
        Ok(receipt)
    }

    fn confirmed(&self) -> Result<bool, Error> {
        let path = self.transaction.join("confirmed");
        match fs::symlink_metadata(&path) {
            Ok(metadata)
                if metadata.is_file()
                    && !metadata.file_type().is_symlink()
                    && metadata.len() == 10 =>
            {
                if fs::read(path)? == b"confirmed\n" {
                    Ok(true)
                } else {
                    Err(Error::Installation("invalid confirmation marker"))
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            _ => Err(Error::Installation("invalid confirmation marker")),
        }
    }

    fn cleanup(&self) -> Result<(), Error> {
        if directory_present(&self.garbage())? {
            return Err(Error::Installation("unexpected replacement cleanup state"));
        }
        // Keep the receipt/marker intact until the decision itself is committed.
        // Renaming first makes interruption during recursive deletion recoverable.
        fs::rename(&self.transaction, self.garbage())?;
        sync_directory(self.package.parent().unwrap())?;
        fs::remove_dir_all(self.garbage())?;
        sync_directory(self.package.parent().unwrap())
    }
}

fn require_id(receipt: &Receipt, expected: Option<uuid::Uuid>) -> Result<(), Error> {
    if expected.is_some_and(|id| id != receipt.id) {
        Err(Error::Installation("replacement is no longer pending"))
    } else {
        Ok(())
    }
}

fn require_host_target(target: Target) -> Result<(), Error> {
    if Target::current_host() == Some(target) {
        Ok(())
    } else {
        Err(Error::Target)
    }
}

fn directory_present(path: &Path) -> Result<bool, Error> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Ok(_) => Err(Error::Installation(
            "package or transaction is not a real directory",
        )),
        Err(error) => Err(Error::Io(error)),
    }
}

fn require_fingerprint(path: &Path, target: Target, expected: &str) -> Result<(), Error> {
    if fingerprint(path, target, &CancelToken::default())? != expected {
        return Err(Error::Installation(
            "package changed; retained files were not removed",
        ));
    }
    Ok(())
}

// Includes every file/directory, empty directories, content and execute bits,
// with framed paths rather than ambiguous concatenation. Timestamps are ignored.
fn fingerprint(package: &Path, target: Target, cancel: &CancelToken) -> Result<String, Error> {
    if !directory_present(package)? {
        return Err(Error::Package);
    }
    let mut hash = Sha256::new();
    let mut pending = vec![package.to_owned()];
    let mut entries = 0;
    let mut expanded = 0u64;
    let mut executables = HashSet::new();
    let mut buffer = [0; 64 * 1024];
    while let Some(path) = pending.pop() {
        check_cancel(cancel)?;
        entries += 1;
        if entries > staging::MAX_ENTRIES {
            return Err(Error::Archive);
        }
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() || (!metadata.is_file() && !metadata.is_dir()) {
            return Err(Error::ArchivePath);
        }
        let name = path
            .strip_prefix(package)
            .unwrap()
            .to_str()
            .ok_or(Error::ArchivePath)?;
        if name.len() > 1024 {
            return Err(Error::ArchivePath);
        }
        hash.update([u8::from(metadata.is_file())]);
        hash.update((name.len() as u64).to_le_bytes());
        hash.update(name.as_bytes());
        #[cfg(unix)]
        let executable = {
            use std::os::unix::fs::PermissionsExt;
            metadata.permissions().mode() & 0o111 != 0
        };
        #[cfg(not(unix))]
        let executable = false;
        hash.update([u8::from(executable)]);
        if executable && metadata.is_file() {
            executables.insert(path.clone());
        }
        if metadata.is_dir() {
            let mut children = Vec::new();
            for entry in fs::read_dir(&path)? {
                check_cancel(cancel)?;
                if entries + pending.len() + children.len() >= staging::MAX_ENTRIES {
                    return Err(Error::Archive);
                }
                children.push(entry?.path());
            }
            children.sort();
            pending.extend(children.into_iter().rev());
        } else {
            if metadata.len() > staging::MAX_FILE_BYTES {
                return Err(Error::Archive);
            }
            expanded = expanded.saturating_add(metadata.len());
            if expanded > staging::MAX_EXPANDED_BYTES {
                return Err(Error::Archive);
            }
            hash.update(metadata.len().to_le_bytes());
            let mut file = File::open(&path)?.take(metadata.len() + 1);
            let mut bytes = 0;
            loop {
                check_cancel(cancel)?;
                let count = file.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                hash.update(&buffer[..count]);
                bytes += count as u64;
            }
            if bytes != metadata.len() {
                return Err(Error::Installation("package changed while reading"));
            }
        }
    }
    staging::validate_package(package, target, &executables, cancel)?;
    Ok(format!("{:x}", hash.finalize()))
}

fn sync_directory(path: &Path) -> Result<(), Error> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path; // No portable Windows directory-fsync equivalent in std.
    Ok(())
}

#[cfg(test)]
mod tests;
