//! Offline BSDIFF40 fixture/packaging tool. Outputs unsigned metadata, not a release.
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    path::PathBuf,
};

use semver::Version;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const USAGE: &str = "usage: native_update_patch --base-archive ABSOLUTE_PATH --target-archive ABSOLUTE_PATH --base-version VERSION --patch-url URL --output ABSOLUTE_NEW_FILE\nPrints unsigned delta/target metadata. Add the delta to a schema-2 manifest and sign its exact bytes separately; never publishes or installs.";

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
            "--base-archive" | "--target-archive" | "--base-version" | "--patch-url" | "--output"
        ) {
            return Err(USAGE.into());
        }
        let value = arguments.next().ok_or(USAGE)?;
        if options.insert(flag, value).is_some() {
            return Err(USAGE.into());
        }
    }
    let get = |flag: &str| options.get(flag).ok_or(USAGE);
    let version = Version::parse(get("--base-version")?).map_err(|error| error.to_string())?;
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
    let base = input(&PathBuf::from(get("--base-archive")?))?;
    let target = input(&PathBuf::from(get("--target-archive")?))?;
    let mut patch = Vec::new();
    qbsdiff::Bsdiff::new(&base, &target)
        .compare(&mut patch)
        .map_err(|error| error.to_string())?;
    let mut reconstructed = Vec::new();
    qbsdiff::Bspatch::new(&patch)
        .and_then(|p| p.apply(&base, &mut reconstructed))
        .map_err(|error| error.to_string())?;
    if reconstructed != target {
        return Err("Generated patch did not reconstruct the exact target.".into());
    }
    let mut file = tempfile::NamedTempFile::new_in(output.parent().unwrap())
        .map_err(|error| error.to_string())?;
    file.write_all(&patch)
        .and_then(|_| file.as_file().sync_all())
        .map_err(|error| error.to_string())?;
    file.persist_noclobber(output)
        .map_err(|error| error.to_string())?;
    Ok(json!({
        "delta":{"format":"bsdiff40", "base_version":version.to_string(),
            "base_size":base.len(), "base_sha256":format!("{:x}", Sha256::digest(&base)),
            "url":url, "size":patch.len(), "sha256":format!("{:x}", Sha256::digest(&patch))},
        "target":{"size":target.len(), "sha256":format!("{:x}", Sha256::digest(&target))},
        "smaller":patch.len() < target.len()
    }))
}

fn input(path: &PathBuf) -> Result<Vec<u8>, String> {
    const LIMIT: u64 = 1024 * 1024 * 1024;
    if !path.is_absolute()
        || !path
            .symlink_metadata()
            .is_ok_and(|m| m.is_file() && m.len() > 0 && m.len() <= LIMIT)
    {
        return Err(
            "Inputs must be explicit nonempty regular absolute archives, at most 1 GiB, not links."
                .into(),
        );
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(|error| error.to_string())?
        .take(LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.is_empty() || bytes.len() as u64 > LIMIT {
        return Err("Input archive size is invalid.".into());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn offline_tool_round_trips_metadata_preserves_inputs_and_never_overwrites() {
        let root = tempfile::tempdir().unwrap();
        let base: Vec<_> = (0..131_072)
            .map(|i| (i * 17 + (i >> 9) * 31) as u8)
            .collect();
        let mut target = base[177..].to_vec();
        target[501] ^= 55;
        target.extend_from_slice(b"a different ending");
        let base_path = root.path().join("base");
        let target_path = root.path().join("target");
        let output = root.path().join("patch");
        fs::write(&base_path, &base).unwrap();
        fs::write(&target_path, &target).unwrap();
        let args: Vec<String> = vec![
            "--base-archive".into(),
            base_path.display().to_string(),
            "--target-archive".into(),
            target_path.display().to_string(),
            "--base-version".into(),
            "2026.9.99".into(),
            "--patch-url".into(),
            "https://example.invalid/patch".into(),
            "--output".into(),
            output.display().to_string(),
        ];
        let metadata = run(args.clone().into_iter()).unwrap();
        let bytes = fs::read(&output).unwrap();
        let mut actual = Vec::new();
        qbsdiff::Bspatch::new(&bytes)
            .unwrap()
            .apply(&base, &mut actual)
            .unwrap();
        assert_eq!(actual, target);
        assert_eq!(metadata["delta"]["base_size"], base.len());
        assert_eq!(metadata["target"]["size"], target.len());
        assert_eq!(metadata["delta"]["size"], bytes.len());
        assert_eq!(metadata["smaller"], true);
        assert!(run(args.clone().into_iter()).is_err());
        assert_eq!(fs::read(&output).unwrap(), bytes);
        assert_eq!(fs::read(&base_path).unwrap(), base);
        assert_eq!(fs::read(&target_path).unwrap(), target);
        fs::remove_file(output).unwrap();
        let mut invalid = args.clone();
        invalid[1] = "relative".into();
        assert!(run(invalid.into_iter()).is_err());
        let mut invalid = args;
        invalid.extend(["--base-version".into(), "2026.9.99".into()]);
        assert!(run(invalid.into_iter()).is_err());
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 2);
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
        for artifact in manifest["artifacts"].as_object().unwrap().values() {
            assert!(
                artifact["url"]
                    .as_str()
                    .unwrap()
                    .starts_with("http://127.0.0.1:")
            );
            if let Some(delta) = artifact.get("delta") {
                assert!(
                    delta["url"]
                        .as_str()
                        .unwrap()
                        .starts_with("http://127.0.0.1:")
                );
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
