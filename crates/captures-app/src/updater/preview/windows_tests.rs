//! Uses the release bundler's actual Unicode NSIS and RunAsUser plugin. No
//! installation, registry writes, elevation, or existing profile is involved.
use super::*;
use std::{io::Cursor, sync::mpsc, time::Instant};

#[test]
#[ignore = "requires pinned NSIS toolchain; run by the Windows CI job"]
fn windows_nsis_preserves_restart_session_until_matching_acknowledgement() {
    let compiler = std::env::var_os("CAPTURES_TEST_MAKENSIS")
        .expect("run scripts/test-windows-update-session.ps1");
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("capture space 日本 ' $ ` session");
    fs::create_dir(&directory).unwrap();
    // A standalone process observes the real argv after GetOptions and
    // RunAsUser. Copy the session before publishing a deliberately wrong token;
    // only the test's gate permits the matching acknowledgement.
    fs::write(
        root.path().join("probe.rs"),
        r#"
use std::{env, fs, path::PathBuf, thread, time::{Duration, Instant}};
fn main() {
    let args: Vec<_> = env::args_os().skip(1).collect();
    assert_eq!(args.len(), 5);
    assert_eq!(args[0], "--live");
    assert_eq!(args[1], "--native-update-ready-file");
    assert_eq!(args[3], "--native-update-ready-token");
    let ready = PathBuf::from(&args[2]);
    let directory = ready.parent().unwrap();
    let marker = ready.with_extension("restart.json");
    fs::copy(&marker, directory.join("observed.json")).unwrap();
    fs::write(&ready, b"wrong-token\n").unwrap();
    fs::write(directory.join("observed"), b"argv and session received\n").unwrap();
    let started = Instant::now();
    while !directory.join("release").exists() {
        assert!(started.elapsed() < Duration::from_secs(30));
        thread::sleep(Duration::from_millis(25));
    }
    fs::remove_file(marker).unwrap();
    fs::write(ready, format!("{}\n", args[4].to_str().unwrap())).unwrap();
}
"#,
    )
    .unwrap();
    let output = Command::new("rustc")
        .arg(root.path().join("probe.rs"))
        .arg("-o")
        .arg(directory.join("probe.exe"))
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    // Match the release template's /ARGS extraction and asynchronous launch,
    // without changing the machine's installed application.
    fs::write(
        directory.join("fixture.nsi"),
        r#"
Unicode true
RequestExecutionLevel user
SilentInstall silent
!addplugindir "${NSISDIR}\Plugins\x86-unicode\additional"
!include "FileFunc.nsh"
OutFile "fixture.exe"
Section
  ${GetOptions} $CMDLINE "/R" $R0
  IfErrors done
  ${GetOptions} $CMDLINE "/ARGS" $R0
  nsis_tauri_utils::RunAsUser "$EXEDIR\probe.exe" "$R0"
  done:
SectionEnd
"#,
    )
    .unwrap();
    let output = Command::new(compiler)
        .current_dir(&directory)
        .arg("fixture.nsi")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let payload = fs::read(directory.join("fixture.exe")).unwrap();
    fs::write(directory.join("artifact"), &payload).unwrap();
    let pair = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
    let signature = minisign::sign(
        Some(&pair.pk),
        &pair.sk,
        Cursor::new(&payload),
        Some("timestamp:1\tfile:Captures-2026.10.1101-x86_64-pc-windows-msvc-setup.exe"),
        None,
    )
    .unwrap();
    let signature = crate::editor_layers::base64(signature.into_string().as_bytes());
    let key = PublicKey::decode(&pair.pk.to_box().unwrap().to_string()).unwrap();
    let request = InstallRequest {
        version: "2026.10.1101".into(),
        target: Target::WindowsX64,
        signature,
        installation: InstallTarget::Windows {
            executable: directory.join("captures.exe"),
        },
        executable: directory.join("captures.exe"),
        preferences_visible: true,
        preview_artifact_ids: vec!["second".into(), "first".into()],
        capture_windows: vec![
            super::super::RestartCaptureWindow::ScreenshotEditor {
                artifact_id: "draft-owner".into(),
                source_artifact_id: Some("saved-copy".into()),
            },
            super::super::RestartCaptureWindow::RecordingEditor {
                artifact_id: "recording".into(),
            },
        ],
        recovery: None,
    };
    let request_path = directory.join("install.json");
    fs::write(&request_path, serde_json::to_vec(&request).unwrap()).unwrap();
    let lock = tempfile::tempfile().unwrap();
    let (send, receive) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        send.send(install_with(&request_path, "2026.10.1001", &key, || {
            Ok(lock)
        }))
        .unwrap();
    });
    let started = Instant::now();
    while !directory.join("observed").exists() {
        assert!(started.elapsed() < Duration::from_secs(30));
        assert!(matches!(receive.try_recv(), Err(mpsc::TryRecvError::Empty)));
        std::thread::sleep(Duration::from_millis(25));
    }
    let observed: super::super::RestartIntent =
        serde_json::from_slice(&fs::read(directory.join("observed.json")).unwrap()).unwrap();
    assert!(observed.restore_preferences);
    assert_eq!(observed.preview_artifact_ids, ["second", "first"]);
    assert_eq!(observed.capture_windows, request.capture_windows);
    assert!(matches!(
        receive.recv_timeout(Duration::from_millis(500)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    assert!(!directory.join("completed").exists());
    assert!(directory.join("artifact").exists());
    fs::write(directory.join("release"), []).unwrap();
    receive
        .recv_timeout(Duration::from_secs(15))
        .unwrap()
        .unwrap();
    worker.join().unwrap();
    assert_eq!(
        fs::read(directory.join("completed")).unwrap(),
        b"installed\n"
    );
    assert!(!directory.join("artifact").exists());
    assert!(!directory.join("ready.restart.json").exists());

    // Reject a command that NSIS would truncate before launching it. Four
    // legal components add 480 UTF-16 units to each of the two command paths;
    // counting Unicode scalar values instead would incorrectly allow it.
    let mut long = root.path().to_owned();
    for _ in 0..4 {
        long.push("🦀".repeat(60));
    }
    fs::create_dir_all(&long).unwrap();
    fs::write(long.join("artifact"), payload).unwrap();
    fs::write(
        long.join("install.json"),
        serde_json::to_vec(&request).unwrap(),
    )
    .unwrap();
    let key = PublicKey::decode(&pair.pk.to_box().unwrap().to_string()).unwrap();
    assert!(matches!(
        install_with(&long.join("install.json"), "2026.10.1001", &key, || {
            Ok(tempfile::tempfile().unwrap())
        }),
        Err(Error::Configuration(
            "Windows update command exceeds the NSIS string limit"
        ))
    ));
    assert!(!long.join("completed").exists());
    assert!(long.join("artifact").exists());
}
