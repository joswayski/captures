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
    use captures_app::{
        capture_flow::PortalCapture,
        shortcuts::{
            CaptureShortcuts, PortalShortcutStatus, SelectorInputScope, selector_input_scope,
        },
    };
    use std::{
        io::{BufRead, Write},
        sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
            mpsc,
        },
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
    let wakes = Arc::new(AtomicU64::new(0));
    let create = || {
        let wakes = wakes.clone();
        CaptureShortcuts::new_wayland(&settings, move || {
            wakes.fetch_add(1, Ordering::Release);
        })
    };
    let scope_json = |scope: Option<SelectorInputScope>| {
        scope.map(
            |scope| serde_json::json!({"generation":scope.generation,"revision":scope.revision}),
        )
    };
    let mut owner = Some(create()?);
    let mut capture: Option<PortalCapture> = None;
    let mut hold_actions = false;
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
                    capture.take();
                    owner = Some(create()?);
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
                "hold" => hold_actions = true,
                "selector" => {
                    shortcuts.set_selector_generation(None);
                    capture.take();
                    let guard = PortalCapture::begin()?;
                    shortcuts.set_selector_generation(Some(guard.generation()));
                    capture = Some(guard);
                }
                "cancel" => capture.as_ref().ok_or("No capture to cancel")?.cancel(),
                "take" => {
                    let action = shortcuts.next_action();
                    println!("{}", serde_json::json!({"event":"taken","action":action}));
                }
                "inspect" => {}
                _ => return Err(format!("Unknown command: {command}")),
            }
            let shortcuts = owner.as_ref().unwrap();
            println!(
                "{}",
                serde_json::json!({"event":"command","command":command.trim(),
                    "wakes":wakes.load(Ordering::Acquire),
                    "observed":scope_json(selector_input_scope()),
                    "applied":scope_json(shortcuts.applied_selector_input_scope()),
                    "capture_current":capture.as_ref().map(PortalCapture::is_current)})
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
        if !hold_actions && let Some(action) = shortcuts.next_action() {
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
