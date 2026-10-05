use super::super::{Renderer, UpdateClient};
use super::*;
use serde_json::json;
use std::process::Command;

const EXECUTABLE: &[u8] = b"asymmetric native bytes\0\xff";

// Use the real Python packager, not the extraction code, for layout/hash fixtures.
// The media bodies are inert test bytes, not playable/runnable distribution files.
fn package_fixture(target: Target, scenario: &str) -> Vec<u8> {
    let root = tempfile::tempdir().unwrap();
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let python = if cfg!(windows) { "python" } else { "python3" };
    let result = Command::new(python).args(["-c", r#"
import importlib.util, io, json, pathlib, plistlib, sys, tarfile, zipfile
repo, root = map(pathlib.Path, sys.argv[1:3])
target, scenario = sys.argv[3:5]
platform = 'macos' if 'apple' in target else 'windows' if 'windows' in target else 'linux'
spec = importlib.util.spec_from_file_location('package', repo / 'apps/native/package.py')
package = importlib.util.module_from_spec(spec); spec.loader.exec_module(package)
binary = root / 'binary'; binary.write_bytes(b'asymmetric native bytes\x00\xff')
resources = root / 'resources'; resources.mkdir(); (resources / 'tokens.json').write_text('{}')
output = root / ('native é ' + platform)
_, executable = package.stage(platform, binary, output, resources)
suffix = '.exe' if platform == 'windows' else ''
for name in ('ffmpeg', 'ffprobe'):
    path = executable.parent / 'binaries' / (name + '-' + target + suffix)
    path.parent.mkdir(exist_ok=True); path.write_bytes(b'inert tool'); path.chmod(0o755)
resource_root = output / 'Captures Native Development.app/Contents/Resources' if platform == 'macos' else output
licenses = resource_root / 'media-licenses'
for name in ('LICENSE', 'NOTICE.md'):
    path = licenses / 'openh264' / name; path.parent.mkdir(parents=True, exist_ok=True); path.write_text('fixture notice')
for ending in ('.tar.xz', '.tar.xz.asc', '-BUILD_CONFIG.txt', '-COPYING.LGPLv2.1', '-NOTICE.md'):
    path = licenses / 'ffmpeg' / ('ffmpeg-8.1' + ending); path.parent.mkdir(exist_ok=True); path.write_bytes(b'fixture source or notice')
if scenario == 'missing-sidecar': next((executable.parent / 'binaries').glob('ffprobe*')).unlink()
if scenario == 'missing-source': (licenses / 'ffmpeg/ffmpeg-8.1.tar.xz').unlink()
if scenario == 'missing-license': (licenses / 'openh264/LICENSE').unlink()
if scenario == 'wrong-identity':
    path = output / 'Captures Native Development.app/Contents/Info.plist'
    value = plistlib.loads(path.read_bytes()); value['CFBundleIdentifier'] = 'es.captur.app'; path.write_bytes(plistlib.dumps(value))
archive = root / ('artifact.tar.gz' if platform == 'linux' else 'artifact.zip')
package.archive(platform, output, executable, archive, target)
if platform != 'linux':
    # Explicit UNIX attributes also make cross-target fixtures portable when
    # this test runs on Windows (where chmod cannot manufacture UNIX modes).
    with zipfile.ZipFile(archive) as reader: entries = [(item, reader.read(item)) for item in reader.infolist()]
    with zipfile.ZipFile(archive, 'w', compression=zipfile.ZIP_DEFLATED) as writer:
        for item, body in entries:
            item.create_system = 3
            is_executable = item.filename.endswith('/CapturesNative') or '/binaries/' in item.filename
            item.external_attr = ((0o40755 if item.is_dir() else 0o100755 if is_executable else 0o100644) << 16) | (0x10 if item.is_dir() else 0)
            if scenario == 'wrong-hash' and item.filename.endswith('/BUILD_INFO.json'):
                value = json.loads(body); value['binary_sha256'] = '0' * 64; body = json.dumps(value).encode()
            writer.writestr(item, body)
else:
    # Preserve the real tar/PAX format while normalizing cross-target exec bits.
    with tarfile.open(archive) as reader: entries = [(item, reader.extractfile(item).read() if item.isfile() else None) for item in reader]
    with tarfile.open(archive, 'w:gz') as writer:
        for item, body in entries:
            if item.isfile(): item.mode = 0o755 if item.name.endswith('/captures-native') or '/binaries/' in item.name else 0o644
            writer.addfile(item, io.BytesIO(body) if body is not None else None)
"#]).arg(repository).arg(root.path()).arg(target.as_str()).arg(scenario).output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    fs::read(root.path().join(if target == Target::LinuxX64 {
        "artifact.tar.gz"
    } else {
        "artifact.zip"
    }))
    .unwrap()
}

fn verified(body: &[u8], target: Target, directory: &Path) -> VerifiedUpdate {
    let key = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
    let renderer = if matches!(target, Target::MacArm64 | Target::MacX64) {
        Renderer::Appkit
    } else {
        Renderer::Wgpu
    };
    let manifest = serde_json::to_vec(&json!({
        "schema":1, "identity":DEVELOPMENT_IDENTITY, "renderer":renderer,
        "version":"2026.10.51", "artifacts":{target.as_str():{
            "url":"https://example.invalid/native", "size":body.len(),
            "sha256":format!("{:x}", Sha256::digest(body))
        }}
    }))
    .unwrap();
    let signature = minisign::sign(None, &key.sk, Cursor::new(&manifest), None, None)
        .unwrap()
        .into_string();
    UpdateClient::new(
        "https://example.invalid/native.json",
        &key.pk.to_box().unwrap().to_string(),
        renderer,
        target,
        "2026.10.50",
    )
    .unwrap()
    .authenticate(&manifest, signature.as_bytes())
    .unwrap()
    .unwrap()
    .verify_download(
        Cursor::new(body),
        directory,
        &CancelToken::default(),
        |_, _| {},
    )
    .unwrap()
}

fn zip_fixture(names: &[&str]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for name in names {
        writer
            .start_file(
                *name,
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Stored)
                    .unix_permissions(0o644),
            )
            .unwrap();
        writer.write_all(b"distinct entry bytes").unwrap();
    }
    writer.finish().unwrap().into_inner()
}

#[test]
fn signed_packages_stage_all_platform_layouts_and_remove_only_owned_scratch() {
    for target in [
        Target::MacArm64,
        Target::MacX64,
        Target::WindowsX64,
        Target::LinuxX64,
    ] {
        let directory = tempfile::tempdir().unwrap();
        let installed = directory.path().join("installed-profile");
        fs::create_dir(&installed).unwrap();
        fs::write(installed.join("settings.json"), b"untouched").unwrap();
        let body = package_fixture(target, "normal");
        let download = verified(&body, target, directory.path());
        let staged = download
            .stage(directory.path(), &CancelToken::default())
            .unwrap();
        assert_eq!(fs::read(staged.executable()).unwrap(), EXECUTABLE);
        assert_eq!(staged.info().target, target);
        assert_eq!(staged.info().version, "2026.10.51");
        let package = staged.package().to_owned();
        assert!(package.is_dir() && package.starts_with(directory.path()));
        assert_eq!(
            fs::read(installed.join("settings.json")).unwrap(),
            b"untouched"
        );
        drop(staged);
        assert!(!package.exists());
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }
}

#[test]
fn incomplete_wrong_identity_and_changed_binary_packages_leave_no_stage() {
    for (target, scenario) in [
        (Target::LinuxX64, "missing-sidecar"),
        (Target::WindowsX64, "missing-source"),
        (Target::MacArm64, "missing-license"),
        (Target::MacX64, "wrong-identity"),
        (Target::WindowsX64, "wrong-hash"),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let body = package_fixture(target, scenario);
        assert!(
            matches!(
                verified(&body, target, directory.path())
                    .stage(directory.path(), &CancelToken::default()),
                Err(Error::Package)
            ),
            "{scenario}"
        );
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
    }
}

#[test]
fn staging_rehashes_download_after_handle_mutation_and_resets_its_read_offset() {
    let directory = tempfile::tempdir().unwrap();
    let body = package_fixture(Target::LinuxX64, "normal");
    let download = verified(&body, Target::LinuxX64, directory.path());
    download.file().read_to_end(&mut Vec::new()).unwrap();
    let staged = download
        .stage(directory.path(), &CancelToken::default())
        .unwrap();
    assert_eq!(fs::read(staged.executable()).unwrap(), EXECUTABLE);
    drop(staged);
    let download = verified(&body, Target::LinuxX64, directory.path());
    let mut handle = download.file().try_clone().unwrap();
    handle.seek(SeekFrom::Start(7)).unwrap();
    handle.write_all(b"X").unwrap();
    drop(handle);
    assert!(matches!(
        download.stage(directory.path(), &CancelToken::default()),
        Err(Error::Hash)
    ));
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[test]
fn unsafe_names_case_duplicates_and_file_directory_conflicts_are_rejected() {
    for names in [
        vec!["../escaped"],
        vec!["/absolute"],
        vec!["C:/absolute"],
        vec!["pkg/../escaped"],
        vec!["pkg\\escaped"],
        vec!["pkg/file:stream"],
        vec!["pkg/NUL.txt"],
        vec!["pkg/LPT3.log"],
        vec!["pkg/trailing."],
        vec!["pkg//empty"],
        vec!["pkg/non-ascii-é"],
        vec!["pkg/a", "pkg/A"],
        vec!["pkg/a", "pkg/a/child"],
        vec!["pkg/a/child", "pkg/a"],
        vec!["pkg/Foo/a", "pkg/foo/b"],
        vec!["pkg/a", "second/b"],
    ] {
        let directory = tempfile::tempdir().unwrap();
        let body = zip_fixture(&names);
        assert!(
            matches!(
                verified(&body, Target::WindowsX64, directory.path())
                    .stage(directory.path(), &CancelToken::default()),
                Err(Error::ArchivePath)
            ),
            "{names:?}"
        );
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
    }
}

#[test]
fn malformed_zip_bounds_links_local_mismatch_crc_and_false_sizes_are_rejected() {
    let original = zip_fixture(&["pkg/safe"]);
    let end = original.len() - 22;
    let central = u32_at(&original, end + 16) as usize;
    let cases = [
        (end + 8, (MAX_ENTRIES as u16 + 1).to_le_bytes().to_vec()),
        (
            end + 12,
            (MAX_CENTRAL_BYTES as u32 + 1).to_le_bytes().to_vec(),
        ),
        (central + 38, (0o120777_u32 << 16).to_le_bytes().to_vec()),
        (
            central + 24,
            (MAX_FILE_BYTES as u32 + 1).to_le_bytes().to_vec(),
        ),
        (central + 8, 0x0008_u16.to_le_bytes().to_vec()),
        (6, 0x0008_u16.to_le_bytes().to_vec()),
        (28, 1_u16.to_le_bytes().to_vec()),
        (30, b"pKg/safe".to_vec()),
        (14, 0_u32.to_le_bytes().to_vec()),
    ];
    for (at, bytes) in cases {
        let mut body = original.clone();
        body[at..at + bytes.len()].copy_from_slice(&bytes);
        let directory = tempfile::tempdir().unwrap();
        assert!(matches!(
            verified(&body, Target::WindowsX64, directory.path())
                .stage(directory.path(), &CancelToken::default()),
            Err(Error::Archive)
        ));
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
    }
    // A size lie must fail even when both headers agree (reader CRC alone does
    // not enforce the declared uncompressed size).
    let mut body = original.clone();
    for at in [22, central + 24] {
        body[at..at + 4].copy_from_slice(&3_u32.to_le_bytes());
    }
    let directory = tempfile::tempdir().unwrap();
    assert!(matches!(
        verified(&body, Target::WindowsX64, directory.path())
            .stage(directory.path(), &CancelToken::default()),
        Err(Error::Archive)
    ));
    // Corrupt contents while keeping local and central CRC declarations equal.
    let mut body = original.clone();
    body[30 + "pkg/safe".len()] ^= 0x40;
    assert!(matches!(
        verified(&body, Target::WindowsX64, directory.path())
            .stage(directory.path(), &CancelToken::default()),
        Err(Error::Archive)
    ));
    // Never search backward and accept an older footer after a bad final one.
    let mut body = original.clone();
    body.extend_from_slice(&original[end..]);
    assert!(matches!(
        verified(&body, Target::WindowsX64, directory.path())
            .stage(directory.path(), &CancelToken::default()),
        Err(Error::Archive)
    ));
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[test]
fn tar_links_pax_metadata_overflow_and_corrupted_gzip_are_rejected() {
    for kind in [
        tar::EntryType::Symlink,
        tar::EntryType::Link,
        tar::EntryType::Fifo,
        tar::EntryType::XGlobalHeader,
    ] {
        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_ustar();
        header.set_entry_type(kind);
        header.set_mode(0o644);
        header.set_size(0);
        header.set_link_name("outside").unwrap();
        header.set_cksum();
        builder
            .append_data(&mut header, "pkg/unsafe", io::empty())
            .unwrap();
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&builder.into_inner().unwrap()).unwrap();
        let body = encoder.finish().unwrap();
        let directory = tempfile::tempdir().unwrap();
        assert!(matches!(
            verified(&body, Target::LinuxX64, directory.path())
                .stage(directory.path(), &CancelToken::default()),
            Err(Error::Archive)
        ));
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
    }
    // An oversized raw PAX declaration must fail before attempting to allocate
    // its body, which is deliberately absent.
    let mut header = tar::Header::new_ustar();
    header.set_entry_type(tar::EntryType::XHeader);
    header.set_mode(0o644);
    header.set_size(MAX_METADATA_BYTES + 1);
    header.set_cksum();
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(header.as_bytes()).unwrap();
    let body = encoder.finish().unwrap();
    let directory = tempfile::tempdir().unwrap();
    assert!(matches!(
        verified(&body, Target::LinuxX64, directory.path())
            .stage(directory.path(), &CancelToken::default()),
        Err(Error::Archive)
    ));
    let mut body = package_fixture(Target::LinuxX64, "normal");
    let last = body.len() - 8;
    body[last] ^= 0x40;
    assert!(matches!(
        verified(&body, Target::LinuxX64, directory.path())
            .stage(directory.path(), &CancelToken::default()),
        Err(Error::Archive)
    ));
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[test]
fn extraction_budget_edges_and_cancellation_do_not_write_past_a_boundary() {
    let directory = tempfile::tempdir().unwrap();
    let mut extraction = Extraction::new(directory.path().into());
    extraction
        .reserve("pkg/at-limit", false, MAX_FILE_BYTES)
        .unwrap();
    assert!(matches!(
        extraction.reserve("pkg/over-limit", false, MAX_FILE_BYTES + 1),
        Err(Error::Archive)
    ));
    extraction.expanded = MAX_EXPANDED_BYTES - 3;
    extraction.reserve("pkg/last", false, 3).unwrap();
    assert!(matches!(
        extraction.reserve("pkg/one-more", false, 1),
        Err(Error::Archive)
    ));
    let mut tree = Extraction::new(directory.path().into());
    // One root plus these files uses the node budget exactly; declaring just
    // one more file must fail even though the explicit-entry count still fits.
    for index in 0..MAX_ENTRIES - 1 {
        tree.reserve(&format!("pkg/file-{index}"), false, 0)
            .unwrap();
    }
    assert!(matches!(
        tree.reserve("pkg/one-more", false, 0),
        Err(Error::Archive)
    ));
    let mut tree = Extraction::new(directory.path().into());
    // Implicit ancestors also consume the limit; a deeply nested final entry
    // cannot create more nodes merely because it is a single archive record.
    for index in 0..MAX_ENTRIES - 2 {
        tree.reserve(&format!("pkg/file-{index}"), false, 0)
            .unwrap();
    }
    assert!(matches!(
        tree.reserve("pkg/implicit/final", false, 0),
        Err(Error::Archive)
    ));
    let cancel = CancelToken::default();
    cancel.cancel();
    let body = zip_fixture(&["pkg/safe"]);
    let download = verified(&body, Target::WindowsX64, directory.path());
    assert!(matches!(
        download.stage(directory.path(), &cancel),
        Err(Error::Cancelled)
    ));
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
    struct CancelOnRead(CancelToken);
    impl Read for CancelOnRead {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            bytes[0] = 7;
            self.0.cancel();
            Ok(1)
        }
    }
    let cancel = CancelToken::default();
    let mut output = Vec::new();
    assert!(matches!(
        copy_entry(CancelOnRead(cancel.clone()), &mut output, 4, &cancel),
        Err(Error::Cancelled)
    ));
    assert!(output.is_empty());
}
