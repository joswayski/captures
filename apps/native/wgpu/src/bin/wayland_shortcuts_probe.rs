//! No-window global-shortcut diagnostic. No X11 grabs or system-key takeover.
#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("The global shortcuts portal probe is Linux-only.");
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
    use captures_app::shortcuts::{CaptureShortcuts, PortalShortcutStatus};
    use std::{
        io::{BufRead, Write},
        sync::mpsc,
        thread,
        time::{Duration, Instant},
    };
    if std::env::var_os("DISPLAY").is_some() {
        return Err("DISPLAY must be unset so this probe cannot use X11.".into());
    }
    let mut settings = captures_settings::AppSettings {
        new_capture_shortcut: "Ctrl+Alt+F10".into(),
        region_shortcut: "Ctrl+Shift+F7".into(),
        window_shortcut: "Ctrl+Shift+F8".into(),
        display_shortcut: "Ctrl+Shift+F9".into(),
        ..Default::default()
    };
    settings.recording.video_shortcut = "Ctrl+Alt+F11".into();
    settings.recording.window_shortcut = "Alt+F9".into();
    settings.recording.display_shortcut = "Ctrl+Alt+F12".into();
    let (send, commands) = mpsc::sync_channel(16);
    thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else {
                break;
            };
            if send.send(line).is_err() {
                break;
            }
        }
    });
    let mut owner = Some(CaptureShortcuts::new_wayland(&settings, || {})?);
    let mut previous = None;
    let started = Instant::now();
    loop {
        let command = match commands.try_recv() {
            Ok(command) => Some(command),
            Err(mpsc::TryRecvError::Disconnected) => Some("quit".into()),
            Err(mpsc::TryRecvError::Empty) => None,
        };
        if let Some(command) = command {
            let shortcuts = owner.as_mut().unwrap();
            match command.trim() {
                "quit" => {
                    owner.take();
                    println!("{}", serde_json::json!({"event":"quit"}));
                    return Ok(());
                }
                "retry" => {
                    owner.take();
                    owner = Some(CaptureShortcuts::new_wayland(&settings, || {})?);
                    previous = None;
                }
                "configure" => {
                    let error = shortcuts.configure_portal().err();
                    println!("{}", serde_json::json!({"event":"configure","error":error}));
                }
                "suspend" => shortcuts.set_suspended(true)?,
                "resume" => shortcuts.set_suspended(false)?,
                "disable" => shortcuts.set_enabled(false),
                "enable" => shortcuts.set_enabled(true),
                _ => return Err(format!("Unknown command: {command}")),
            }
            println!(
                "{}",
                serde_json::json!({"event":"command","command":command.trim()})
            );
        }
        let shortcuts = owner.as_ref().unwrap();
        let status = shortcuts.portal_status().unwrap();
        if previous.as_ref() != Some(&status) {
            let detail = match &status {
                PortalShortcutStatus::Pending => serde_json::json!({"state":"pending"}),
                PortalShortcutStatus::Bound {
                    configurable,
                    triggers,
                    configuration_error,
                } => {
                    serde_json::json!({"state":"bound","configurable":configurable,"triggers":triggers,"configuration_error":configuration_error})
                }
                PortalShortcutStatus::Unavailable(error) => {
                    serde_json::json!({"state":"unavailable","error":error})
                }
            };
            println!("{}", serde_json::json!({"event":"status","detail":detail}));
            previous = Some(status);
        }
        if let Some(action) = shortcuts.next_action() {
            println!("{}", serde_json::json!({"event":"action","action":action}));
        }
        std::io::stdout()
            .flush()
            .map_err(|error| error.to_string())?;
        if started.elapsed() > Duration::from_secs(60) {
            return Err("Global-shortcuts diagnostic exceeded 60 seconds.".into());
        }
        thread::sleep(Duration::from_millis(20));
    }
}
