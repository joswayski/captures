//! Authenticated BSDIFF40 acquisition; the complete target is verified as usual.
use super::*;
use bzip2::read::BzDecoder;
use std::{fs::File, io};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Description {
    format: String,
    base_version: String,
    base_size: u64,
    base_sha256: String,
    url: String,
    size: u64,
    sha256: String,
}

pub(super) struct Candidate {
    base_size: u64,
    base_hash: [u8; 32],
    url: Url,
    size: u64,
    hash: [u8; 32],
}

impl Candidate {
    pub(super) fn new(
        value: &serde_json::Value,
        current: &Version,
        target_size: u64,
        loopback: bool,
    ) -> Result<Self, Error> {
        let description: Description =
            serde_json::from_value(value.clone()).map_err(|_| Error::Manifest)?;
        if description.format != "bsdiff40"
            || description.base_version != current.to_string()
            || description.base_size == 0
            || description.base_size > MAX_ARTIFACT_BYTES
            || description.size == 0
            || description.size >= target_size
        {
            return Err(Error::Manifest);
        }
        let url = Url::parse(&description.url).map_err(|_| Error::Manifest)?;
        validate_url(&url, loopback)?;
        Ok(Self {
            base_size: description.base_size,
            base_hash: decode_hash(&description.base_sha256)?,
            url,
            size: description.size,
            hash: decode_hash(&description.sha256)?,
        })
    }

    pub(super) fn download(
        &self,
        update: &PendingUpdate,
        base: &Path,
        directory: &Path,
        cancel: &CancelToken,
        progress: &mut impl FnMut(u64, u64),
    ) -> Result<VerifiedUpdate, Error> {
        check_cancel(cancel)?;
        if !base.symlink_metadata()?.is_file() {
            return Err(Error::Archive);
        }
        // Own a hashed snapshot: patching cannot mutate or race the base input.
        let base = read_verified(File::open(base)?, self.base_size, self.base_hash, cancel)?;
        check_cancel(cancel)?;
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
        let mut patch_info = update.info.clone();
        patch_info.size = self.size;
        let mut patch = verify_file(
            response,
            directory,
            &patch_info,
            self.hash,
            cancel,
            progress,
        )?;
        let bytes = read_verified(patch.file.as_file_mut(), self.size, self.hash, cancel)?;
        validate_controls(&bytes, base.len() as u64, update.info.size, cancel)?;

        let mut target = NamedTempFile::new_in(directory)?;
        let result = qbsdiff::Bspatch::new(&bytes).and_then(|patcher| {
            patcher.apply(
                &base,
                TargetWriter {
                    file: target.as_file_mut(),
                    cancel,
                    remaining: update.info.size,
                },
            )
        });
        check_cancel(cancel)?;
        result.map_err(|_| Error::Archive)?;
        target.seek(SeekFrom::Start(0))?;
        // No patch output becomes an update until the full signed size/hash pass.
        // Stage and replacement still rehash their own copies, as for full downloads.
        verify_file(
            target.as_file_mut(),
            directory,
            &update.info,
            update.hash,
            cancel,
            |_, _| {},
        )
    }
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
        check_cancel(cancel)?;
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

// qbsdiff's size is a hint, not a bound. Validate signed header lengths before
// calling it (negative lengths otherwise cast to u64), and bound control count
// and output before decoding. A leading zero-output source seek is legitimate.
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

struct TargetWriter<'a> {
    file: &'a mut File,
    cancel: &'a CancelToken,
    remaining: u64,
}

impl Write for TargetWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.cancel.is_cancelled() || bytes.len() as u64 > self.remaining {
            return Err(io::Error::other("cancelled or oversized delta output"));
        }
        let count = self.file.write(bytes)?;
        self.remaining -= count as u64;
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.cancel.is_cancelled() {
            return Err(io::Error::other("delta cancelled"));
        }
        self.file.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::updater::tests::{manifest, serve, signed};
    use serde_json::{Value, json};
    use std::{fs, net::TcpListener};

    fn contents() -> (Vec<u8>, Vec<u8>) {
        let base: Vec<_> = (0..262_144)
            .map(|i| (i * 73 + (i >> 8) * 29) as u8)
            .collect();
        let mut target = base[400..210_000].to_vec();
        target[137] ^= 0x7a;
        target[19_800..19_913].fill(17);
        target.extend_from_slice(b"new unequal tail\0\xff");
        (base, target)
    }

    fn hash(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    fn patch(base: &[u8], target: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        qbsdiff::Bsdiff::new(base, target)
            .compare(&mut bytes)
            .unwrap();
        bytes
    }

    fn pending(
        url: &str,
        base_path: &Path,
        base: &[u8],
        patch: &[u8],
        target: &[u8],
        change: impl FnOnce(&mut Value),
    ) -> PendingUpdate {
        pending_for(
            Target::LinuxX64,
            url,
            base_path,
            base,
            patch,
            target,
            change,
        )
    }

    fn pending_for(
        kind: Target,
        url: &str,
        base_path: &Path,
        base: &[u8],
        patch: &[u8],
        target: &[u8],
        change: impl FnOnce(&mut Value),
    ) -> PendingUpdate {
        let mut value = manifest(&format!("{url}/full"));
        value["schema"] = json!(2);
        let renderer = if matches!(kind, Target::MacArm64 | Target::MacX64) {
            Renderer::Appkit
        } else {
            Renderer::Wgpu
        };
        value["renderer"] = json!(renderer);
        value["artifacts"][kind.as_str()] = json!({
            "url":format!("{url}/full"), "size":target.len(), "sha256":hash(target),
            "delta": {"format":"bsdiff40", "base_version":"2026.9.99",
                "base_size":base.len(), "base_sha256":hash(base),
                "url":format!("{url}/patch"), "size":patch.len(), "sha256":hash(patch)}
        });
        change(&mut value);
        let (key, bytes, signature) = signed(&value);
        UpdateClient::new(
            &format!("{url}/manifest"),
            &key,
            renderer,
            kind,
            "2026.9.99",
        )
        .unwrap()
        .with_base_archive(base_path.to_owned())
        .unwrap()
        .authenticate(&bytes, &signature)
        .unwrap()
        .unwrap()
    }

    #[test]
    fn reconstructs_exact_signed_target_without_full_request_and_preserves_base() {
        let (base, target) = contents();
        let patch = patch(&base, &target);
        assert!(patch.len() < target.len());
        let root = tempfile::tempdir().unwrap();
        let base_path = root.path().join("base");
        fs::write(&base_path, &base).unwrap();
        let scratch = tempfile::tempdir_in(root.path()).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let update = pending(&url, &base_path, &base, &patch, &target, |_| {});
        let server = serve(listener, vec![(200, patch.clone())]);
        let mut progress = Vec::new();
        let verified = update
            .download(scratch.path(), &CancelToken::default(), |n, total| {
                progress.push((n, total))
            })
            .unwrap();
        let mut actual = Vec::new();
        verified.file().read_to_end(&mut actual).unwrap();
        assert_eq!(actual, target);
        assert_eq!(verified.info().size, target.len() as u64);
        assert_eq!(server.join().unwrap(), ["/patch"]);
        assert_eq!(progress.first(), Some(&(0, patch.len() as u64)));
        assert_eq!(
            progress.last(),
            Some(&(patch.len() as u64, patch.len() as u64))
        );
        assert_eq!(fs::read(&base_path).unwrap(), base);
        assert_eq!(fs::read_dir(scratch.path()).unwrap().count(), 1);
        drop(verified);
        assert_eq!(fs::read_dir(scratch.path()).unwrap().count(), 0);
    }

    #[test]
    fn missing_wrong_and_nonregular_bases_use_full_without_requesting_a_patch() {
        let (base, target) = contents();
        let patch = patch(&base, &target);
        for mode in ["missing", "wrong", "directory"] {
            let root = tempfile::tempdir().unwrap();
            let base_path = root.path().join("base");
            if mode == "wrong" {
                let mut wrong = base.clone();
                wrong[411] ^= 17;
                fs::write(&base_path, wrong).unwrap();
            } else if mode == "directory" {
                fs::create_dir(&base_path).unwrap();
            }
            let before = fs::read(&base_path).ok();
            let scratch = tempfile::tempdir_in(root.path()).unwrap();
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let update = pending(&url, &base_path, &base, &patch, &target, |_| {});
            let server = serve(listener, vec![(200, target.clone())]);
            let verified = update
                .download(scratch.path(), &CancelToken::default(), |_, _| {})
                .unwrap();
            let mut actual = Vec::new();
            verified.file().read_to_end(&mut actual).unwrap();
            assert_eq!(actual, target, "{mode}");
            assert_eq!(server.join().unwrap(), ["/full"]);
            assert_eq!(fs::read(&base_path).ok(), before);
            drop(verified);
            assert_eq!(fs::read_dir(scratch.path()).unwrap().count(), 0);
        }
    }

    #[test]
    fn tampered_corrupt_and_wrong_target_patches_fall_back_and_reset_byte_totals() {
        let (base, target) = contents();
        let good_patch = patch(&base, &target);
        let mut altered_target = target.clone();
        altered_target[311] ^= 55;
        let wrong_target_patch = patch(&base, &altered_target);
        for mode in ["tampered", "corrupt", "wrong-target", "negative-header"] {
            let root = tempfile::tempdir().unwrap();
            let base_path = root.path().join("base");
            fs::write(&base_path, &base).unwrap();
            let scratch = tempfile::tempdir_in(root.path()).unwrap();
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let mut delivered = match mode {
                "corrupt" => vec![17; 44],
                "wrong-target" => wrong_target_patch.clone(),
                _ => good_patch.clone(),
            };
            if mode == "negative-header" {
                delivered[15] |= 128;
            }
            let signed_patch = if mode == "tampered" {
                &good_patch
            } else {
                &delivered
            };
            let update = pending(&url, &base_path, &base, signed_patch, &target, |_| {});
            if mode == "tampered" {
                let last = delivered.len() - 1;
                delivered[last] ^= 64;
            }
            let server = serve(listener, vec![(200, delivered), (200, target.clone())]);
            let mut progress = Vec::new();
            let verified = update
                .download(scratch.path(), &CancelToken::default(), |n, total| {
                    progress.push((n, total))
                })
                .unwrap();
            let mut actual = Vec::new();
            verified.file().read_to_end(&mut actual).unwrap();
            assert_eq!(actual, target, "{mode}");
            assert_eq!(server.join().unwrap(), ["/patch", "/full"]);
            let reset = progress
                .iter()
                .position(|p| *p == (0, target.len() as u64))
                .unwrap();
            assert!(reset > 0, "patch progress must precede reset");
            assert_eq!(
                progress.last(),
                Some(&(target.len() as u64, target.len() as u64))
            );
            assert_eq!(fs::read(&base_path).unwrap(), base);
            drop(verified);
            assert_eq!(fs::read_dir(scratch.path()).unwrap().count(), 0);
        }
    }

    #[test]
    fn incompatible_metadata_and_no_size_saving_use_full_only() {
        let (base, target) = contents();
        let patch = patch(&base, &target);
        for (field, value) in [
            ("format", json!("unsupported")),
            ("base_version", json!("2026.9.98")),
            ("base_sha256", json!("invalid")),
            ("size", json!(target.len())),
            ("size", json!(target.len() + 1)),
            ("base_size", json!(MAX_ARTIFACT_BYTES + 1)),
            ("url", json!("http://example.invalid/patch")),
        ] {
            let root = tempfile::tempdir().unwrap();
            let base_path = root.path().join("base");
            fs::write(&base_path, &base).unwrap();
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let update = pending(&url, &base_path, &base, &patch, &target, |m| {
                m["artifacts"][Target::LinuxX64.as_str()]["delta"][field] = value
            });
            let server = serve(listener, vec![(200, target.clone())]);
            let verified = update
                .download(root.path(), &CancelToken::default(), |_, _| {})
                .unwrap();
            let mut actual = Vec::new();
            verified.file().read_to_end(&mut actual).unwrap();
            assert_eq!(actual, target);
            assert_eq!(server.join().unwrap(), ["/full"]);
        }
    }

    #[test]
    fn cancelled_patch_never_falls_back_and_partial_storage_is_removed() {
        let (base, target) = contents();
        let patch = patch(&base, &target);
        let root = tempfile::tempdir().unwrap();
        let base_path = root.path().join("base");
        fs::write(&base_path, &base).unwrap();
        let scratch = tempfile::tempdir_in(root.path()).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let update = pending(&url, &base_path, &base, &patch, &target, |_| {});
        let server = serve(listener, vec![(200, patch)]);
        let cancel = CancelToken::default();
        assert!(matches!(
            update.download(scratch.path(), &cancel, |n, _| {
                if n > 0 {
                    cancel.cancel();
                }
            }),
            Err(Error::Cancelled)
        ));
        assert_eq!(server.join().unwrap(), ["/patch"]);
        assert_eq!(fs::read_dir(scratch.path()).unwrap().count(), 0);
        assert_eq!(fs::read(&base_path).unwrap(), base);
    }

    #[test]
    fn legacy_absent_and_unparseable_delta_descriptions_do_not_disable_full_acquisition() {
        let (base, target) = contents();
        let patch = patch(&base, &target);
        for mode in ["legacy", "absent", "number", "missing-fields"] {
            let root = tempfile::tempdir().unwrap();
            let base_path = root.path().join("base");
            fs::write(&base_path, &base).unwrap();
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let update = pending(&url, &base_path, &base, &patch, &target, |m| match mode {
                "legacy" => m["schema"] = json!(1),
                "absent" => {
                    m["artifacts"][Target::LinuxX64.as_str()]
                        .as_object_mut()
                        .unwrap()
                        .remove("delta");
                }
                "number" => m["artifacts"][Target::LinuxX64.as_str()]["delta"] = json!(17),
                _ => {
                    m["artifacts"][Target::LinuxX64.as_str()]["delta"] =
                        json!({"format":"bsdiff40"})
                }
            });
            let server = serve(listener, vec![(200, target.clone())]);
            let verified = update
                .download(root.path(), &CancelToken::default(), |_, _| {})
                .unwrap();
            let mut actual = Vec::new();
            verified.file().read_to_end(&mut actual).unwrap();
            assert_eq!(actual, target);
            assert_eq!(server.join().unwrap(), ["/full"]);
        }
    }

    #[test]
    fn failed_patch_does_not_bypass_authentication_of_the_full_fallback() {
        let (base, target) = contents();
        let patch = patch(&base, &target);
        let root = tempfile::tempdir().unwrap();
        let base_path = root.path().join("base");
        fs::write(&base_path, &base).unwrap();
        let scratch = tempfile::tempdir_in(root.path()).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let update = pending(&url, &base_path, &base, &patch, &target, |_| {});
        let mut tampered_patch = patch;
        tampered_patch[31] ^= 17;
        let mut tampered_full = target;
        tampered_full[300] ^= 31;
        let server = serve(listener, vec![(200, tampered_patch), (200, tampered_full)]);
        assert!(matches!(
            update.download(scratch.path(), &CancelToken::default(), |_, _| {}),
            Err(Error::Hash)
        ));
        assert_eq!(server.join().unwrap(), ["/patch", "/full"]);
        assert_eq!(fs::read_dir(scratch.path()).unwrap().count(), 0);
        assert_eq!(fs::read(base_path).unwrap(), base);
    }

    #[test]
    fn reconstructed_portable_packages_stage_and_host_replacement_still_rolls_back() {
        use crate::updater::staging::tests::{package_fixture_with_binary, verified};
        for kind in [
            Target::MacArm64,
            Target::MacX64,
            Target::WindowsX64,
            Target::LinuxX64,
        ] {
            // Incompressible unchanged bytes make this a reliable patch-saving
            // fixture even if tar/ZIP timestamps differ during a busy full suite.
            let mut state = 0x9e37_79b9_7f4a_7c15_u64;
            let old_binary: Vec<_> = (0..262_144)
                .map(|_| {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    state as u8
                })
                .collect();
            let mut new_binary = old_binary.clone();
            new_binary[501] ^= 55;
            new_binary[200_013] ^= 17;
            let base = package_fixture_with_binary(kind, "normal", &old_binary);
            let target = package_fixture_with_binary(kind, "normal", &new_binary);
            let patch = patch(&base, &target);
            assert!(
                patch.len() < target.len(),
                "{kind:?}: patch {}, full {}",
                patch.len(),
                target.len()
            );
            let root = tempfile::tempdir().unwrap();
            let base_path = root.path().join("base");
            fs::write(&base_path, &base).unwrap();
            let profile = root.path().join("profile");
            fs::create_dir(&profile).unwrap();
            fs::write(profile.join("settings"), b"private settings").unwrap();
            fs::write(profile.join("history"), b"private captures").unwrap();
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let update = pending_for(kind, &url, &base_path, &base, &patch, &target, |_| {});
            let server = serve(listener, vec![(200, patch)]);
            let staged = update
                .download(root.path(), &CancelToken::default(), |_, _| {})
                .unwrap()
                .stage(root.path(), &CancelToken::default())
                .unwrap();
            assert_eq!(fs::read(staged.executable()).unwrap(), new_binary);
            assert_eq!(server.join().unwrap(), ["/patch"]);
            if Target::current_host() == Some(kind) {
                let previous = verified(&base, kind, root.path())
                    .stage(root.path(), &CancelToken::default())
                    .unwrap();
                let executable = previous
                    .executable()
                    .strip_prefix(previous.package())
                    .unwrap()
                    .to_owned();
                let destination = root.path().join("stopped-development-package");
                fs::rename(previous.package(), &destination).unwrap();
                drop(previous);
                let pending = staged
                    .replace(&destination, &CancelToken::default())
                    .unwrap();
                assert_eq!(fs::read(destination.join(&executable)).unwrap(), new_binary);
                drop(pending);
                assert!(recover_installation(&destination).unwrap());
                assert_eq!(fs::read(destination.join(executable)).unwrap(), old_binary);
            }
            assert_eq!(fs::read(base_path).unwrap(), base);
            assert_eq!(
                fs::read(profile.join("settings")).unwrap(),
                b"private settings"
            );
            assert_eq!(
                fs::read(profile.join("history")).unwrap(),
                b"private captures"
            );
        }
    }

    #[test]
    fn control_validation_rejects_zero_output_overflow_negative_and_oversized_records() {
        fn encode(n: i64) -> [u8; 8] {
            let mut bytes = n.unsigned_abs().to_le_bytes();
            if n < 0 {
                bytes[7] |= 128;
            }
            bytes
        }
        for records in [
            vec![[0, 0, 0]],
            vec![[-1, 2, 0]],
            vec![[2, 0, 0]],
            vec![[0, 1, -1]],
            vec![[i64::MAX, 1, 0]],
            vec![[0, 0, 0], [0, 0, 0], [0, 0, 0], [0, 1, 0]],
        ] {
            let mut compressor =
                bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::default());
            for record in &records {
                for n in *record {
                    compressor.write_all(&encode(n)).unwrap();
                }
            }
            let controls = compressor.finish().unwrap();
            let mut bytes = b"BSDIFF40".to_vec();
            bytes.extend_from_slice(&encode(controls.len() as i64));
            bytes.extend_from_slice(&encode(0));
            bytes.extend_from_slice(&encode(1));
            bytes.extend_from_slice(&controls);
            assert!(
                validate_controls(&bytes, 1, 1, &CancelToken::default()).is_err(),
                "{records:?}"
            );
        }
        let mut bomb = b"BSDIFF40".to_vec();
        for _ in 0..2 {
            bomb.extend_from_slice(&i64::MAX.to_le_bytes());
        }
        bomb.extend_from_slice(&1_i64.to_le_bytes());
        assert!(validate_controls(&bomb, 1, 1, &CancelToken::default()).is_err());
        let mut file = tempfile::tempfile().unwrap();
        let cancel = CancelToken::default();
        let mut writer = TargetWriter {
            file: &mut file,
            cancel: &cancel,
            remaining: 2,
        };
        assert!(writer.write_all(b"123").is_err());
        writer.write_all(b"12").unwrap();
        cancel.cancel();
        assert!(writer.flush().is_err());
        assert_eq!(file.metadata().unwrap().len(), 2);
    }
}
