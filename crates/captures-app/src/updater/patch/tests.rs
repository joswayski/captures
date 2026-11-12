use super::*;
use crate::updater::staging::tests::package_fixture_with_binary;
use crate::updater::tests::{manifest, serve, signed};
use serde_json::{Value, json};
use std::net::TcpListener;

const FROM: &str = "2026.9.99";
const TO: &str = "2026.10.50";

/// Incompressible bytes, so bsdiff and the half-size rule are meaningful.
fn program(seed: u64, length: usize) -> Vec<u8> {
    let mut state = 0x9e37_79b9_7f4a_7c15_u64 ^ seed;
    (0..length)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn version(text: &str) -> Version {
    Version::parse(text).unwrap()
}

fn write_tree(root: &Path, files: &[(&str, &[u8])], dirs: &[&str]) {
    for dir in dirs {
        fs::create_dir_all(join(root, dir)).unwrap();
    }
    for (path, bytes) in files {
        let path = join(root, path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
}

/// Encode a patch between two unpacked trees, as `generate` does after extraction.
fn encode_trees(base: &Path, target: &Path, executables: &[&str]) -> (Vec<u8>, Summary) {
    let cancel = CancelToken::default();
    let executables: HashSet<_> = executables.iter().map(|path| join(target, path)).collect();
    encode(
        Target::LinuxX64,
        &version(FROM),
        &version(TO),
        target.file_name().unwrap().to_str().unwrap().to_owned(),
        base,
        &tree(base, &HashSet::new(), &cancel).unwrap(),
        target,
        &tree(target, &executables, &cancel).unwrap(),
    )
    .unwrap()
}

fn rebuild_bytes(
    bytes: &[u8],
    base: &Path,
    root: &Path,
) -> Result<(PathBuf, HashSet<PathBuf>), Error> {
    let mut file = tempfile::tempfile().unwrap();
    file.write_all(bytes).unwrap();
    rebuild(
        &mut file,
        &Expected {
            target: Target::LinuxX64,
            base_version: &version(FROM),
            version: TO,
        },
        base,
        root.to_owned(),
        &CancelToken::default(),
    )
}

/// An installed tree and a newer one: changed, unchanged, moved, new and removed
/// files, an execute bit gained, and a new empty folder.
fn asymmetric(root: &Path) -> (PathBuf, PathBuf, Vec<u8>) {
    let old = program(1, 200_000);
    let mut new = old.clone();
    new[1_000..1_040].fill(7);
    new.extend_from_slice(b"one more function");
    let base = root.join("installed").join("native linux");
    write_tree(
        &base,
        &[
            ("captures-native", &old),
            ("LICENSE", b"license"),
            ("docs/README.md", b"read me"),
            ("tool", b"not yet executable"),
            ("removed.txt", b"gone in the update"),
        ],
        &[],
    );
    let target = root.join("target").join("native linux");
    write_tree(
        &target,
        &[
            ("captures-native", &new),
            ("LICENSE", b"license"),
            ("README.md", b"read me"),
            ("tool", b"not yet executable"),
            ("notes/new.txt", b"a brand new file"),
        ],
        &["empty"],
    );
    (base, target, new)
}

#[test]
fn rebuilds_an_asymmetric_tree_exactly_from_the_installed_files() {
    let root = tempfile::tempdir().unwrap();
    let (base, target, new) = asymmetric(root.path());
    let (bytes, summary) = encode_trees(&base, &target, &["captures-native", "tool"]);
    // LICENSE and tool copied in place, README copied from its old path.
    assert_eq!((summary.copied, summary.bsdiff, summary.whole), (3, 1, 1));
    assert!(bytes.len() < 20_000, "{} bytes", bytes.len());
    let (package, executables) = rebuild_bytes(&bytes, &base, &root.path().join("stage")).unwrap();
    assert_eq!(package, root.path().join("stage").join("native linux"));
    assert_eq!(fs::read(package.join("captures-native")).unwrap(), new);
    assert_eq!(fs::read(package.join("README.md")).unwrap(), b"read me");
    assert_eq!(
        fs::read(package.join("notes/new.txt")).unwrap(),
        b"a brand new file"
    );
    assert!(package.join("empty").is_dir());
    assert!(!package.join("removed.txt").exists() && !package.join("docs").exists());
    assert_eq!(
        executables,
        HashSet::from([package.join("captures-native"), package.join("tool")])
    );
    let cancel = CancelToken::default();
    let expected: HashSet<_> = ["captures-native", "tool"]
        .iter()
        .map(|path| join(&target, path))
        .collect();
    assert_eq!(
        tree(&package, &executables, &cancel).unwrap(),
        tree(&target, &expected, &cancel).unwrap()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |path: &str| {
            fs::metadata(package.join(path))
                .unwrap()
                .permissions()
                .mode()
                & 0o777
        };
        assert_eq!((mode("captures-native"), mode("tool")), (0o700, 0o700));
        assert_eq!(mode("LICENSE"), 0o600, "same private modes as full staging");
    }
    // The installed files are only read.
    assert_eq!(
        fs::read(base.join("removed.txt")).unwrap(),
        b"gone in the update"
    );
}

#[test]
fn changed_missing_or_linked_installed_files_are_refused() {
    let root = tempfile::tempdir().unwrap();
    let (base, target, _) = asymmetric(root.path());
    let (bytes, _) = encode_trees(&base, &target, &["captures-native"]);
    // A copied file, a moved copy source and a bsdiff base, one at a time.
    for (index, changed) in ["LICENSE", "docs/README.md", "captures-native"]
        .into_iter()
        .enumerate()
    {
        let path = join(&base, changed);
        let original = fs::read(&path).unwrap();
        let mut edited = original.clone();
        edited[0] ^= 1;
        fs::write(&path, &edited).unwrap();
        let stage = root.path().join(format!("changed-{index}"));
        assert!(rebuild_bytes(&bytes, &base, &stage).is_err(), "{changed}");
        fs::write(&path, &original).unwrap();
        fs::remove_file(&path).unwrap();
        let stage = root.path().join(format!("missing-{index}"));
        assert!(rebuild_bytes(&bytes, &base, &stage).is_err(), "{changed}");
        fs::write(&path, &original).unwrap();
    }
    #[cfg(unix)]
    {
        // A link to the right bytes elsewhere is not an installed base file.
        fs::rename(base.join("LICENSE"), root.path().join("elsewhere")).unwrap();
        std::os::unix::fs::symlink(root.path().join("elsewhere"), base.join("LICENSE")).unwrap();
        assert!(rebuild_bytes(&bytes, &base, &root.path().join("linked")).is_err());
    }
}

fn header_patch(change: impl FnOnce(&mut Value), data: &[u8]) -> Vec<u8> {
    let mut header = json!({
        "schema": 1, "target": Target::LinuxX64.as_str(), "base_version": FROM,
        "version": TO, "root": "native linux",
        "entries": [{"type": "file", "path": "notice", "executable": false,
            "size": 6, "sha256": hex(b"notice"),
            "source": {"kind": "whole", "offset": 0, "length": data.len()}}]
    });
    change(&mut header);
    let json = serde_json::to_vec(&header).unwrap();
    let mut bytes = MAGIC.to_vec();
    bytes.extend_from_slice(&(json.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&json);
    bytes.extend_from_slice(data);
    bytes
}

#[test]
fn header_paths_ranges_and_bounds_are_validated_before_writing() {
    let root = tempfile::tempdir().unwrap();
    let base = root.path().join("base");
    fs::create_dir(&base).unwrap();
    let mut whole = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::best());
    whole.write_all(b"notice").unwrap();
    let data = whole.finish().unwrap();
    let valid = header_patch(|_| {}, &data);
    rebuild_bytes(&valid, &base, &root.path().join("valid")).unwrap();

    let entry = |value: Value| move |header: &mut Value| header["entries"][0] = value;
    let file = |path: &str, source: Value| {
        json!({"type": "file", "path": path, "executable": false, "size": 6,
            "sha256": hex(b"notice"), "source": source})
    };
    let whole_source = json!({"kind": "whole", "offset": 0, "length": data.len()});
    let mut cases: Vec<(&str, Vec<u8>)> = vec![
        ("magic", {
            let mut bytes = valid.clone();
            bytes[0] ^= 1;
            bytes
        }),
        ("header length beyond the file", {
            let mut bytes = valid.clone();
            bytes[15..19].copy_from_slice(&(valid.len() as u32).to_le_bytes());
            bytes
        }),
        ("header length over the limit", {
            let mut bytes = valid.clone();
            bytes[15..19].copy_from_slice(&(MAX_HEADER_BYTES + 1).to_le_bytes());
            bytes
        }),
        ("truncated", valid[..valid.len() - 1].to_vec()),
        ("schema", header_patch(|h| h["schema"] = json!(2), &data)),
        (
            "target",
            header_patch(|h| h["target"] = json!(Target::WindowsX64.as_str()), &data),
        ),
        (
            "base version",
            header_patch(|h| h["base_version"] = json!("2026.9.98"), &data),
        ),
        (
            "version",
            header_patch(|h| h["version"] = json!("2026.10.51"), &data),
        ),
        (
            "root with a folder",
            header_patch(|h| h["root"] = json!("a/b"), &data),
        ),
        (
            "unknown field",
            header_patch(|h| h["extra"] = json!(1), &data),
        ),
        (
            "symlink entry",
            header_patch(
                entry(json!({"type": "symlink", "path": "lib", "target": "notice"})),
                &data,
            ),
        ),
        (
            "data range",
            header_patch(
                entry(file(
                    "notice",
                    json!({"kind": "whole", "offset": 1, "length": data.len()}),
                )),
                &data,
            ),
        ),
        (
            "offset overflow",
            header_patch(
                entry(file(
                    "notice",
                    json!({"kind": "whole", "offset": u64::MAX, "length": 2}),
                )),
                &data,
            ),
        ),
        (
            "declared size",
            header_patch(|h| h["entries"][0]["size"] = json!(5), &data),
        ),
        (
            "hash",
            header_patch(|h| h["entries"][0]["sha256"] = json!(hex(b"other")), &data),
        ),
        (
            "copy base outside",
            header_patch(
                entry(file(
                    "notice",
                    json!({"kind": "copy", "base": "../base/notice"}),
                )),
                &data,
            ),
        ),
        (
            "copy base drive",
            header_patch(
                entry(file("notice", json!({"kind": "copy", "base": "C:notice"}))),
                &data,
            ),
        ),
    ];
    for path in [
        "../escaped",
        "a/../../escaped",
        "/etc/passwd",
        "a\\..\\b",
        "CON",
        "trailing/",
    ] {
        cases.push((
            path,
            header_patch(entry(file(path, whole_source.clone())), &data),
        ));
    }
    cases.push((
        "case-insensitive duplicate",
        header_patch(
            |h| {
                h["entries"] = json!([
                    file("Notice", whole_source.clone()),
                    file("notice", whole_source.clone())
                ])
            },
            &data,
        ),
    ));
    for (index, (name, bytes)) in cases.into_iter().enumerate() {
        let stage = root.path().join(format!("case-{index}"));
        assert!(rebuild_bytes(&bytes, &base, &stage).is_err(), "{name}");
    }
}

#[test]
fn only_worthwhile_patches_from_the_running_version_are_selected() {
    let current = version(FROM);
    let offer = |base: &str, size: u64| {
        json!({"base_version": base, "url": "https://example.invalid/patch",
            "size": size, "sha256": "a".repeat(64)})
    };
    let select = |patches: Value, full: u64| {
        Candidate::select(
            &json!({"format": FORMAT, "patches": patches}),
            &current,
            full,
            false,
        )
        .map(|candidate| candidate.size)
    };
    assert_eq!(select(json!([offer(FROM, 49)]), 100), Some(49));
    assert_eq!(
        select(json!([offer(FROM, 50)]), 100),
        None,
        "half is not smaller"
    );
    assert_eq!(select(json!([offer(FROM, 0)]), 100), None);
    assert_eq!(select(json!([offer("2026.9.98", 10)]), 100), None);
    assert_eq!(
        select(
            json!([offer("2026.9.98", 10), offer(FROM, 60), offer(FROM, 30)]),
            100
        ),
        Some(30)
    );
    let mut insecure = offer(FROM, 10);
    insecure["url"] = json!("http://example.invalid/patch");
    assert_eq!(select(json!([insecure]), 100), None);
    let many: Vec<_> = (0..=MAX_OFFERS).map(|_| offer(FROM, 10)).collect();
    assert_eq!(select(json!(many), 100), None);
    for descriptor in [
        json!({"format": "bsdiff40", "patches": [offer(FROM, 10)]}),
        json!({"format": FORMAT, "patches": [offer(FROM, 10)], "extra": 1}),
        json!({"format": FORMAT}),
        json!(17),
    ] {
        assert!(Candidate::select(&descriptor, &current, 100, false).is_none());
    }
    assert!(worthwhile(1, 3) && !worthwhile(2, 4) && !worthwhile(0, 4));
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
    let cancel = CancelToken::default();
    let mut output = Output {
        inner: Vec::new(),
        hasher: Sha256::new(),
        written: 0,
        remaining: 2,
        cancel: &cancel,
    };
    assert!(output.write_all(b"123").is_err());
    output.write_all(b"12").unwrap();
    cancel.cancel();
    assert!(output.flush().is_err());
    assert_eq!(output.inner, b"12");
}

// Signed acquisition with real packager archives.

struct Bases {
    installed: Option<PathBuf>,
    archive: Option<PathBuf>,
}

struct Release {
    root: tempfile::TempDir,
    kind: Target,
    base: Vec<u8>,
    target: Vec<u8>,
    patch: Vec<u8>,
    new_binary: Vec<u8>,
    installed: PathBuf,
}

impl Release {
    fn new(kind: Target) -> Self {
        let old_binary = program(2, 262_144);
        let mut new_binary = old_binary.clone();
        new_binary[501] ^= 55;
        new_binary[200_013] ^= 17;
        let base = package_fixture_with_binary(kind, "normal", &old_binary);
        let target = package_fixture_with_binary(kind, "normal", &new_binary);
        let root = tempfile::tempdir().unwrap();
        let base_path = root.path().join("base-archive");
        let target_path = root.path().join("target-archive");
        fs::write(&base_path, &base).unwrap();
        fs::write(&target_path, &target).unwrap();
        let output = root.path().join("update.capturespatch");
        let summary = generate(
            kind,
            &base_path,
            &version(FROM),
            &target_path,
            &version(TO),
            &output,
        )
        .unwrap();
        let patch = fs::read(&output).unwrap();
        assert_eq!(summary.size, patch.len() as u64);
        assert_eq!(summary.sha256, hex(&patch));
        assert!(summary.bsdiff >= 1 && summary.whole == 0, "{summary:?}");
        assert!(
            worthwhile(summary.size, target.len() as u64),
            "{kind:?}: {summary:?}"
        );
        // A second run never overwrites.
        assert!(
            generate(
                kind,
                &base_path,
                &version(FROM),
                &target_path,
                &version(TO),
                &output
            )
            .is_err()
        );
        assert_eq!(fs::read(&output).unwrap(), patch);
        fs::remove_file(&output).unwrap();
        let installed = staging::unpack(
            kind,
            &mut File::open(&base_path).unwrap(),
            root.path().join("installed"),
            &CancelToken::default(),
        )
        .unwrap()
        .0;
        Self {
            root,
            kind,
            base,
            target,
            patch,
            new_binary,
            installed,
        }
    }

    /// A pending update from a freshly signed schema-2 manifest offering
    /// `signed_patch`. `change` edits the manifest and the client's bases.
    fn pending(
        &self,
        url: &str,
        signed_patch: &[u8],
        change: impl FnOnce(&mut Value, &mut Bases),
    ) -> PendingUpdate {
        let mut value = manifest(&format!("{url}/full"));
        value["schema"] = json!(2);
        let renderer = if matches!(self.kind, Target::MacArm64 | Target::MacX64) {
            Renderer::Appkit
        } else {
            Renderer::Wgpu
        };
        value["renderer"] = json!(renderer);
        value["artifacts"][self.kind.as_str()] = json!({
            "url": format!("{url}/full"), "size": self.target.len(), "sha256": hex(&self.target),
            "delta": {"format": FORMAT, "patches": [{"base_version": FROM,
                "url": format!("{url}/patch"), "size": signed_patch.len(),
                "sha256": hex(signed_patch)}]}
        });
        let mut bases = Bases {
            installed: Some(self.installed.clone()),
            archive: None,
        };
        change(&mut value, &mut bases);
        let (key, bytes, signature) = signed(&value);
        let mut client =
            UpdateClient::new(&format!("{url}/manifest"), &key, renderer, self.kind, FROM).unwrap();
        client.installed_package = bases.installed;
        client.base_archive = bases.archive;
        client.authenticate(&bytes, &signature).unwrap().unwrap()
    }

    fn scratch(&self) -> tempfile::TempDir {
        tempfile::tempdir_in(self.root.path()).unwrap()
    }
}

fn listen() -> (TcpListener, String) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    (listener, url)
}

fn stage(
    update: &PendingUpdate,
    scratch: &Path,
    cancel: &CancelToken,
    progress: &mut Vec<(u64, u64)>,
) -> Result<StagedUpdate, Error> {
    let mut verified = false;
    let result = update.stage(
        scratch,
        cancel,
        |n, total| progress.push((n, total)),
        || verified = true,
    );
    assert_eq!(verified, result.is_ok());
    result
}

#[test]
fn every_target_stages_from_the_installed_package_without_requesting_the_full_artifact() {
    for kind in [
        Target::MacArm64,
        Target::MacX64,
        Target::WindowsX64,
        Target::LinuxX64,
    ] {
        let release = Release::new(kind);
        let scratch = release.scratch();
        let (listener, url) = listen();
        let update = release.pending(&url, &release.patch, |_, _| {});
        let server = serve(listener, vec![(200, release.patch.clone())]);
        let mut progress = Vec::new();
        let staged = stage(
            &update,
            scratch.path(),
            &CancelToken::default(),
            &mut progress,
        )
        .unwrap();
        assert_eq!(server.join().unwrap(), ["/patch"], "{kind:?}");
        assert_eq!(fs::read(staged.executable()).unwrap(), release.new_binary);
        assert_eq!(staged.info().size, release.target.len() as u64);
        assert_eq!(staged.sha256(), hex(&release.target));
        let patch_size = release.patch.len() as u64;
        assert_eq!(progress.first(), Some(&(0, patch_size)));
        assert_eq!(progress.last(), Some(&(patch_size, patch_size)));
        // Identical to staging the full archive: entries, bytes and modes.
        let full = crate::updater::staging::tests::verified(&release.target, kind, scratch.path())
            .stage(scratch.path(), &CancelToken::default())
            .unwrap();
        let cancel = CancelToken::default();
        let modes = |package: &Path| {
            walk(package, &cancel)
                .unwrap()
                .into_iter()
                .map(|(path, found)| {
                    let hash = (!found.directory).then(|| hash_file(&found.path, &cancel).unwrap());
                    (path, found.directory, found.mode, hash)
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(modes(staged.package()), modes(full.package()), "{kind:?}");
        assert_eq!(staged.package().file_name(), full.package().file_name());
        drop(full);
        // Only the stage remains; the downloaded patch copy was consumed.
        assert_eq!(fs::read_dir(scratch.path()).unwrap().count(), 1);
        drop(staged);
        assert_eq!(fs::read_dir(scratch.path()).unwrap().count(), 0);
        assert_eq!(
            fs::read(release.root.path().join("base-archive")).unwrap(),
            release.base
        );
    }
}

#[test]
fn patch_failures_fall_back_to_the_full_artifact_and_reset_byte_totals() {
    let release = Release::new(Target::LinuxX64);
    for mode in [
        "tampered-patch",
        "changed-installed-file",
        "truncated-signed-patch",
    ] {
        let scratch = release.scratch();
        let (listener, url) = listen();
        let mut delivered = release.patch.clone();
        let signed_patch = match mode {
            "truncated-signed-patch" => {
                // Signed and delivered intact, but its last file's data is cut short.
                delivered.pop();
                delivered.clone()
            }
            _ => release.patch.clone(),
        };
        if mode == "tampered-patch" {
            delivered[40] ^= 64;
        }
        let executable = release
            .installed
            .join(Target::LinuxX64.package_executable());
        let original = fs::read(&executable).unwrap();
        if mode == "changed-installed-file" {
            let mut changed = original.clone();
            changed[7] ^= 1;
            fs::write(&executable, changed).unwrap();
        }
        let update = release.pending(&url, &signed_patch, |_, _| {});
        let server = serve(
            listener,
            vec![(200, delivered), (200, release.target.clone())],
        );
        let mut progress = Vec::new();
        let staged = stage(
            &update,
            scratch.path(),
            &CancelToken::default(),
            &mut progress,
        )
        .unwrap();
        assert_eq!(server.join().unwrap(), ["/patch", "/full"], "{mode}");
        assert_eq!(fs::read(staged.executable()).unwrap(), release.new_binary);
        let full = release.target.len() as u64;
        let reset = progress.iter().position(|p| *p == (0, full)).unwrap();
        assert!(
            reset > 0
                && progress[..reset]
                    .iter()
                    .all(|p| p.1 == signed_patch.len() as u64)
        );
        assert_eq!(progress.last(), Some(&(full, full)));
        drop(staged);
        assert_eq!(fs::read_dir(scratch.path()).unwrap().count(), 0, "{mode}");
        fs::write(&executable, original).unwrap();
    }
}

#[test]
fn unusable_patches_use_only_the_full_artifact() {
    let release = Release::new(Target::LinuxX64);
    let platform = Target::LinuxX64.as_str();
    for mode in [
        "no-saving",
        "other-version",
        "legacy-schema",
        "legacy-bsdiff40",
        "no-installed-package",
        "missing-installed-package",
    ] {
        let scratch = release.scratch();
        let (listener, url) = listen();
        let target_size = release.target.len();
        let update = release.pending(&url, &release.patch, |value, bases| match mode {
            "no-saving" => value["artifacts"][platform]["delta"]["patches"][0]["size"] = json!(target_size.div_ceil(2)),
            "other-version" => value["artifacts"][platform]["delta"]["patches"][0]["base_version"] = json!("2026.9.98"),
            "legacy-schema" => value["schema"] = json!(1),
            "legacy-bsdiff40" => {
                value["artifacts"][platform]["delta"] = json!({"format": "bsdiff40", "base_version": FROM,
                    "base_size": 1, "base_sha256": "a".repeat(64), "url": format!("{url}/patch"),
                    "size": 1, "sha256": "a".repeat(64)})
            }
            "no-installed-package" => bases.installed = None,
            _ => bases.installed = Some(release.root.path().join("not-installed")),
        });
        let server = serve(listener, vec![(200, release.target.clone())]);
        let staged = stage(
            &update,
            scratch.path(),
            &CancelToken::default(),
            &mut Vec::new(),
        )
        .unwrap();
        assert_eq!(server.join().unwrap(), ["/full"], "{mode}");
        assert_eq!(fs::read(staged.executable()).unwrap(), release.new_binary);
    }
}

#[test]
fn a_base_archive_overrides_the_installed_package() {
    let release = Release::new(Target::LinuxX64);
    let scratch = release.scratch();
    let (listener, url) = listen();
    let archive = release.root.path().join("base-archive");
    let update = release.pending(&url, &release.patch, |_, bases| {
        bases.installed = Some(release.root.path().join("not-installed"));
        bases.archive = Some(archive.clone());
    });
    let server = serve(listener, vec![(200, release.patch.clone())]);
    let staged = stage(
        &update,
        scratch.path(),
        &CancelToken::default(),
        &mut Vec::new(),
    )
    .unwrap();
    assert_eq!(server.join().unwrap(), ["/patch"]);
    assert_eq!(fs::read(staged.executable()).unwrap(), release.new_binary);
    // The private unpacked base is removed once the package is rebuilt.
    let stage_root = staged.package().parent().unwrap().parent().unwrap();
    assert!(!stage_root.join("base").exists());
    assert_eq!(fs::read(archive).unwrap(), release.base);
}

#[test]
fn cancelled_patches_never_fall_back_and_failed_patches_never_bypass_full_authentication() {
    let release = Release::new(Target::LinuxX64);
    let scratch = release.scratch();
    let (listener, url) = listen();
    let update = release.pending(&url, &release.patch, |_, _| {});
    let server = serve(listener, vec![(200, release.patch.clone())]);
    let cancel = CancelToken::default();
    let result = update.stage(
        scratch.path(),
        &cancel,
        |n, _| {
            if n > 0 {
                cancel.cancel()
            }
        },
        || {},
    );
    assert!(matches!(result, Err(Error::Cancelled)));
    assert_eq!(server.join().unwrap(), ["/patch"]);
    assert_eq!(fs::read_dir(scratch.path()).unwrap().count(), 0);

    let (listener, url) = listen();
    let update = release.pending(&url, &release.patch, |_, _| {});
    let mut tampered_patch = release.patch.clone();
    tampered_patch[31] ^= 17;
    let mut tampered_full = release.target.clone();
    tampered_full[300] ^= 31;
    let server = serve(listener, vec![(200, tampered_patch), (200, tampered_full)]);
    assert!(matches!(
        update.stage(scratch.path(), &CancelToken::default(), |_, _| {}, || {}),
        Err(Error::Hash)
    ));
    assert_eq!(server.join().unwrap(), ["/patch", "/full"]);
    assert_eq!(fs::read_dir(scratch.path()).unwrap().count(), 0);
}

#[test]
fn patched_stage_replaces_the_installed_package_and_still_rolls_back() {
    let Some(kind) = Target::current_host() else {
        return;
    };
    let release = Release::new(kind);
    let profile = release.root.path().join("profile");
    fs::create_dir(&profile).unwrap();
    fs::write(profile.join("settings"), b"private settings").unwrap();
    let scratch = release.scratch();
    let (listener, url) = listen();
    let update = release.pending(&url, &release.patch, |_, _| {});
    let server = serve(listener, vec![(200, release.patch.clone())]);
    let staged = stage(
        &update,
        scratch.path(),
        &CancelToken::default(),
        &mut Vec::new(),
    )
    .unwrap();
    assert_eq!(server.join().unwrap(), ["/patch"]);
    let old_binary = fs::read(release.installed.join(kind.package_executable())).unwrap();
    // Replacement rebuilds a fresh copy from the still-installed package.
    let pending = staged
        .replace(&release.installed, &CancelToken::default())
        .unwrap();
    let executable = release.installed.join(kind.package_executable());
    assert_eq!(fs::read(&executable).unwrap(), release.new_binary);
    drop(pending);
    assert!(recover_installation(&release.installed).unwrap());
    assert_eq!(fs::read(&executable).unwrap(), old_binary);
    assert_eq!(
        fs::read(profile.join("settings")).unwrap(),
        b"private settings"
    );
}
