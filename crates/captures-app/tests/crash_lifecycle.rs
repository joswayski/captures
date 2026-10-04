//! Process isolation is required for the panic hook and OS shutdown handlers.
use captures_app::crash::Session;
use captures_feedback::crash_diagnostics::CrashSession;
use std::{io::Write, path::PathBuf, process::Command};
#[cfg(unix)]
use std::{
    io::{BufRead, BufReader},
    process::Stdio,
};

#[test]
fn native_sessions_classify_exits_and_preserve_replacement_ownership() {
    const PROFILE: &str = "CAPTURES_CRASH_CHILD_PROFILE";
    const MODE: &str = "CAPTURES_CRASH_CHILD_MODE";
    if let Some(profile) = std::env::var_os(PROFILE) {
        let profile = PathBuf::from(profile);
        let session = Session::start(&profile).unwrap();
        match std::env::var(MODE).unwrap().as_str() {
            "clean" => {
                session.clean_exit().unwrap();
                return;
            }
            "panic" => panic!("fixture panic at /home/diagnostic-person/code.rs"),
            "disarmed" => {
                session.clean_exit().unwrap();
                std::fs::write(
                    profile.join(".crash-diagnostics/current-session"),
                    "replacement marker",
                )
                .unwrap();
                std::fs::write(
                    profile.join(".crash-diagnostics/last-panic"),
                    "replacement panic",
                )
                .unwrap();
            }
            "resumed" => {
                session.clean_exit().unwrap();
                session.resume().unwrap();
            }
            _ => {}
        }
        println!("native-crash-child-ready");
        std::io::stdout().flush().unwrap();
        loop {
            std::thread::sleep(std::time::Duration::from_secs(60));
        }
    }

    for mode in ["clean", "panic"] {
        let profile = tempfile::tempdir().unwrap();
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "native_sessions_classify_exits_and_preserve_replacement_ownership",
                "--nocapture",
            ])
            .env(PROFILE, profile.path())
            .env(MODE, mode)
            .output()
            .unwrap();
        assert_eq!(output.status.success(), mode == "clean");
        let preview = CrashSession::start(profile.path()).unwrap().preview();
        assert_eq!(preview.unclean_exit, mode == "panic");
        assert_eq!(preview.has_exception_evidence(), mode == "panic");
        if mode == "panic" {
            let panic = preview.rust_panic.unwrap();
            assert!(panic.contains("~/code.rs"));
            assert!(!panic.contains("diagnostic-person"));
        }
    }

    #[cfg(unix)]
    for (mode, signal, clean) in [
        ("signal", "TERM", true),
        ("signal", "HUP", true),
        ("resumed", "INT", true),
        ("signal", "KILL", false),
        ("disarmed", "TERM", false),
    ] {
        let profile = tempfile::tempdir().unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "native_sessions_classify_exits_and_preserve_replacement_ownership",
                "--nocapture",
            ])
            .env(PROFILE, profile.path())
            .env(MODE, mode)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        let (ready, wait) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut line = String::new();
            while stdout.read_line(&mut line).is_ok_and(|count| count > 0) {
                if line.contains("native-crash-child-ready") {
                    let _ = ready.send(());
                    break;
                }
                line.clear();
            }
        });
        if wait
            .recv_timeout(std::time::Duration::from_secs(10))
            .is_err()
        {
            let _ = child.kill();
            let _ = child.wait();
            panic!("diagnostic child did not become ready");
        }
        assert!(
            Command::new("kill")
                .arg(format!("-{signal}"))
                .arg(child.id().to_string())
                .status()
                .unwrap()
                .success()
        );
        assert!(!child.wait().unwrap().success());
        let directory = profile.path().join(".crash-diagnostics");
        assert_eq!(
            directory.join("current-session").exists(),
            !clean,
            "{mode} {signal}"
        );
        if mode == "disarmed" {
            assert_eq!(
                std::fs::read_to_string(directory.join("current-session")).unwrap(),
                "replacement marker"
            );
            assert_eq!(
                std::fs::read_to_string(directory.join("last-panic")).unwrap(),
                "replacement panic"
            );
        }
    }
}

#[test]
fn history_retention_does_not_remove_profile_diagnostics() {
    let profile = tempfile::tempdir().unwrap();
    let session = CrashSession::start(profile.path()).unwrap();
    let marker = session.clean_exit_paths()[0].clone();
    let original = std::fs::read(&marker).unwrap();
    // The competing implementation (an ordinary child directory) is pruned.
    std::fs::create_dir(profile.path().join("malformed-capture")).unwrap();
    assert!(
        captures_history::load(profile.path(), chrono::Utc::now())
            .unwrap()
            .is_empty()
    );
    assert!(!profile.path().join("malformed-capture").exists());
    assert_eq!(std::fs::read(marker).unwrap(), original);
    session.mark_clean_exit().unwrap();
}
