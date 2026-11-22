//! Patch updates for the public feed: download a small delta from the installed
//! build instead of the whole updater artifact.
//!
//! A patch is a bzip2 BSDIFF40 delta that rebuilds one signed file from a copy
//! the installed app already has, so the rebuilt bytes pass the same version-
//! and target-bound Minisign check as a full download:
//! - AppImage: from the running `$APPIMAGE` to the new AppImage.
//! - macOS: `.app.tar.gz` is compressed, so the patch rebuilds
//!   `Captures-<version>-<target>.app.tar`, the [`bundle_tar`] of the released
//!   bundle, from the [`bundle_tar`] of the installed app. Release tooling signs
//!   that tar like any artifact; the helper installs it like the `.tar.gz`.
//!
//! Windows NSIS installers are solid-compressed and leave no installed copy, so
//! Windows (and the manual Debian path) always download in full.
//!
//! `latest.json` lists them per platform as `patches: [{from, url, size,
//! signature}]`, where `signature` covers the rebuilt file. Clients that predate
//! patches ignore the field. A missing or changed base, a failed download or a
//! signature mismatch falls back to the full artifact in the same attempt, so a
//! patch can only make an update smaller, never different.
use super::*;
use std::io;

/// One offered patch, from the installed version to the release.
#[derive(Clone, Debug, Deserialize)]
pub(super) struct Offer {
    from: String,
    url: String,
    pub size: u64,
    pub signature: String,
}

impl Offer {
    pub fn url(&self) -> Result<Url, Error> {
        let url = Url::parse(&self.url).map_err(|_| Error::Manifest)?;
        validate_url(&url, cfg!(test) && is_loopback(&url))?;
        Ok(url)
    }
}

/// The patch from `current` this installation can apply, if one is listed and
/// worth it. Malformed `patches` never block the full update.
pub(super) fn select(
    patches: &serde_json::Value,
    current: &Version,
    full_size: u64,
    installation: &InstallTarget,
) -> Option<Offer> {
    if !matches!(
        installation,
        InstallTarget::Mac { .. } | InstallTarget::AppImage { .. }
    ) {
        return None;
    }
    let offers: Vec<Offer> = serde_json::from_value(patches.clone()).ok()?;
    offers.into_iter().find(|offer| {
        Version::parse(&offer.from).is_ok_and(|from| from == *current)
            && offer.size > 0
            && offer.size.saturating_mul(2) < full_size
            && offer.signature.len() as u64 <= MAX_SIGNATURE_BYTES
            && offer.url().is_ok()
    })
}

/// The signed file a patch rebuilds.
pub fn target_name(target: Target, version: &str) -> String {
    match target {
        Target::MacArm64 | Target::MacX64 => {
            format!("Captures-{version}-{}.app.tar", target.as_str())
        }
        _ => artifact_name(target, version),
    }
}

/// The installed copy a patch starts from.
pub(super) fn base(installation: &InstallTarget) -> Result<Vec<u8>, Error> {
    match installation {
        InstallTarget::Mac { app } => bundle_tar(app),
        InstallTarget::AppImage { path } => {
            if fs::metadata(path)?.len() > MAX_ARTIFACT_BYTES {
                return Err(Error::Size);
            }
            Ok(fs::read(path)?)
        }
        _ => Err(Error::Target),
    }
}

/// A deterministic, uncompressed tar of an app bundle: entries sorted by path
/// under the bundle's own folder name, directories and regular files only,
/// modes 755 (directories and executables) or 644, and no owners or times.
/// Release tooling and clients must produce identical bytes from identical
/// bundles, so this never reads anything else from the file system.
pub fn bundle_tar(app: &Path) -> Result<Vec<u8>, Error> {
    let root = app
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(Error::Package)?;
    if !fs::symlink_metadata(app)?.is_dir() {
        return Err(Error::Package);
    }
    let mut entries = vec![(root.to_owned(), app.to_owned())];
    let mut pending = vec![(root.to_owned(), app.to_owned())];
    while let Some((name, directory)) = pending.pop() {
        for item in fs::read_dir(&directory)? {
            let item = item?;
            let child = format!(
                "{name}/{}",
                item.file_name().to_str().ok_or(Error::ArchivePath)?
            );
            if item.file_type()?.is_dir() {
                pending.push((child.clone(), item.path()));
            }
            entries.push((child, item.path()));
        }
    }
    entries.sort();
    let mut archive = tar::Builder::new(Vec::new());
    let mut total = 0u64;
    for (name, path) in entries {
        let metadata = fs::symlink_metadata(&path)?;
        let mut header = tar::Header::new_gnu();
        header.set_mtime(0);
        header.set_uid(0);
        header.set_gid(0);
        if metadata.is_dir() {
            header.set_entry_type(tar::EntryType::Directory);
            header.set_mode(0o755);
            header.set_size(0);
            archive.append_data(&mut header, &name, io::empty())?;
        } else if metadata.is_file() {
            total = total.saturating_add(metadata.len());
            if total > MAX_ARTIFACT_BYTES {
                return Err(Error::Size);
            }
            header.set_entry_type(tar::EntryType::Regular);
            header.set_mode(if executable(&metadata) { 0o755 } else { 0o644 });
            header.set_size(metadata.len());
            archive.append_data(&mut header, &name, fs::File::open(&path)?)?;
        } else {
            // Released bundles hold no links; an installed one that does is
            // not the released bundle and downloads in full instead.
            return Err(Error::Package);
        }
    }
    Ok(archive.into_inner()?)
}

#[cfg(unix)]
fn executable(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn executable(_: &fs::Metadata) -> bool {
    false
}

/// Release tooling: the patch from `base` to `target`, checked to rebuild it.
pub fn make(base: &[u8], target: &[u8]) -> Result<Vec<u8>, Error> {
    let mut patch = Vec::new();
    qbsdiff::Bsdiff::new(base, target)
        .compression_level(9)
        .compare(io::Cursor::new(&mut patch))?;
    if apply(base, &patch, &CancelToken::default())? != target {
        return Err(Error::Archive);
    }
    Ok(patch)
}

/// Rebuild a file from its base. The output is untrusted until the caller
/// verifies its signature; this only bounds the work an unsigned patch causes.
pub(super) fn apply(base: &[u8], patch: &[u8], cancel: &CancelToken) -> Result<Vec<u8>, Error> {
    let size = patch
        .get(24..32)
        .and_then(|bytes| u64::try_from(super::super::patch::integer(bytes)).ok())
        .filter(|size| (1..=MAX_ARTIFACT_BYTES).contains(size))
        .ok_or(Error::Archive)?;
    super::super::patch::validate_controls(patch, base.len() as u64, size, cancel)?;
    let mut output = Vec::with_capacity(size as usize);
    qbsdiff::Bspatch::new(patch)
        .and_then(|patcher| patcher.apply(base, io::Cursor::new(&mut output)))
        .map_err(|_| Error::Archive)?;
    check_cancel(cancel)?;
    if output.len() as u64 != size {
        return Err(Error::Archive);
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(root: &Path, binary: &[u8]) -> PathBuf {
        let app = root.join("Captures.app");
        fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        fs::create_dir_all(app.join("Contents/Resources/notices")).unwrap();
        fs::write(app.join("Contents/Info.plist"), b"<plist/>").unwrap();
        fs::write(app.join("Contents/Resources/notices/NOTICE.md"), b"notice").unwrap();
        fs::write(app.join("Contents/MacOS/captures"), binary).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                app.join("Contents/MacOS/captures"),
                fs::Permissions::from_mode(0o700),
            )
            .unwrap();
        }
        app
    }

    #[test]
    fn bundle_tar_depends_only_on_names_bytes_and_execute_bits() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let a = app(first.path(), b"binary");
        // Same bundle written in another order, at another time.
        let b = second.path().join("Captures.app");
        fs::create_dir_all(b.join("Contents/Resources/notices")).unwrap();
        fs::write(b.join("Contents/Resources/notices/NOTICE.md"), b"notice").unwrap();
        std::thread::sleep(Duration::from_millis(20));
        let b = app(second.path(), b"binary");
        let tar = bundle_tar(&a).unwrap();
        assert_eq!(tar, bundle_tar(&b).unwrap());

        let mut archive = tar::Archive::new(&tar[..]);
        let entries: Vec<_> = archive
            .entries()
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                let header = entry.header();
                (
                    entry.path().unwrap().to_string_lossy().into_owned(),
                    header.mode().unwrap(),
                    header.mtime().unwrap(),
                )
            })
            .collect();
        let names: Vec<_> = entries.iter().map(|(name, ..)| name.as_str()).collect();
        assert_eq!(
            names,
            [
                "Captures.app",
                "Captures.app/Contents",
                "Captures.app/Contents/Info.plist",
                "Captures.app/Contents/MacOS",
                "Captures.app/Contents/MacOS/captures",
                "Captures.app/Contents/Resources",
                "Captures.app/Contents/Resources/notices",
                "Captures.app/Contents/Resources/notices/NOTICE.md",
            ]
        );
        assert!(entries.iter().all(|(.., mtime)| *mtime == 0));
        #[cfg(unix)]
        assert_eq!(entries[4].1, 0o755);
        assert_eq!(entries[2].1, 0o644);

        fs::write(b.join("Contents/MacOS/captures"), b"binarY").unwrap();
        assert_ne!(tar, bundle_tar(&b).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn bundle_tar_refuses_links() {
        let root = tempfile::tempdir().unwrap();
        let app = app(root.path(), b"binary");
        std::os::unix::fs::symlink("/etc/passwd", app.join("Contents/link")).unwrap();
        assert!(matches!(bundle_tar(&app), Err(Error::Package)));
    }

    #[test]
    fn patch_rebuilds_only_from_its_own_base() {
        let base: Vec<u8> = (0..200_000u32).map(|n| (n * 7 % 251) as u8).collect();
        let mut target = base.clone();
        target[1000..1010].copy_from_slice(b"new code!!");
        target.extend_from_slice(b"appended resources");
        let patch = make(&base, &target).unwrap();
        assert!(patch.len() < target.len() / 10, "{}", patch.len());
        let cancel = CancelToken::default();
        assert_eq!(apply(&base, &patch, &cancel).unwrap(), target);
        // A different base rebuilds something else, which the signature then
        // rejects; it must not panic or exceed the target size.
        let mut other = base.clone();
        other[50_000] ^= 0xff;
        assert_ne!(apply(&other, &patch, &cancel).unwrap(), target);
        assert!(apply(&base[..1000], &patch, &cancel).is_err());
        assert!(apply(&base, &patch[..patch.len() / 2], &cancel).is_err());
        assert!(apply(&base, b"not a patch", &cancel).is_err());
    }

    #[test]
    fn select_needs_the_installed_version_a_base_and_a_real_saving() {
        let offer = |from: &str, size: u64| {
            serde_json::json!({
                "from": from, "url": "https://example.com/p.patch",
                "size": size, "signature": "c2ln"
            })
        };
        let current = Version::parse("2026.11.2109").unwrap();
        let mac = InstallTarget::Mac {
            app: "/Applications/Captures.app".into(),
        };
        let patches = serde_json::json!([offer("2026.11.2108", 10), offer("2026.11.2109", 20)]);
        assert_eq!(
            select(&patches, &current, 100, &mac).map(|offer| offer.size),
            Some(20)
        );
        // Not smaller than half the full download.
        assert!(select(&patches, &current, 40, &mac).is_none());
        // Windows and Debian have no installed copy to patch.
        for installation in [
            InstallTarget::Windows {
                executable: "C:\\Captures\\captures.exe".into(),
            },
            InstallTarget::Manual,
        ] {
            assert!(select(&patches, &current, 100, &installation).is_none());
        }
        // Malformed or absent patches never block the update itself.
        for patches in [
            serde_json::Value::Null,
            serde_json::json!({"from": "2026.11.2109"}),
            serde_json::json!([{"from": "2026.11.2109", "url": "http://example.com/p", "size": 1, "signature": "x"}]),
        ] {
            assert!(select(&patches, &current, 100, &mac).is_none());
        }
    }
}
