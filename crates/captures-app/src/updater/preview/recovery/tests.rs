use super::*;
#[cfg(target_os = "linux")]
use std::{io::Cursor, net::TcpListener};

fn host(directory: &Path) -> Startup {
    let lifetime = new_file(&directory.join("host.lock")).unwrap();
    lifetime.try_lock().unwrap();
    let ready = directory.join("ready");
    new_file(&ready).unwrap();
    Startup {
        _lifetime: lifetime,
        directory: directory.into(),
        acknowledgement: HealthAcknowledgement::new(ready, uuid::Uuid::new_v4().to_string())
            .unwrap(),
    }
}

#[test]
fn startup_health_and_secondary_quit_stop_monitoring_but_unacknowledged_exit_fails() {
    for outcome in ["ready", "normal", "crash"] {
        let directory = tempfile::tempdir().unwrap();
        let host = host(directory.path());
        let mut monitor = Monitor::open(directory.path(), &host.acknowledgement).unwrap();
        assert_eq!(monitor.state().unwrap(), State::Starting);
        fs::write(&host.acknowledgement.file, b"not the readiness token\n").unwrap();
        assert_eq!(monitor.state().unwrap(), State::Starting);
        fs::write(&host.acknowledgement.file, []).unwrap();
        match outcome {
            "ready" => host.acknowledgement.acknowledge().unwrap(),
            "normal" => host.finish(),
            _ => {}
        }
        drop(host);
        for _ in 0..3 {
            assert_eq!(
                monitor.state().unwrap(),
                if outcome == "crash" {
                    State::Failed
                } else {
                    State::Finished
                }
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn replacing_the_readiness_path_cannot_acknowledge_the_original_handle() {
    let directory = tempfile::tempdir().unwrap();
    let host = host(directory.path());
    let mut monitor = Monitor::open(directory.path(), &host.acknowledgement).unwrap();
    fs::rename(
        &host.acknowledgement.file,
        directory.path().join("original"),
    )
    .unwrap();
    new_file(&host.acknowledgement.file).unwrap();
    host.acknowledgement.acknowledge().unwrap();
    assert_eq!(monitor.state().unwrap(), State::Starting);
    drop(host);
    assert_eq!(monitor.state().unwrap(), State::Failed);
}

#[test]
fn changed_installations_and_repeated_or_older_repairs_are_not_replaced() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("Captures.AppImage");
    fs::write(&path, b"installed version").unwrap();
    let installation = InstallTarget::AppImage { path: path.clone() };
    let original = fingerprint(&installation).unwrap().unwrap();
    require_unchanged(&installation, &original).unwrap();
    fs::write(&path, b"newer manual replacement").unwrap();
    assert!(require_unchanged(&installation, &original).is_err());
    assert_eq!(fs::read(path).unwrap(), b"newer manual replacement");
    let attempted = directory.path().join("attempted-version.json");
    assert!(new_attempt(&attempted, "2026.10.1106").unwrap());
    replace_json(&attempted, &"2026.10.1106").unwrap();
    for version in ["2026.10.1105", "2026.10.1106", "2026.10.1106+other"] {
        assert!(!new_attempt(&attempted, version).unwrap());
    }
    assert!(new_attempt(&attempted, "2026.10.1107").unwrap());
    fs::write(&attempted, b"interrupted marker").unwrap();
    assert!(new_attempt(&attempted, "2026.10.1107").is_err());
}

#[cfg(target_os = "linux")]
#[test]
fn recovery_retries_http_and_tampering_then_installs_signed_fix_only_after_package_exit() {
    use std::os::unix::fs::PermissionsExt;
    for confirms_startup in [true, false] {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("watch-session");
        fs::create_dir(&directory).unwrap();
        let installed = root.path().join("Captures.AppImage");
        fs::write(&installed, b"broken package").unwrap();
        fs::set_permissions(&installed, fs::Permissions::from_mode(0o755)).unwrap();
        let history = root.path().join("old-history");
        fs::write(&history, b"retain my captures and drafts").unwrap();
        let installation = InstallTarget::AppImage {
            path: installed.clone(),
        };
        let host = host(&directory);
        let request = Request {
            version: "2026.10.1105".into(),
            installation: installation.clone(),
            fingerprint: fingerprint(&installation).unwrap(),
            health: host.acknowledgement.clone(),
        };
        // A real replacement process. No profile paths may bypass first-run import.
        // Readiness is the production one-shot contract, not an installer mock.
        let payload = if confirms_startup {
            b"#!/bin/sh\nset -eu\n[ \"$1\" = --live ]\n[ \"$2\" = --native-update-ready-file ]\n[ \"$4\" = --native-update-ready-token ]\n[ \"$#\" = 5 ]\nprintf '%s\\n' \"$5\" > \"$3\"\n".as_slice()
        } else {
            b"#!/bin/sh\nexit 42\n".as_slice()
        };
        let pair = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let key_text = pair.pk.to_box().unwrap().to_string();
        let key = PublicKey::decode(&key_text).unwrap();
        let signature = minisign::sign(
            None,
            &pair.sk,
            Cursor::new(payload),
            Some("timestamp:1\tfile:Captures-2026.10.1106-x86_64-unknown-linux-gnu.AppImage"),
            None,
        )
        .unwrap();
        let signature = crate::editor_layers::base64(signature.into_string().as_bytes());
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let manifest = serde_json::to_vec(&serde_json::json!({
            "version":"2026.10.1106", "platforms":{"linux-x86_64":{
                "url":format!("{endpoint}/artifact"),"signature":signature,"size":payload.len()
            }}
        }))
        .unwrap();
        let server = crate::updater::tests::serve(
            listener,
            vec![
                (503, vec![]),
                (200, manifest.clone()),
                (200, b"tampered".to_vec()),
                (200, manifest),
                (200, payload.to_vec()),
            ],
        );
        let client = Client::new(
            &[&endpoint],
            &key_text,
            Renderer::Wgpu,
            Target::LinuxX64,
            "2026.10.1105",
            installation,
        )
        .unwrap();
        let package_path = root.path().join("package.lock");
        let package = new_file(&package_path).unwrap();
        package.try_lock_shared().unwrap();
        let thread_directory = directory.clone();
        let (installing, wait_installing) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            acquire_fix(
                &request,
                &thread_directory,
                &client,
                3,
                Duration::ZERO,
                |path| {
                    install_with(path, "2026.10.1105", &key, || {
                        installing.send(()).unwrap();
                        Ok(File::options().read(true).write(true).open(&package_path)?)
                    })
                },
            )
        });
        wait_installing
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        std::thread::sleep(Duration::from_millis(250));
        assert!(!worker.is_finished());
        assert!(
            !root.path().join("attempted-version.json").exists(),
            "a busy package must not consume the repair attempt"
        );
        assert_eq!(
            fs::read(&installed).unwrap(),
            b"broken package",
            "a live package cannot be replaced"
        );
        assert!(!directory.join("completed").exists());
        drop(host);
        drop(package);
        let outcome = worker.join().unwrap();
        assert_eq!(outcome.is_ok(), confirms_startup);
        assert_eq!(fs::read(&installed).unwrap(), payload);
        assert_eq!(fs::read(history).unwrap(), b"retain my captures and drafts");
        assert_eq!(directory.join("completed").is_file(), confirms_startup);
        assert_eq!(
            read_json::<String>(&root.path().join("attempted-version.json")).unwrap(),
            "2026.10.1106"
        );
        assert_eq!(
            server.join().unwrap(),
            ["/", "/", "/artifact", "/", "/artifact"]
        );
        let backups: Vec<_> = fs::read_dir(root.path())
            .unwrap()
            .flatten()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".Captures-previous-")
            })
            .collect();
        if confirms_startup {
            assert!(backups.is_empty());
        } else {
            assert_eq!(backups.len(), 1);
            assert_eq!(fs::read(backups[0].path()).unwrap(), b"broken package");
            assert!(
                !new_attempt(&root.path().join("attempted-version.json"), "2026.10.1106").unwrap()
            );
        }
    }
}
