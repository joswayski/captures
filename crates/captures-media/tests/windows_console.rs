#![cfg(target_os = "windows")]

use std::{fs, os::windows::process::CommandExt, process::Command};

use captures_media::{CancelToken, MediaToolchain, RecordingAssemblyKind, RecordingSegmentInput};

#[test]
fn media_workers_do_not_create_a_console_window() {
    // CI may itself have a windowless console. Run the public-interface checks
    // with a console window so removing CREATE_NO_WINDOW would inherit it and
    // fail, rather than silently passing because of the runner's environment.
    if std::env::var_os("CAPTURES_MEDIA_CONSOLE_TEST_CHILD").is_none() {
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "media_workers_do_not_create_a_console_window",
                "--nocapture",
            ])
            .env("CAPTURES_MEDIA_CONSOLE_TEST_CHILD", "1")
            .creation_flags(0x0000_0010) // CREATE_NEW_CONSOLE
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "console-parent checks failed ({:?}):\n{}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }

    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("console_probe.rs");
    let executable = directory.path().join("console_probe.exe");
    // A console-subsystem stand-in for either sidecar. CREATE_NO_WINDOW still
    // provides a console buffer (CONOUT$ opens), but no console window. Reject
    // any window handle, not just a currently visible window, so hiding a window
    // after creation cannot mask a flash. No FFmpeg or bindings crate is needed.
    fs::write(
        &source,
        r##"
#[link(name = "kernel32")]
extern "system" {
    fn GetConsoleWindow() -> *mut std::ffi::c_void;
}
fn main() {
    // SAFETY: GetConsoleWindow has no arguments or caller preconditions.
    if !unsafe { GetConsoleWindow() }.is_null() {
        eprintln!("media worker has a console window");
        std::process::exit(23);
    }
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.iter().any(|arg| arg == "-show_streams") {
        println!("{}", r#"{"streams":[{"codec_type":"video","width":320,"height":180}],"format":{"format_name":"mov,mp4","duration":"1.25","size":"100"}}"#);
    } else if !args.iter().any(|arg| arg == "-version") {
        std::fs::write(args.last().unwrap(), b"fixture output").unwrap();
    }
}
"##,
    )
    .unwrap();
    let mut compiler = Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()));
    compiler.arg(&source).arg("-o").arg(&executable);
    #[cfg(target_env = "msvc")]
    compiler.args(["-C", "linker=rust-lld"]);
    let compiled = compiler.output().unwrap();
    assert!(
        compiled.status.success(),
        "fixture compilation failed: {}",
        String::from_utf8_lossy(&compiled.stderr)
    );

    // A positive control makes this meaningful even on a headless CI runner.
    let visible = Command::new(&executable)
        .creation_flags(0x0000_0010) // CREATE_NEW_CONSOLE
        .arg("-version")
        .output()
        .unwrap();
    assert_eq!(visible.status.code(), Some(23), "{visible:?}");

    let toolchain = MediaToolchain::new(executable.clone(), executable);
    let input = directory.path().join("recording.mp4");
    let cancel = CancelToken::default();
    // Recording startup and recovery, and Open With's probe/poster path, must
    // all keep their sidecars windowless while preserving output and I/O.
    toolchain.verify().unwrap();
    for probe in [
        toolchain.probe(&input).unwrap(),
        toolchain.probe_with_cancel(&input, &cancel).unwrap(),
    ] {
        assert_eq!((probe.metadata.width, probe.metadata.height), (320, 180));
        assert_eq!(probe.metadata.duration_ms, Some(1_250));
    }
    let segments: Vec<_> = [700, 550]
        .into_iter()
        .map(|duration_ms| RecordingSegmentInput {
            video_path: input.clone(),
            system_audio_path: None,
            system_audio_offset_ms: 0,
            microphone_path: None,
            microphone_offset_ms: 0,
            duration_ms,
        })
        .collect();
    let assembled = directory.path().join("assembled.mp4");
    // Missing system audio forces normalization before the two-segment concat.
    toolchain
        .assemble_recording(
            &segments,
            &assembled,
            directory.path(),
            RecordingAssemblyKind::Video {
                capture_system_audio: true,
            },
            &cancel,
        )
        .unwrap();
    assert_eq!(fs::read(&assembled).unwrap(), b"fixture output");
    let poster = directory.path().join("poster.png");
    toolchain.create_poster(&input, &poster, &cancel).unwrap();
    assert_eq!(fs::read(&poster).unwrap(), b"fixture output");
    let frame = directory.path().join("frame.png");
    toolchain
        .extract_frame(&input, 500, &frame, &cancel)
        .unwrap();
    assert_eq!(fs::read(&frame).unwrap(), b"fixture output");
}
