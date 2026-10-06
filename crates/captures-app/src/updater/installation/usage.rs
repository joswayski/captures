//! A cooperative busy guard, not proof that all processes have stopped.
//! Old hosts and media children do not participate. Keep the operator's stopped-
//! process/launch-exclusion requirement; never enable GUI installation from this.
use super::{Error, File, OperationLock, Paths, Target};
use serde::Deserialize;
use std::{fs, io::Read, path::Path};

/// Retain for the entire packaged host lifetime, independent of its profile.
/// Closing the owned descriptor releases this host's shared lease. It is not
/// inherited by media children and does not supervise them after a host crash.
pub struct PackageUse {
    _lock: File,
}

impl PackageUse {
    /// Recognize the current Python-packaged development layout before any
    /// profile, workers or renderer start. Unpackaged builds acquire no lease.
    /// BUILD_INFO identifies the layout; it is not a signature or admission token.
    pub fn current() -> Result<Option<Self>, Error> {
        let Some(target) = Target::current_host() else {
            return Ok(None);
        };
        Self::for_executable(&std::env::current_exe()?.canonicalize()?, target)
    }

    /// Explicit package-root guard. Profiles and package versions never key it.
    pub fn acquire(package: &Path) -> Result<Self, Error> {
        let lock = Paths::for_package(package)?.use_file()?;
        lock.try_lock_shared()
            .map_err(|_| Error::Installation("the development package is busy"))?;
        Ok(Self { _lock: lock })
    }

    fn for_executable(executable: &Path, target: Target) -> Result<Option<Self>, Error> {
        let relative = Path::new(target.package_executable());
        if !executable.ends_with(relative) {
            return Ok(None);
        }
        let Some(package) = executable.ancestors().nth(relative.components().count()) else {
            return Err(Error::Package);
        };
        let path = package.join("BUILD_INFO.json");
        let metadata = match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            result => result?,
        };
        const LIMIT: u64 = 256 * 1024;
        if !metadata.is_file() || metadata.len() > LIMIT {
            return Err(Error::Package);
        }
        let mut bytes = Vec::new();
        File::open(path)?.take(LIMIT + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > LIMIT {
            return Err(Error::Package);
        }
        #[derive(Deserialize)]
        struct Layout {
            development: bool,
            platform: String,
            binary: String,
        }
        let layout: Layout = serde_json::from_slice(&bytes).map_err(|_| Error::Package)?;
        let platform = match target {
            Target::MacArm64 | Target::MacX64 => "macos",
            Target::WindowsX64 => "windows",
            Target::LinuxX64 => "linux",
        };
        if !layout.development
            || layout.platform != platform
            || layout.binary != target.package_executable()
        {
            return Err(Error::Package);
        }
        Self::acquire(package).map(Some)
    }
}

impl Paths {
    fn use_file(&self) -> Result<File, Error> {
        // Stable sibling inode survives activation/rollback renames. Keep it
        // separate from the operation lock held through replacement startup.
        let path = self.lock.with_extension("use.lock");
        if let Ok(metadata) = fs::symlink_metadata(&path)
            && !metadata.is_file()
        {
            return Err(Error::Installation("invalid development package use guard"));
        }
        Ok(File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?)
    }

    pub(super) fn exclusive_use(&self) -> Result<OperationLock, Error> {
        let lock = self.use_file()?;
        lock.try_lock()
            .map_err(|_| Error::Installation("the development package is in use"))?;
        Ok(OperationLock(lock))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn current_layout_detection_covers_every_target_and_unpackaged_builds() {
        for (target, platform) in [
            (Target::MacArm64, "macos"),
            (Target::MacX64, "macos"),
            (Target::WindowsX64, "windows"),
            (Target::LinuxX64, "linux"),
        ] {
            let root = tempfile::tempdir().unwrap();
            let executable = root.path().join(target.package_executable());
            assert!(
                PackageUse::for_executable(&executable, target)
                    .unwrap()
                    .is_none()
            );
            let metadata = root.path().join("BUILD_INFO.json");
            fs::write(&metadata, json!({"development":true, "platform":platform,
                "binary":target.package_executable(), "binary_sha256":"not an authentication token"}).to_string()).unwrap();
            let lease = PackageUse::for_executable(&executable, target)
                .unwrap()
                .unwrap();
            let paths = Paths::for_package(root.path()).unwrap();
            assert!(paths.exclusive_use().is_err());
            drop(lease);
            assert!(paths.exclusive_use().is_ok());
            assert!(
                PackageUse::for_executable(&root.path().join("checkout-binary"), target)
                    .unwrap()
                    .is_none()
            );
            for value in [
                json!({"development":false, "platform":platform, "binary":target.package_executable()}),
                json!({"development":true, "platform":"other", "binary":target.package_executable()}),
                json!({"development":true, "platform":platform, "binary":"other"}),
            ] {
                fs::write(&metadata, value.to_string()).unwrap();
                assert!(PackageUse::for_executable(&executable, target).is_err());
            }
            fs::write(&metadata, vec![b' '; 256 * 1024 + 1]).unwrap();
            assert!(PackageUse::for_executable(&executable, target).is_err());
        }
    }

    #[test]
    fn exclusive_use_rejects_new_hosts_without_truncating_the_persistent_guard() {
        let root = tempfile::tempdir().unwrap();
        let paths = Paths::for_package(root.path()).unwrap();
        let path = paths.lock.with_extension("use.lock");
        fs::write(&path, b"persistent guard sentinel").unwrap();
        let exclusive = paths.exclusive_use().unwrap();
        assert!(PackageUse::acquire(root.path()).is_err());
        drop(exclusive);
        let lease = PackageUse::acquire(root.path()).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"persistent guard sentinel");
        drop(lease);
        assert_eq!(fs::read(&path).unwrap(), b"persistent guard sentinel");
        let other = tempfile::tempdir().unwrap();
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(PackageUse::acquire(root.path()).is_err());
        assert!(paths.exclusive_use().is_err());
        assert!(PackageUse::acquire(other.path()).is_ok());
    }
}
