use super::*;
use crate::updater::staging::tests::{package_fixture, verified};
use serde_json::Value;
use tempfile::TempDir;

const OLD_BINARY: &[u8] = b"previous executable\0\xff\x13";
const NEW_BINARY: &[u8] = b"asymmetric native bytes\0\xff";
const OLD_TOOL: &[u8] = b"previous media tool";

struct Fixture {
    directory: TempDir,
    destination: PathBuf,
    executable: PathBuf,
    tool: PathBuf,
    target: Target,
    body: Vec<u8>,
}

impl Fixture {
    fn new() -> Self {
        let target = match (std::env::consts::OS, std::env::consts::ARCH) {
            ("macos", "aarch64") => Target::MacArm64,
            ("macos", "x86_64") => Target::MacX64,
            ("windows", "x86_64") => Target::WindowsX64,
            ("linux", "x86_64") => Target::LinuxX64,
            _ => panic!("unsupported fixture host"),
        };
        let directory = tempfile::tempdir().unwrap();
        let body = package_fixture(target, "normal");
        let old = verified(&body, target, directory.path())
            .stage(directory.path(), &CancelToken::default())
            .unwrap();
        let executable = old
            .executable()
            .strip_prefix(old.package())
            .unwrap()
            .to_owned();
        let tool = executable.parent().unwrap().join("binaries").join(format!(
            "ffmpeg-{}{}",
            target.as_str(),
            if target == Target::WindowsX64 {
                ".exe"
            } else {
                ""
            }
        ));
        let destination = directory.path().join("installed native é");
        fs::rename(old.package(), &destination).unwrap();
        drop(old);
        fs::write(destination.join(&executable), OLD_BINARY).unwrap();
        let mut metadata: Value =
            serde_json::from_slice(&fs::read(destination.join("BUILD_INFO.json")).unwrap())
                .unwrap();
        metadata["binary_sha256"] = format!("{:x}", Sha256::digest(OLD_BINARY)).into();
        fs::write(
            destination.join("BUILD_INFO.json"),
            serde_json::to_vec(&metadata).unwrap(),
        )
        .unwrap();
        fs::write(destination.join(&tool), OLD_TOOL).unwrap();
        fs::write(destination.join("LICENSE"), b"previous license\0").unwrap();
        fs::create_dir(destination.join("previous-empty-directory")).unwrap();
        fs::create_dir(directory.path().join("profile")).unwrap();
        fs::write(
            directory.path().join("profile/history.bin"),
            b"private captures stay outside",
        )
        .unwrap();
        Self {
            directory,
            destination,
            executable,
            tool,
            target,
            body,
        }
    }

    fn staged(&self) -> StagedUpdate {
        verified(&self.body, self.target, self.directory.path())
            .stage(self.directory.path(), &CancelToken::default())
            .unwrap()
    }

    fn replace(&self) -> PendingInstallation {
        self.staged()
            .replace(&self.destination, &CancelToken::default())
            .unwrap()
    }

    fn old(&self) {
        assert_eq!(
            fs::read(self.destination.join(&self.executable)).unwrap(),
            OLD_BINARY
        );
        assert_eq!(
            fs::read(self.destination.join(&self.tool)).unwrap(),
            OLD_TOOL
        );
        assert_eq!(
            fs::read(self.destination.join("LICENSE")).unwrap(),
            b"previous license\0"
        );
        assert!(self.destination.join("previous-empty-directory").is_dir());
        self.profile();
    }

    fn new_package(&self) {
        assert_eq!(
            fs::read(self.destination.join(&self.executable)).unwrap(),
            NEW_BINARY
        );
        assert_eq!(
            fs::read(self.destination.join(&self.tool)).unwrap(),
            b"inert tool"
        );
        assert!(!self.destination.join("previous-empty-directory").exists());
        self.profile();
    }

    fn profile(&self) {
        assert_eq!(
            fs::read(self.directory.path().join("profile/history.bin")).unwrap(),
            b"private captures stay outside"
        );
    }
}

#[test]
fn every_package_user_blocks_replacement_and_running_replacement_blocks_rollback() {
    let fixture = Fixture::new();
    let first_profile_host = PackageUse::acquire(&fixture.destination).unwrap();
    let second_profile_host = PackageUse::acquire(&fixture.destination).unwrap();
    let other_package = tempfile::tempdir().unwrap();
    let _other_host = PackageUse::acquire(other_package.path()).unwrap();
    assert!(matches!(
        fixture
            .staged()
            .replace(&fixture.destination, &CancelToken::default()),
        Err(Error::Installation("the development package is in use"))
    ));
    fixture.old();
    drop(first_profile_host);
    // The second independent descriptor must continue to block mutation.
    assert!(
        fixture
            .staged()
            .replace(&fixture.destination, &CancelToken::default())
            .is_err()
    );
    fixture.old();
    drop(second_profile_host);
    let pending = fixture.replace();
    let transaction = pending.paths.transaction.clone();
    let running_replacement = PackageUse::acquire(&fixture.destination).unwrap();
    assert!(matches!(
        pending.rollback(),
        Err(Error::Installation("the development package is in use"))
    ));
    assert!(matches!(
        recover_installation(&fixture.destination),
        Err(Error::Installation("the development package is in use"))
    ));
    fixture.new_package();
    assert!(transaction.is_dir());
    // The stable sibling guard also blocks recovery in the missing-root gap.
    fs::rename(&fixture.destination, transaction.join("prepared-new")).unwrap();
    assert!(matches!(
        recover_installation(&fixture.destination),
        Err(Error::Installation("the development package is in use"))
    ));
    assert_eq!(
        fs::read(transaction.join("prepared-new").join(&fixture.executable)).unwrap(),
        NEW_BINARY
    );
    drop(running_replacement);
    assert!(recover_installation(&fixture.destination).unwrap());
    fixture.old();
    assert!(!transaction.exists());
}

#[test]
#[ignore = "subprocess fixture invoked by package_use_is_released_on_normal_exit_and_host_death"]
fn package_use_process_fixture() {
    let package = PathBuf::from(std::env::var_os("CAPTURES_TEST_PACKAGE_USE").unwrap());
    let _lease = PackageUse::acquire(&package).unwrap();
    println!("PACKAGE_USE_READY");
    std::io::stdout().flush().unwrap();
    std::io::stdin().read_to_end(&mut Vec::new()).unwrap();
}

#[test]
fn package_use_is_released_on_normal_exit_and_host_death() {
    use std::{
        io::{BufRead, BufReader},
        process::{Command, Stdio},
    };
    let directory = tempfile::tempdir().unwrap();
    let paths = Paths::for_package(directory.path()).unwrap();
    for killed in [false, true] {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "updater::installation::tests::package_use_process_fixture",
                "--nocapture",
            ])
            .env("CAPTURES_TEST_PACKAGE_USE", directory.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        loop {
            assert_ne!(
                output.read_line(&mut line).unwrap(),
                0,
                "fixture exited before acquiring its lease"
            );
            if line.contains("PACKAGE_USE_READY") {
                break;
            }
            line.clear();
        }
        assert!(paths.exclusive_use().is_err());
        if killed {
            child.kill().unwrap();
        } else {
            drop(child.stdin.take());
        }
        let status = child.wait().unwrap();
        assert_eq!(status.success(), !killed);
        assert!(paths.exclusive_use().is_ok());
        assert!(paths.lock.with_extension("use.lock").is_file());
    }
    // Host death frees its OS lease, NOT proof that media descendants stopped.
    // The operator's all-processes-stopped requirement remains mandatory.
}

#[test]
fn replacement_reextracts_signed_bytes_and_confirmation_retains_new_package() {
    let fixture = Fixture::new();
    let staged = fixture.staged();
    fs::write(staged.executable(), b"mutated exposed executable").unwrap();
    fs::write(
        staged.package().join(&fixture.tool),
        b"mutated exposed sidecar",
    )
    .unwrap();
    let pending = staged
        .replace(&fixture.destination, &CancelToken::default())
        .unwrap();
    assert_eq!(pending.info().version, "2026.10.51");
    fixture.new_package();
    assert_eq!(
        fs::read(pending.paths.previous().join(&fixture.executable)).unwrap(),
        OLD_BINARY
    );
    let transaction = pending.paths.transaction.clone();
    pending.confirm().unwrap();
    fixture.new_package();
    assert!(!transaction.exists());
    assert!(!recover_installation(&fixture.destination).unwrap());
}

#[test]
fn explicit_rollback_and_dropped_handles_restore_every_previous_file() {
    for explicit in [false, true] {
        let fixture = Fixture::new();
        let pending = fixture.replace();
        fixture.new_package();
        if explicit {
            pending.rollback().unwrap();
        } else {
            drop(pending);
            assert!(recover_installation(&fixture.destination).unwrap());
        }
        fixture.old();
        assert!(!recover_installation(&fixture.destination).unwrap());
    }
}

#[test]
fn each_unconfirmed_rename_boundary_recovers_the_previous_package() {
    for boundary in [
        "prepared",
        "old-moved",
        "activated",
        "new-rejected",
        "old-restored",
    ] {
        let fixture = Fixture::new();
        let pending = fixture.replace();
        let paths = &pending.paths;
        match boundary {
            "prepared" => {
                fs::rename(&paths.package, paths.transaction.join("prepared-new")).unwrap();
                fs::rename(paths.previous(), &paths.package).unwrap();
            }
            "old-moved" => {
                fs::rename(&paths.package, paths.transaction.join("prepared-new")).unwrap();
            }
            "activated" => {}
            "new-rejected" | "old-restored" => {
                fs::rename(&paths.package, paths.transaction.join("rejected")).unwrap();
                if boundary == "old-restored" {
                    fs::rename(paths.previous(), &paths.package).unwrap();
                }
            }
            _ => unreachable!(),
        }
        drop(pending);
        assert!(
            recover_installation(&fixture.destination).unwrap(),
            "{boundary}"
        );
        fixture.old();
        assert!(
            !recover_installation(&fixture.destination).unwrap(),
            "{boundary}"
        );
    }
}

#[test]
fn committed_cleanup_survives_receipt_and_backup_deletion() {
    for confirmed in [false, true] {
        let fixture = Fixture::new();
        let pending = fixture.replace();
        if confirmed {
            fs::write(pending.paths.transaction.join("confirmed"), b"confirmed\n").unwrap();
        } else {
            fs::rename(
                &pending.paths.package,
                pending.paths.transaction.join("rejected"),
            )
            .unwrap();
            fs::rename(pending.paths.previous(), &pending.paths.package).unwrap();
        }
        // Emulate interruption AFTER the cleanup decision was committed, while
        // recursive deletion has already removed its receipt and some files.
        let garbage = pending.paths.garbage();
        fs::rename(&pending.paths.transaction, &garbage).unwrap();
        fs::remove_file(garbage.join("receipt.json")).unwrap();
        fs::remove_file(
            garbage
                .join(if confirmed { "previous" } else { "rejected" })
                .join(&fixture.executable),
        )
        .unwrap();
        drop(pending);
        assert!(recover_installation(&fixture.destination).unwrap());
        assert!(!garbage.exists());
        if confirmed {
            fixture.new_package();
        } else {
            fixture.old();
        }
        assert!(!recover_installation(&fixture.destination).unwrap());
    }
}

#[test]
fn confirmation_marker_before_cleanup_does_not_roll_back() {
    let fixture = Fixture::new();
    let pending = fixture.replace();
    fs::write(pending.paths.transaction.join("confirmed"), b"confirmed\n").unwrap();
    drop(pending);
    assert!(recover_installation(&fixture.destination).unwrap());
    fixture.new_package();
}

#[test]
fn changed_current_or_retained_resources_are_not_overwritten_or_deleted() {
    for change_previous in [false, true] {
        let fixture = Fixture::new();
        let pending = fixture.replace();
        let previous = pending.paths.previous();
        let changed = if change_previous {
            previous.join("LICENSE")
        } else {
            fixture.destination.join(&fixture.tool)
        };
        fs::write(&changed, b"changes made outside installer").unwrap();
        assert!(pending.confirm().is_err());
        assert!(recover_installation(&fixture.destination).is_err());
        assert_eq!(
            fs::read(changed).unwrap(),
            b"changes made outside installer"
        );
        assert!(previous.is_dir());
        fixture.profile();
    }
}

#[test]
fn stale_handles_cannot_confirm_or_roll_back_a_newer_transaction() {
    for confirm in [false, true] {
        let fixture = Fixture::new();
        let first = fixture.replace();
        assert!(recover_installation(&fixture.destination).unwrap());
        let second = fixture.replace();
        let result = if confirm {
            first.confirm()
        } else {
            first.rollback()
        };
        assert!(matches!(
            result,
            Err(Error::Installation("replacement is no longer pending"))
        ));
        fixture.new_package();
        assert!(second.paths.previous().is_dir());
        second.rollback().unwrap();
        fixture.old();
    }
}

#[test]
fn archive_tampering_and_cancellation_never_move_the_current_package() {
    for cancelled in [false, true] {
        let fixture = Fixture::new();
        let mut staged = fixture.staged();
        let cancel = CancelToken::default();
        if cancelled {
            cancel.cancel();
        } else {
            staged
                .archive
                .file
                .write_all(b"corrupt retained archive")
                .unwrap();
        }
        let result = staged.replace(&fixture.destination, &cancel);
        assert!(matches!(
            result,
            Err(Error::Cancelled | Error::Hash | Error::Size)
        ));
        fixture.old();
        assert!(
            !Paths::new(&fixture.destination)
                .unwrap()
                .transaction
                .exists()
        );
    }
}

#[test]
fn wrong_targets_profiles_pending_updates_and_locks_are_rejected() {
    let fixture = Fixture::new();
    let foreign = if fixture.target == Target::WindowsX64 {
        Target::LinuxX64
    } else {
        Target::WindowsX64
    };
    let foreign_body = package_fixture(foreign, "normal");
    let staged = verified(&foreign_body, foreign, fixture.directory.path())
        .stage(fixture.directory.path(), &CancelToken::default())
        .unwrap();
    assert!(matches!(
        staged.replace(&fixture.destination, &CancelToken::default()),
        Err(Error::Target)
    ));
    assert!(
        fixture
            .staged()
            .replace(
                &fixture.directory.path().join("profile"),
                &CancelToken::default()
            )
            .is_err()
    );
    assert!(
        fixture
            .staged()
            .replace(Path::new("relative"), &CancelToken::default())
            .is_err()
    );
    let running = std::env::current_exe().unwrap().canonicalize().unwrap();
    assert!(matches!(
        fixture
            .staged()
            .replace(running.parent().unwrap(), &CancelToken::default()),
        Err(Error::Installation(
            "the updater must run outside the package"
        ))
    ));
    let paths = Paths::new(&fixture.destination).unwrap();
    let lock = paths.lock().unwrap();
    assert!(matches!(
        fixture
            .staged()
            .replace(&fixture.destination, &CancelToken::default()),
        Err(Error::Installation(
            "another replacement operation is running"
        ))
    ));
    assert!(recover_installation(&fixture.destination).is_err());
    drop(lock);
    let pending = fixture.replace();
    assert!(matches!(
        fixture
            .staged()
            .replace(&fixture.destination, &CancelToken::default()),
        Err(Error::Installation("recover the pending replacement first"))
    ));
    pending.rollback().unwrap();
    fixture.old();
}

#[cfg(unix)]
#[test]
fn descriptor_inheritance_does_not_extend_the_operation_lock() {
    let fixture = Fixture::new();
    let paths = Paths::new(&fixture.destination).unwrap();
    let lock = paths.lock().unwrap();
    // A dup shares the open-file description just as a child inherits it between
    // fork and exec. Keep that descriptor alive after the operation finishes.
    let inherited = lock.0.try_clone().unwrap();
    assert!(paths.lock().is_err());
    drop(lock);
    assert!(paths.lock().is_ok());
    drop(inherited);
    fixture.old();
}

#[test]
fn tree_node_limit_is_inclusive_and_oversized_files_fail_before_reading() {
    let fixture = Fixture::new();
    // Count the input tree independently, then populate it to exactly the public
    // staging contract's 10,000-node limit (including the package root).
    let mut pending = vec![fixture.destination.clone()];
    let mut original_nodes = 0;
    while let Some(path) = pending.pop() {
        original_nodes += 1;
        if path.is_dir() {
            pending.extend(
                fs::read_dir(path)
                    .unwrap()
                    .map(|entry| entry.unwrap().path()),
            );
        }
    }
    for index in original_nodes..10_000 {
        File::create(fixture.destination.join(format!("empty-{index:05}"))).unwrap();
    }
    assert!(
        fingerprint(
            &fixture.destination,
            fixture.target,
            &CancelToken::default()
        )
        .is_ok()
    );
    let extra = fixture.destination.join("one-too-many");
    File::create(&extra).unwrap();
    assert!(matches!(
        fingerprint(
            &fixture.destination,
            fixture.target,
            &CancelToken::default()
        ),
        Err(Error::Archive)
    ));
    fs::remove_file(extra).unwrap();
    let large = fixture.destination.join("empty-09999");
    File::options()
        .write(true)
        .open(large)
        .unwrap()
        .set_len(512 * 1024 * 1024 + 1)
        .unwrap();
    assert!(matches!(
        fingerprint(
            &fixture.destination,
            fixture.target,
            &CancelToken::default()
        ),
        Err(Error::Archive)
    ));
    fixture.old();
}

#[test]
fn invalid_receipts_and_incomplete_preparation_preserve_all_packages() {
    for scenario in ["destination", "schema", "hash", "unknown", "missing"] {
        let fixture = Fixture::new();
        let pending = fixture.replace();
        let receipt_path = pending.paths.transaction.join("receipt.json");
        let mut value: Value = serde_json::from_slice(&fs::read(&receipt_path).unwrap()).unwrap();
        match scenario {
            "destination" => value["destination"] = "../another-package".into(),
            "schema" => value["schema"] = 99.into(),
            "hash" => value["new_hash"] = "bad".into(),
            "unknown" => value["unexpected"] = true.into(),
            "missing" => {}
            _ => unreachable!(),
        }
        if scenario == "missing" {
            fs::remove_file(&receipt_path).unwrap();
        } else {
            fs::write(&receipt_path, serde_json::to_vec(&value).unwrap()).unwrap();
        }
        assert!(
            recover_installation(&fixture.destination).is_err(),
            "{scenario}"
        );
        fixture.new_package();
        assert_eq!(
            fs::read(pending.paths.previous().join(&fixture.executable)).unwrap(),
            OLD_BINARY
        );
    }
}

#[cfg(unix)]
#[test]
fn links_and_execute_permission_changes_are_preserved_for_manual_recovery() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let fixture = Fixture::new();
    let pending = fixture.replace();
    let tool = fixture.destination.join(&fixture.tool);
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(recover_installation(&fixture.destination).is_err());
    assert!(pending.paths.previous().is_dir());
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o700)).unwrap();
    pending.rollback().unwrap();
    let sentinel = fixture.directory.path().join("sentinel");
    fs::write(&sentinel, b"do not touch").unwrap();
    fs::remove_file(fixture.destination.join(&fixture.tool)).unwrap();
    symlink(&sentinel, fixture.destination.join(&fixture.tool)).unwrap();
    assert!(
        fixture
            .staged()
            .replace(&fixture.destination, &CancelToken::default())
            .is_err()
    );
    assert_eq!(fs::read(&sentinel).unwrap(), b"do not touch");
    let alias = fixture.directory.path().join("alias");
    symlink(&fixture.destination, &alias).unwrap();
    assert!(
        fixture
            .staged()
            .replace(&alias, &CancelToken::default())
            .is_err()
    );
}

#[cfg(windows)]
#[test]
fn locked_destination_rename_failure_leaves_the_old_package_and_cleans_scratch() {
    use std::os::windows::fs::OpenOptionsExt;
    let fixture = Fixture::new();
    // FILE_FLAG_BACKUP_SEMANTICS + FILE_SHARE_READ: allow validation reads but
    // deny deletion/rename of the directory, emulating a Windows sharing failure.
    let locked = File::options()
        .read(true)
        .custom_flags(0x02000000)
        .share_mode(1)
        .open(&fixture.destination)
        .unwrap();
    assert!(matches!(
        fixture
            .staged()
            .replace(&fixture.destination, &CancelToken::default()),
        Err(Error::Io(_))
    ));
    fixture.old();
    assert!(
        !Paths::new(&fixture.destination)
            .unwrap()
            .transaction
            .exists()
    );
    drop(locked);
    fixture.replace().rollback().unwrap();
    fixture.old();
}

mod handoff {
    use super::*;
    use crate::updater::staging::tests::package_fixture_with_binary;
    use std::{
        process::{Child, Command},
        sync::OnceLock,
        thread,
        time::{Duration, Instant},
    };

    // A real same-target executable inside a signed real-packager archive. It
    // implements only the private wire, never calls the code under test, and
    // keeps a child alive after root exit to distinguish exit from quiescence.
    fn binary() -> &'static [u8] {
        static BINARY: OnceLock<Vec<u8>> = OnceLock::new();
        BINARY.get_or_init(|| {
            let directory = tempfile::tempdir().unwrap();
            let source = directory.path().join("host.rs");
            let output = directory.path().join(if cfg!(windows) { "host.exe" } else { "host" });
            fs::write(&source, r#"
use std::{env, fs, path::PathBuf, process::Command, thread, time::Duration};
fn wait(path: PathBuf) {
    while !path.exists() { thread::sleep(Duration::from_millis(5)); }
}
fn main() {
    let args: Vec<_> = env::args().collect();
    if args[1] == "--descendant" {
        let profile = PathBuf::from(&args[2]);
        let mut counter = 0;
        while !profile.join("stop-descendant").exists() {
            counter += 1;
            fs::write(profile.join("heartbeat"), counter.to_string()).unwrap();
            thread::sleep(Duration::from_millis(5));
        }
        fs::write(profile.join("descendant-done"), b"done").unwrap();
        return;
    }
    let value = |key| PathBuf::from(&args[args.iter().position(|arg| arg == key).unwrap() + 1]);
    let profile = value("--history-root").parent().unwrap().to_owned();
    assert_eq!(fs::canonicalize(env::current_dir().unwrap()).unwrap(), fs::canonicalize(&profile).unwrap());
    assert_eq!(env::var("CAPTURES_NATIVE_SKIP_SYSTEM_SHORTCUT_TAKEOVER").unwrap(), "1");
    let file = value("--native-update-ready-file");
    let token = &args[args.iter().position(|arg| arg == "--native-update-ready-token").unwrap() + 1];
    let bytes = format!("{token}\n");
    let mut descendant = None;
    match profile.file_name().unwrap().to_str().unwrap() {
        "partial" => {
            fs::write(&file, &bytes.as_bytes()[..9]).unwrap();
            fs::write(profile.join("partial-written"), b"ready").unwrap();
            wait(profile.join("complete"));
            fs::write(&file, &bytes).unwrap();
        }
        "wrong" => { fs::write(&file, b"not-the-attempt-token\n").unwrap(); }
        "oversize" => { fs::write(&file, format!("{bytes}x")).unwrap(); }
        "replaced-file" => {
            let other = file.with_extension("other");
            fs::write(&other, &bytes).unwrap();
            fs::remove_file(&file).unwrap();
            fs::rename(other, &file).unwrap();
        }
        "late" => {
            wait(profile.join("complete"));
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(&file, &bytes).unwrap();
            fs::write(profile.join("late-written"), b"ready").unwrap();
        }
        "exit-descendant" | "timeout-descendant" => {
            descendant = Some(Command::new(env::current_exe().unwrap())
                .arg("--descendant").arg(&profile).spawn().unwrap());
            wait(profile.join("heartbeat"));
            if profile.file_name().unwrap() == "exit-descendant" { return; }
        }
        _ => unreachable!(),
    }
    wait(profile.join("stop"));
    if let Some(mut child) = descendant { child.wait().unwrap(); }
}
"#).unwrap();
            let mut compiler = Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()));
            compiler.arg("--edition=2021").arg(&source).arg("-o").arg(&output);
            if cfg!(windows) { compiler.args(["-C", "linker=rust-lld"]); }
            let result = compiler.output().unwrap();
            assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
            fs::read(output).unwrap()
        })
    }

    fn fixture(mode: &str) -> (Fixture, PathBuf) {
        let mut fixture = Fixture::new();
        fixture.body = package_fixture_with_binary(fixture.target, "normal", binary());
        let profile = fixture.directory.path().join(mode);
        fs::create_dir(&profile).unwrap();
        (fixture, profile)
    }

    fn until(path: &Path) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !path.exists() {
            assert!(
                Instant::now() < deadline,
                "missing fixture event {}",
                path.display()
            );
            thread::sleep(Duration::from_millis(5));
        }
    }

    struct Running(Child, PathBuf);
    impl Drop for Running {
        fn drop(&mut self) {
            let _ = fs::write(self.1.join("stop-descendant"), []);
            let _ = fs::write(self.1.join("stop"), []);
            let _ = self.0.wait();
        }
    }

    fn retained(fixture: &Fixture, transaction: &Path) {
        assert!(transaction.join("receipt.json").is_file());
        assert!(!transaction.join("confirmed").exists());
        assert_eq!(
            fs::read(transaction.join("previous").join(&fixture.executable)).unwrap(),
            OLD_BINARY
        );
        assert_eq!(
            fs::read(fixture.destination.join(&fixture.executable)).unwrap(),
            binary()
        );
        fixture.profile();
    }

    fn shipping_sources(fixture: &Fixture) -> (PathBuf, PathBuf, Vec<u8>, String) {
        let settings = fixture.directory.path().join("shipping settings é.json");
        let data = fixture.directory.path().join("shipping data");
        let preferences = captures_settings::AppSettings {
            appearance: captures_settings::Appearance::Dark,
            onboarding_completed: true,
            launch_at_login: true,
            recording: captures_settings::RecordingSettings {
                gif_fps: 27,
                ..Default::default()
            },
            ..Default::default()
        };
        let bytes = serde_json::to_vec(&preferences).unwrap();
        fs::write(&settings, &bytes).unwrap();
        let image = image::RgbaImage::from_fn(7, 3, |x, y| {
            image::Rgba([x as u8 * 31, y as u8 * 71, 19, 255])
        });
        let capture = crate::persist_screenshot(
            &data.join("capture-history"),
            &image,
            captures_capture::CaptureMode::Region,
        )
        .unwrap();
        (settings, data, bytes, capture.entry.id)
    }

    #[test]
    fn imported_profile_waits_for_exact_health_and_preserves_shipping_and_snapshot_bytes() {
        let (fixture, profile) = fixture("partial");
        fs::remove_dir(&profile).unwrap();
        let (settings, data, original, id) = shipping_sources(&fixture);
        let pixels = fs::read(data.join("capture-history").join(&id).join("capture.png")).unwrap();
        let pending = fixture.replace();
        let transaction = pending.paths.transaction.clone();
        let (settings_copy, data_copy, profile_copy) =
            (settings.clone(), data.clone(), profile.clone());
        let launch = thread::spawn(move || {
            pending.launch_importing(
                &settings_copy,
                &data_copy,
                &profile_copy,
                Duration::from_secs(10),
                &CancelToken::default(),
            )
        });
        until(&profile.join("partial-written"));
        assert!(
            !launch.is_finished(),
            "import completion is not startup health"
        );
        retained(&fixture, &transaction);
        assert!(recover_installation(&fixture.destination).is_err());
        assert_eq!(fs::read(&settings).unwrap(), original);
        assert_eq!(
            fs::read(profile.join("source-snapshot/settings.json")).unwrap(),
            original
        );
        assert_eq!(
            fs::read(profile.join("history").join(&id).join("capture.png")).unwrap(),
            pixels
        );
        let copied = captures_settings::load(&profile.join("settings.json")).unwrap();
        assert_eq!(copied.recording.gif_fps, 27);
        assert!(!copied.onboarding_completed && !copied.launch_at_login);
        assert_eq!(
            PathBuf::from(copied.output_directory),
            profile.canonicalize().unwrap().join("exports")
        );
        fs::write(profile.join("complete"), []).unwrap();
        let _running = Running(launch.join().unwrap().unwrap(), profile);
        assert!(!transaction.exists());
        assert_eq!(fs::read(&settings).unwrap(), original);
        assert_eq!(
            fs::read(data.join("capture-history").join(&id).join("capture.png")).unwrap(),
            pixels
        );
    }

    #[test]
    fn failed_imported_handoff_and_explicit_package_recovery_retain_the_copied_profile() {
        let (fixture, profile) = fixture("wrong");
        fs::remove_dir(&profile).unwrap();
        let (settings, data, original, id) = shipping_sources(&fixture);
        let pixels = fs::read(data.join("capture-history").join(&id).join("capture.png")).unwrap();
        let pending = fixture.replace();
        let transaction = pending.paths.transaction.clone();
        let failure = pending
            .launch_importing(
                &settings,
                &data,
                &profile,
                Duration::from_secs(1),
                &CancelToken::default(),
            )
            .unwrap_err();
        let running = Running(failure.process.unwrap(), profile.clone());
        retained(&fixture, &transaction);
        let working = fs::read(profile.join("settings.json")).unwrap();
        drop(running);
        assert!(recover_installation(&fixture.destination).unwrap());
        fixture.old();
        assert_eq!(fs::read(profile.join("settings.json")).unwrap(), working);
        assert_eq!(
            fs::read(profile.join("source-snapshot/settings.json")).unwrap(),
            original
        );
        assert_eq!(fs::read(&settings).unwrap(), original);
        assert_eq!(
            fs::read(profile.join("history").join(id).join("capture.png")).unwrap(),
            pixels
        );
    }

    #[test]
    fn invalid_import_targets_sources_and_cancellation_never_start_a_host_or_publish_partial_data()
    {
        for mode in [
            "occupied",
            "profile-in-package",
            "data-in-package",
            "malformed",
            "cancel",
        ] {
            let fixture = Fixture::new();
            let (settings, mut data, mut original, _) = shipping_sources(&fixture);
            let pending = fixture.replace();
            let mut profile = fixture.directory.path().join("new copy");
            let cancel = CancelToken::default();
            match mode {
                "occupied" => {
                    fs::create_dir(&profile).unwrap();
                    fs::write(profile.join("sentinel"), b"never overwrite this profile").unwrap();
                }
                "profile-in-package" => profile = fixture.destination.join("new copy"),
                "data-in-package" => data = fixture.destination.clone(),
                "malformed" => {
                    original = b"invalid shipping settings".to_vec();
                    fs::write(&settings, &original).unwrap();
                }
                "cancel" => cancel.cancel(),
                _ => unreachable!(),
            }
            let failure = pending
                .launch_importing(&settings, &data, &profile, Duration::from_secs(1), &cancel)
                .unwrap_err();
            assert!(failure.process.is_none(), "{mode}");
            assert_eq!(fs::read(settings).unwrap(), original, "{mode}");
            if mode == "occupied" {
                assert_eq!(
                    fs::read(profile.join("sentinel")).unwrap(),
                    b"never overwrite this profile"
                );
            } else {
                assert!(!profile.exists(), "{mode}");
            }
            assert!(recover_installation(&fixture.destination).unwrap());
            fixture.old();
        }
    }

    #[test]
    fn partial_health_keeps_the_lock_and_backup_until_exact_acknowledgement() {
        let (fixture, profile) = fixture("partial");
        let pending = fixture.replace();
        let transaction = pending.paths.transaction.clone();
        let launched_profile = profile.clone();
        let launch = thread::spawn(move || {
            pending.launch(
                &launched_profile,
                Duration::from_secs(10),
                &CancelToken::default(),
            )
        });
        until(&profile.join("partial-written"));
        assert!(!launch.is_finished(), "a token prefix is not readiness");
        retained(&fixture, &transaction);
        assert!(
            recover_installation(&fixture.destination).is_err(),
            "handoff must exclude recovery"
        );
        fs::write(profile.join("complete"), []).unwrap();
        let _running = Running(launch.join().unwrap().unwrap(), profile);
        assert!(!transaction.exists());
        assert!(!recover_installation(&fixture.destination).unwrap());
        assert_eq!(
            fs::read(fixture.destination.join(&fixture.executable)).unwrap(),
            binary()
        );
        fixture.profile();
    }

    #[test]
    fn invalid_or_replaced_acknowledgements_never_confirm_or_rollback() {
        for mode in ["wrong", "oversize", "replaced-file"] {
            let (fixture, profile) = fixture(mode);
            let pending = fixture.replace();
            let transaction = pending.paths.transaction.clone();
            let failure = pending
                .launch(
                    &profile,
                    Duration::from_millis(500),
                    &CancelToken::default(),
                )
                .unwrap_err();
            let running = Running(failure.process.unwrap(), profile);
            retained(&fixture, &transaction);
            drop(running);
            // The known fixture process has no descendants and is now stopped.
            assert!(recover_installation(&fixture.destination).unwrap());
            fixture.old();
        }
    }

    #[test]
    fn late_health_after_terminal_timeout_cannot_confirm() {
        let (fixture, profile) = fixture("late");
        let pending = fixture.replace();
        let transaction = pending.paths.transaction.clone();
        let failure = pending
            .launch(
                &profile,
                Duration::from_millis(500),
                &CancelToken::default(),
            )
            .unwrap_err();
        let _running = Running(failure.process.unwrap(), profile.clone());
        fs::write(profile.join("complete"), []).unwrap();
        until(&profile.join("late-written"));
        retained(&fixture, &transaction);
    }

    #[test]
    fn successful_root_exit_and_timeout_with_live_descendants_retain_the_backup() {
        for mode in ["exit-descendant", "timeout-descendant"] {
            let (fixture, profile) = fixture(mode);
            let pending = fixture.replace();
            let transaction = pending.paths.transaction.clone();
            let failure = pending
                .launch(&profile, Duration::from_secs(1), &CancelToken::default())
                .unwrap_err();
            let mut running = Running(failure.process.unwrap(), profile.clone());
            until(&profile.join("heartbeat"));
            if mode == "exit-descendant" {
                assert!(
                    running.0.wait().unwrap().success(),
                    "even a clean root exit is not quiescence"
                );
            }
            let before = fs::read(profile.join("heartbeat")).unwrap();
            let deadline = Instant::now() + Duration::from_secs(10);
            while fs::read(profile.join("heartbeat")).unwrap() == before {
                assert!(Instant::now() < deadline, "descendant must remain live");
                thread::sleep(Duration::from_millis(5));
            }
            retained(&fixture, &transaction);
            drop(running);
            until(&profile.join("descendant-done"));
        }
    }

    #[test]
    fn invalid_profiles_deadlines_and_prelaunch_cancel_preserve_recovery() {
        for mode in ["occupied", "inside-package", "zero-deadline", "cancel"] {
            let fixture = Fixture::new();
            let pending = fixture.replace();
            let mut profile = fixture.directory.path().join("empty-test");
            fs::create_dir(&profile).unwrap();
            let cancel = CancelToken::default();
            let mut deadline = Duration::from_secs(1);
            match mode {
                "occupied" => fs::write(profile.join("capture"), b"preserve me").unwrap(),
                "inside-package" => profile = fixture.destination.clone(),
                "zero-deadline" => deadline = Duration::ZERO,
                "cancel" => cancel.cancel(),
                _ => unreachable!(),
            }
            let failure = pending.launch(&profile, deadline, &cancel).unwrap_err();
            assert!(failure.process.is_none());
            fixture.new_package();
            assert!(recover_installation(&fixture.destination).unwrap());
            fixture.old();
            if mode == "occupied" {
                assert_eq!(fs::read(profile.join("capture")).unwrap(), b"preserve me");
            }
        }
    }

    #[test]
    fn health_veto_after_rehash_preserves_unconfirmed_backup() {
        let fixture = Fixture::new();
        let pending = fixture.replace();
        let _lock = pending.paths.lock().unwrap();
        let mut checked = false;
        assert!(
            pending
                .confirm_locked(|| {
                    checked = true;
                    Err(Error::Installation("host exited during rehash"))
                })
                .is_err()
        );
        assert!(checked);
        assert!(!pending.paths.confirmed().unwrap());
        assert_eq!(
            fs::read(pending.paths.previous().join(&fixture.executable)).unwrap(),
            OLD_BINARY
        );
        fixture.new_package();
        drop(_lock);
        pending.rollback().unwrap();
        fixture.old();
    }
}
