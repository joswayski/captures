//! No-window portal recording/session diagnostic. Does not enable resident UI.
#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("The Wayland recording probe is Linux-only.");
    std::process::exit(3);
}

#[cfg(target_os = "linux")]
fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

#[cfg(target_os = "linux")]
fn run() -> Result<(), String> {
    use captures_media::{CancelToken, MediaToolchain};
    use captures_recording::{
        AudioOptions, GifOptions, MaxResolution, RecordingKind, RecordingOptions, RecordingState,
        RecordingTarget,
    };
    use captures_recording_platform::{RecordingRecovery, RecordingSession};
    use std::{
        path::PathBuf,
        thread,
        time::{Duration, Instant},
    };

    if std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_none() {
        return Err("WAYLAND_DISPLAY is required and DISPLAY must be unset.".into());
    }
    let mut output = None;
    let mut duration_ms = 450_u64;
    let mut cancel_after = None;
    let mut show_cursor = false;
    let mut scenario = "video".to_owned();
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        let value = arguments
            .next()
            .ok_or_else(|| format!("Missing value for {argument}"))?;
        match argument.as_str() {
            "--output" => output = Some(PathBuf::from(value)),
            "--duration-ms" => duration_ms = value.parse().map_err(|_| "Invalid duration")?,
            "--cancel-after-ms" => {
                cancel_after = Some(Duration::from_millis(
                    value.parse().map_err(|_| "Invalid cancellation")?,
                ))
            }
            "--show-cursor" => show_cursor = value.parse().map_err(|_| "Invalid cursor flag")?,
            "--scenario" => scenario = value,
            _ => return Err(format!("Unknown argument {argument}")),
        }
    }
    if !(100..=10_000).contains(&duration_ms)
        || ![
            "video",
            "gif",
            "pause",
            "restart",
            "discard",
            "recover",
            "stream-loss",
        ]
        .contains(&scenario.as_str())
    {
        return Err("Invalid duration or recording scenario.".into());
    }
    let output = output.ok_or("usage: wayland_recording_probe --output NEW_DIRECTORY [--scenario video|gif|pause|restart|discard|recover|stream-loss]")?;
    // Diagnostics never overwrite or import an existing profile.
    std::fs::create_dir(&output).map_err(|error| error.to_string())?;
    let history = output.join("history");
    let recovery_root = output.join("recording-recovery");
    let options = RecordingOptions {
        kind: if scenario == "gif" {
            RecordingKind::Gif
        } else {
            RecordingKind::Video
        },
        target: RecordingTarget::PortalDisplay,
        frames_per_second: 15,
        max_resolution: MaxResolution::Original,
        countdown_seconds: 0,
        show_cursor,
        highlight_clicks: false,
        show_keystrokes: false,
        audio: AudioOptions::default(),
        gif: GifOptions::default(),
    };
    let started = Instant::now();
    let is_current = || cancel_after.is_none_or(|duration| started.elapsed() < duration);
    let mut session = RecordingSession::prepare(recovery_root, options, None)?;
    session.start(false, is_current)?;
    println!(
        "{}",
        serde_json::json!({"event": "recording", "target": session.snapshot().options.target})
    );
    let record_until = |session: &mut RecordingSession, duration: u64| -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_millis(duration);
        while Instant::now() < deadline {
            if !is_current() {
                session.discard()?;
                return Err("Recording cancelled".into());
            }
            if session.snapshot().warning.is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        Ok(())
    };
    record_until(&mut session, duration_ms)?;
    if scenario == "discard" {
        let directory = session.directory().to_owned();
        let snapshot = session.discard()?;
        if directory.exists() || snapshot.state != RecordingState::Discarded {
            return Err("Discard retained recording media.".into());
        }
        println!("{}", serde_json::json!({"event": "discarded"}));
        return Ok(());
    }
    if scenario == "pause" {
        let snapshot = session.pause()?;
        println!(
            "{}",
            serde_json::json!({"event": "paused", "elapsed_ms": snapshot.elapsed_ms})
        );
        thread::sleep(Duration::from_millis(800));
        if session.snapshot().elapsed_ms != snapshot.elapsed_ms {
            return Err("Paused recording elapsed time changed.".into());
        }
        session.start(false, is_current)?; // A new grant, never a persistent restore token.
        record_until(&mut session, duration_ms + 300)?;
    }
    if scenario == "restart" {
        session.restart()?;
        if !session.manifest().segments.is_empty() || session.snapshot().elapsed_ms != 0 {
            return Err("Restart retained the previous take.".into());
        }
        session.start(false, is_current)?;
        record_until(&mut session, duration_ms + 300)?;
    }
    let tools = MediaToolchain::from_command_names();
    let cancel = CancelToken::default();
    if scenario == "recover" || scenario == "stream-loss" {
        if scenario == "stream-loss" {
            let error = session
                .stop()
                .err()
                .ok_or("Expected a lost granted stream")?;
            if session.snapshot().state != RecordingState::Failed {
                return Err("Stream loss did not fail the session.".into());
            }
            println!(
                "{}",
                serde_json::json!({"event": "stream-lost", "error": error})
            );
        } else {
            session.pause()?;
        }
        let id = session.manifest().session_id.clone();
        drop(session);
        let recovery = RecordingRecovery::new(history, tools);
        let rows = recovery.list()?;
        let row = rows
            .iter()
            .find(|row| row.session_id == id)
            .ok_or("Missing recovery draft")?;
        let result = recovery.recover(
            &id,
            row.identity.as_deref().ok_or("Missing draft identity")?,
            &cancel,
            |_| {},
        )?;
        println!(
            "{}",
            serde_json::json!({"event": "recovered", "entry": result.entry, "path": result.path, "warning": result.warning})
        );
        return Ok(());
    }
    session.stop()?;
    let segments = session.manifest().segments.len();
    let result = session.finish(&history, &tools, &cancel)?;
    println!(
        "{}",
        serde_json::json!({"event": "ready", "entry": result.entry, "path": result.path, "segments": segments, "warning": result.warning})
    );
    Ok(())
}
