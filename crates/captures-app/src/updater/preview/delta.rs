//! Patch updates for the public feed, done the way Caper does them: an update
//! from a recent release downloads a per-file patch (`capturespatch1`, see
//! [`super::super::patch`]) instead of the whole updater artifact.
//!
//! A patch lists every folder, file and (AppImage) link of the new package.
//! Each file is copied from the installed package when unchanged, rebuilt from
//! the installed file with a bsdiff or zstd delta, or carried whole. Comparing
//! uncompressed installed files is what keeps patches small on every platform,
//! even though the full artifacts are compressed containers:
//! - macOS rebuilds `Captures.app` from the installed bundle.
//! - Windows rebuilds the installed folder from the installed files; the helper
//!   then replaces them in place instead of running the NSIS installer.
//! - AppImage rebuilds the AppDir from the running image's mount (`$APPDIR`);
//!   the helper packs it into a new AppImage with the runtime the package
//!   carries ([`super::appimage`]).
//!
//! `latest.json` lists them per platform as `patches: [{from, url, size,
//! signature}]`. The release key signs each patch file under its own name,
//! which binds the target, the new version and the installed version it starts
//! from, and the patch pins every file's size and SHA-256, so the rebuilt
//! package is as trusted as the full artifact. Clients that predate patches
//! ignore the field. A missing or changed installed file, a failed download or
//! a bad signature falls back to the full artifact in the same attempt.
//! Debian packages update manually and never patch.
use super::super::patch;
use super::*;

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
    if !installation.installable() {
        return None;
    }
    let offers: Vec<Offer> = serde_json::from_value(patches.clone()).ok()?;
    if offers.len() > patch::MAX_OFFERS {
        return None;
    }
    offers.into_iter().find(|offer| {
        Version::parse(&offer.from).is_ok_and(|from| from == *current)
            && patch::worthwhile(offer.size, full_size)
            && offer.signature.len() as u64 <= MAX_SIGNATURE_BYTES
            && offer.url().is_ok()
    })
}

/// The signed patch file's name: target, new version and installed version.
pub fn patch_name(target: Target, version: &str, from: &str) -> String {
    format!(
        "Captures-{version}-{}.from-{from}.capturespatch",
        target.as_str()
    )
}

/// The installed package folder a patch reads unchanged and base files from.
pub(super) fn base(installation: &InstallTarget) -> Result<PathBuf, Error> {
    let folder = match installation {
        InstallTarget::Mac { app } => app.clone(),
        InstallTarget::Windows { executable } => {
            executable.parent().ok_or(Error::Package)?.to_owned()
        }
        // The running image's read-only mount; gone once the app quits, so
        // only the app itself rebuilds an AppImage package.
        InstallTarget::AppImage { .. } => {
            PathBuf::from(std::env::var_os("APPDIR").ok_or(Error::Package)?)
        }
        InstallTarget::Manual => return Err(Error::Target),
    };
    if !folder.is_absolute() || !fs::symlink_metadata(&folder)?.is_dir() {
        return Err(Error::Package);
    }
    Ok(folder)
}

/// Rebuild the package a verified patch describes from `base` into `root`
/// (which must not exist). Returns the package folder and its executables.
pub(super) fn rebuild(
    patch_file: &mut fs::File,
    target: Target,
    from: &Version,
    version: &str,
    base: &Path,
    root: PathBuf,
    cancel: &CancelToken,
) -> Result<(PathBuf, std::collections::HashSet<PathBuf>), Error> {
    patch::rebuild(
        patch_file,
        &patch::Expected {
            target,
            base_version: from,
            version,
        },
        base,
        root,
        cancel,
    )
}

/// Release tooling: a patch from one unpacked package folder to another,
/// proven to rebuild the new one exactly as a client does.
pub fn make(
    target: Target,
    base: &Path,
    from: &Version,
    package: &Path,
    version: &Version,
    output: &Path,
) -> Result<patch::Summary, Error> {
    patch::generate_folders(target, base, from, package, version, output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_needs_the_installed_version_an_installable_copy_and_a_real_saving() {
        let offer = |from: &str, size: u64| {
            serde_json::json!({
                "from": from, "url": "https://example.com/p.capturespatch",
                "size": size, "signature": "c2ln"
            })
        };
        let current = Version::parse("2026.11.2109").unwrap();
        let patches = serde_json::json!([offer("2026.11.2108", 10), offer("2026.11.2109", 20)]);
        for installation in [
            InstallTarget::Mac {
                app: "/Applications/Captures.app".into(),
            },
            InstallTarget::Windows {
                executable: "C:\\Captures\\captures.exe".into(),
            },
            InstallTarget::AppImage {
                path: "/home/u/Captures.AppImage".into(),
            },
        ] {
            assert_eq!(
                select(&patches, &current, 100, &installation).map(|offer| offer.size),
                Some(20)
            );
            // Not smaller than half the full download.
            assert!(select(&patches, &current, 40, &installation).is_none());
        }
        // Debian installs update manually.
        assert!(select(&patches, &current, 100, &InstallTarget::Manual).is_none());
        let mac = InstallTarget::Mac {
            app: "/Applications/Captures.app".into(),
        };
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
