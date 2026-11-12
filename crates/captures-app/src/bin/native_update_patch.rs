//! Offline per-file patch generator. Outputs an unsigned descriptor, not a release.
use std::{collections::BTreeMap, fs, io::Read, path::PathBuf};

use captures_app::updater::{Target, patch};
use semver::Version;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const USAGE: &str = "usage: native_update_patch --target aarch64-apple-darwin|x86_64-apple-darwin|x86_64-pc-windows-msvc|x86_64-unknown-linux-gnu --base ABSOLUTE_ARCHIVE_OR_PACKAGE_FOLDER --base-version VERSION --target-archive ABSOLUTE_PATH --version VERSION --patch-url URL --output ABSOLUTE_NEW_FILE\nWrites a per-file patch after rebuilding the target package from the base exactly as a client does, and prints its unsigned descriptor. Add `delta` to the target's artifact in a schema-2 manifest only when `listable` is true, and sign the exact manifest bytes separately; never signs, publishes, overwrites or installs.";

fn main() {
    match run(std::env::args().skip(1)) {
        Ok(metadata) => println!("{metadata}"),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}

fn run(arguments: impl Iterator<Item = String>) -> Result<Value, String> {
    let mut arguments = arguments;
    let mut options = BTreeMap::new();
    while let Some(flag) = arguments.next() {
        if !matches!(
            flag.as_str(),
            "--target"
                | "--base"
                | "--base-version"
                | "--target-archive"
                | "--version"
                | "--patch-url"
                | "--output"
        ) {
            return Err(USAGE.into());
        }
        let value = arguments.next().ok_or(USAGE)?;
        if options.insert(flag, value).is_some() {
            return Err(USAGE.into());
        }
    }
    let get = |flag: &str| options.get(flag).ok_or(USAGE);
    let target: Target =
        serde_json::from_value(json!(get("--target")?)).map_err(|_| USAGE.to_owned())?;
    let base_version = Version::parse(get("--base-version")?).map_err(|error| error.to_string())?;
    let version = Version::parse(get("--version")?).map_err(|error| error.to_string())?;
    let url = get("--patch-url")?;
    let output = PathBuf::from(get("--output")?);
    if !output.is_absolute()
        || output.symlink_metadata().is_ok()
        || !output
            .parent()
            .is_some_and(|p| p.symlink_metadata().is_ok_and(|m| m.is_dir()))
    {
        return Err("Output must be a new absolute file in an existing directory.".into());
    }
    let base = PathBuf::from(get("--base")?);
    if !base.is_absolute()
        || !base
            .symlink_metadata()
            .is_ok_and(|m| m.is_dir() || (m.is_file() && m.len() > 0))
    {
        return Err(
            "The base must be an absolute archive or unpacked package folder, not a link.".into(),
        );
    }
    let archive = PathBuf::from(get("--target-archive")?);
    let (size, sha256) = archive_digest(&archive)?;
    let summary = patch::generate(target, &base, &base_version, &archive, &version, &output)
        .map_err(|error| error.to_string())?;
    Ok(json!({
        "delta": {"format": patch::FORMAT, "patches": [{
            "base_version": base_version.to_string(), "url": url,
            "size": summary.size, "sha256": summary.sha256}]},
        "target": {"version": version.to_string(), "size": size, "sha256": sha256},
        "files": {"copied": summary.copied, "bsdiff": summary.bsdiff, "whole": summary.whole},
        "listable": patch::worthwhile(summary.size, size)
    }))
}

fn archive_digest(path: &PathBuf) -> Result<(u64, String), String> {
    const LIMIT: u64 = 1024 * 1024 * 1024;
    if !path.is_absolute()
        || !path
            .symlink_metadata()
            .is_ok_and(|m| m.is_file() && m.len() > 0 && m.len() <= LIMIT)
    {
        return Err(
            "The target must be an explicit nonempty regular absolute archive, at most 1 GiB, not a link."
                .into(),
        );
    }
    let mut file = fs::File::open(path).map_err(|error| error.to_string())?;
    let mut hasher = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    let mut size = 0;
    loop {
        let count = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        size += count as u64;
        hasher.update(&buffer[..count]);
    }
    Ok((size, format!("{:x}", hasher.finalize())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_arguments_without_writing() {
        let root = tempfile::tempdir().unwrap();
        let archive = root.path().join("archive.tar.gz");
        fs::write(&archive, b"not an archive").unwrap();
        let output = root.path().join("patch");
        let args = |target: &str, base: &str, output: &str| -> Vec<String> {
            [
                "--target",
                target,
                "--base",
                base,
                "--base-version",
                "2026.9.99",
                "--target-archive",
                archive.to_str().unwrap(),
                "--version",
                "2026.10.50",
                "--patch-url",
                "https://example.invalid/patch",
                "--output",
                output,
            ]
            .map(String::from)
            .to_vec()
        };
        let linux = Target::LinuxX64.as_str();
        let base = archive.to_str().unwrap();
        let out = output.to_str().unwrap();
        for invalid in [
            args("linux", base, out),
            args(linux, "relative", out),
            args(linux, base, "relative"),
            args(linux, base, archive.to_str().unwrap()),
            [
                args(linux, base, out),
                vec!["--version".into(), "1.0.0".into()],
            ]
            .concat(),
            // Not a valid package archive: generation fails before writing.
            args(linux, base, out),
        ] {
            assert!(run(invalid.into_iter()).is_err());
        }
        let mut older = args(linux, base, out);
        older[9] = "2026.9.98".into();
        assert!(run(older.into_iter()).is_err());
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
        assert_eq!(fs::read(&archive).unwrap(), b"not an archive");
    }

    #[test]
    #[ignore = "disposable loopback signing fixture invoked by native_delta_smoke.py"]
    fn sign_development_fixture() {
        let root = PathBuf::from(
            std::env::var_os("CAPTURES_NATIVE_DELTA_FIXTURE").expect("explicit fixture directory"),
        );
        assert!(root.is_absolute());
        let bytes = fs::read(root.join("manifest.json")).unwrap();
        let manifest: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            manifest["identity"],
            captures_app::updater::DEVELOPMENT_IDENTITY
        );
        let loopback = |url: &Value| url.as_str().unwrap().starts_with("http://127.0.0.1:");
        for artifact in manifest["artifacts"].as_object().unwrap().values() {
            assert!(loopback(&artifact["url"]));
            for offer in artifact["delta"]["patches"]
                .as_array()
                .into_iter()
                .flatten()
            {
                assert!(loopback(&offer["url"]));
            }
        }
        // Never accepts/persists a private key. Each test creates a fresh keypair.
        let keys = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let signature =
            minisign::sign(None, &keys.sk, std::io::Cursor::new(bytes), None, None).unwrap();
        fs::write(
            root.join("public.key"),
            keys.pk.to_box().unwrap().to_string(),
        )
        .unwrap();
        fs::write(root.join("manifest.json.minisig"), signature.into_string()).unwrap();
    }
}
