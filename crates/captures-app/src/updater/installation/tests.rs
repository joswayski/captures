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
