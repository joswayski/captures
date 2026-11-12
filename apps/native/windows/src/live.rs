//! `--live`: Captures as the resident app instead of one fixture window.
//!
//! The first launch for a profile wins the shared `captures_app::instance`
//! election and stays running in the notification area; later launches
//! forward to it and exit. Windows open as shipping's
//! `interactive_launch_action` and `app_reactivation` decide.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant, SystemTime};

use captures_app::app_windows::{self, AppWindow, InteractiveLaunch, PrimaryWindow, Reactivation};
use captures_app::instance::{Instance, Launch, OpenRequest};
use captures_app::shortcuts::{CaptureShortcut, CaptureShortcuts};
use captures_app::tray_notice;
use captures_app::updater::checks::{CheckStatus, CheckWorker};
use captures_app::updater::{HealthAcknowledgement, RestartCaptureWindow, RestartIntent};
use captures_history::HistoryEntry;
use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, HWND_MESSAGE, MB_ICONERROR, MB_OK, MessageBoxW, PostQuitMessage,
    PostThreadMessageW, SW_SHOWNORMAL, SetForegroundWindow, ShowWindow, WINDOW_EX_STYLE,
    WINDOW_STYLE, WM_APP,
};
use windows::core::{HSTRING, w};

use crate::Options;
use crate::capture::Outcome;
use crate::tray::{Action, Tray};
use captures_app::capture_menu::MenuMode;

/// Posted to the UI thread so its wait returns; carries no data.
const WM_APP_LIVE: u32 = WM_APP + 20;

struct Resident {
    options: Options,
    /// The installed Preview's update worker; `None` in development builds.
    /// Declared before `instance` so it is joined while the election is held.
    updates: Option<CheckWorker>,
    /// Update now was pressed: install once the package is verified.
    update_install: bool,
    /// Automatic checks after the first, as shipping's five-minute interval.
    next_update_check: Instant,
    /// Open With files waiting to import, in arrival order.
    open_queue: VecDeque<PathBuf>,
    /// The import running on its worker.
    opening: Option<Receiver<(PathBuf, Result<HistoryEntry, String>)>>,
    /// The startup acknowledgement, until settings, the renderer and the
    /// bundled media tools are ready.
    health: Option<Health>,
    instance: Instance,
    /// `None` when the notification area is unavailable: like the other
    /// hosts, closing the last window then quits.
    tray: Option<Tray>,
    /// The saved capture shortcuts; `None` until first-run setup finishes,
    /// like shipping, or when registration failed.
    shortcuts: Option<CaptureShortcuts>,
    /// Released while a Preferences shortcut recorder is active so it can
    /// receive chords Captures owns.
    shortcuts_suspended: bool,
    settings_modified: Option<SystemTime>,
    thread: u32,
    /// A message-only window that owns the clipboard for auto-copy.
    clipboard: Option<HWND>,
}

/// Shipping's automatic update check interval.
const UPDATE_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// The update readiness gate, as the wgpu host: persisted settings load, a
/// Direct2D device starts and the bundled FFmpeg/FFprobe run.
struct Health {
    acknowledgement: HealthAcknowledgement,
    renderer: bool,
    tools: Receiver<Result<(), String>>,
}

thread_local! {
    static RESIDENT: RefCell<Option<Resident>> = const { RefCell::new(None) };
}

/// Run before any window opens. `None` means another process owns this
/// profile and accepted the relaunch with any Open With files.
pub fn elect(options: &Options) -> std::io::Result<Option<Instance>> {
    let root = options
        .history_root
        .clone()
        .unwrap_or_else(captures_app::default_history_root);
    // A relaunch is the foreground process; let the resident copy it
    // forwards to bring its window forward instead of opening it behind.
    let _ = unsafe {
        windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(
            windows::Win32::UI::WindowsAndMessaging::ASFW_ANY,
        )
    };
    Ok(
        match Instance::start(&root, OpenRequest::from_paths(options.open.clone())?)? {
            Launch::Primary(instance) => Some(instance),
            Launch::Forwarded => None,
        },
    )
}

/// Closing the last window keeps a resident app with a tray running.
pub fn resident() -> bool {
    RESIDENT.with(|r| {
        r.try_borrow()
            .map_or(true, |r| r.as_ref().is_some_and(|r| r.tray.is_some()))
    })
}

fn wake_on(thread: u32) -> impl Fn() + Send + Sync + Clone + 'static {
    move || unsafe {
        let _ = PostThreadMessageW(thread, WM_APP_LIVE, WPARAM(0), LPARAM(0));
    }
}

fn modified(options: &Options) -> Option<SystemTime> {
    std::fs::metadata(&options.settings_file)
        .and_then(|m| m.modified())
        .ok()
}

/// Become the resident app and open what shipping opens at launch.
pub fn start(options: &Options, instance: Instance) {
    let thread = unsafe { GetCurrentThreadId() };
    let wake = wake_on(thread);
    instance.set_wake(wake.clone());
    let settings = captures_settings::load(&options.settings_file).unwrap_or_default();
    let tray = match Tray::new(&settings, wake) {
        Ok(tray) => Some(tray),
        Err(error) => {
            eprintln!("{error}");
            None
        }
    };
    let tray_missing = tray.is_none();
    // An installed Preview checks for updates at launch and every five
    // minutes after, as shipping; development builds make no request.
    // `ReleaseInfo` keeps `Renderer::Wgpu` (see docs/native-rewrite.md).
    let updates = captures_app::preview_channel::version().and_then(|_| {
        let wake = wake_on(thread);
        match CheckWorker::preview(captures_app::updater::Renderer::Wgpu, Arc::new(wake)) {
            Ok(mut worker) => {
                worker.check_automatically();
                Some(worker)
            }
            Err(error) => {
                eprintln!("Preview updates unavailable: {error}");
                None
            }
        }
    });
    if updates.is_some() {
        // Wakes the loop for the next automatic check; ends with the process.
        let wake = wake_on(thread);
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(UPDATE_INTERVAL);
                wake();
            }
        });
    }
    let health = options.health.clone().map(|acknowledgement| {
        let (sender, tools) = channel();
        let wake = wake_on(thread);
        std::thread::spawn(move || {
            let _ = sender.send(
                crate::recording::toolchain()
                    .verify()
                    .map_err(|error| error.to_string()),
            );
            wake();
        });
        let tokens = crate::appearance::tokens(options.appearance.as_deref(), &settings);
        Health {
            acknowledgement,
            renderer: crate::gfx::Gfx::new(&tokens, true).is_ok(),
            tools,
        }
    });
    RESIDENT.with(|r| {
        *r.borrow_mut() = Some(Resident {
            options: options.clone(),
            updates,
            update_install: false,
            next_update_check: Instant::now() + UPDATE_INTERVAL,
            open_queue: VecDeque::new(),
            opening: None,
            health,
            instance,
            tray,
            shortcuts: None,
            shortcuts_suspended: false,
            settings_modified: modified(options),
            thread,
            clipboard: unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    w!("STATIC"),
                    None,
                    WINDOW_STYLE(0),
                    0,
                    0,
                    0,
                    0,
                    Some(HWND_MESSAGE),
                    None,
                    None,
                    None,
                )
            }
            .ok(),
        })
    });
    sync_shortcuts(&settings);
    // Open With opens only the files' editors, and an update restart only
    // what was open before, as shipping; unfinished setup still shows.
    let launch = match &options.restart {
        Some(_) if settings.onboarding_completed => None,
        _ => app_windows::interactive_launch(
            settings.onboarding_completed,
            options.quiet,
            !options.open.is_empty(),
            false,
        ),
    };
    if let Some(restart) = &options.restart {
        restore(options, restart);
    }
    open_media(options.open.clone());
    match launch {
        // A quiet launch stays in the notification area with the launch
        // notice for 5 s.
        Some(InteractiveLaunch::StartupNotice) if !tray_missing => {
            startup_notice(options, tray_notice::STARTUP_NOTICE_AUTOSTART_VISIBLE)
        }
        Some(InteractiveLaunch::StartupNoticeAndPreferences) if !tray_missing => {
            startup_notice(options, tray_notice::STARTUP_NOTICE_AUTOSTART_VISIBLE);
            open_preferences(options);
        }
        Some(InteractiveLaunch::Setup) => open_setup(options),
        Some(_) => open_preferences(options),
        None => {}
    }
}

/// Release the tray and the profile election.
pub fn stop() {
    crate::recorder::shutdown();
    RESIDENT.with(|r| r.borrow_mut().take());
}

fn open_preferences(options: &Options) {
    if let Err(error) = crate::host::open(options) {
        eprintln!("couldn't open Preferences: {error}");
    }
}

/// Shipping's launch notice with the saved New Capture keys.
fn startup_notice(options: &Options, visible_for: std::time::Duration) {
    let settings = captures_settings::load(&options.settings_file).unwrap_or_default();
    let shortcut = Some(settings.new_capture_shortcut.clone())
        .filter(|shortcut| !shortcut.trim().is_empty())
        .unwrap_or_else(captures_settings::default_new_capture_shortcut);
    let keys = captures_app::shortcuts::shortcut_display_tokens(
        &shortcut,
        captures_app::shortcuts::ShortcutPlatform::Windows,
    );
    let tokens = crate::appearance::tokens(options.appearance.as_deref(), &settings);
    crate::startup_notice::show(tokens, keys, visible_for, tray_rect);
}

fn tray_rect() -> Option<RECT> {
    RESIDENT.with(|r| r.try_borrow().ok()?.as_ref()?.tray.as_ref()?.rect())
}

/// First-run setup finished: the resident app shows the launch notice for
/// 15 s, like shipping's `complete_onboarding`.
pub fn setup_finished() {
    let options = RESIDENT.with(|r| {
        r.try_borrow()
            .ok()?
            .as_ref()
            .filter(|r| r.tray.is_some())
            .map(|r| r.options.clone())
    });
    if let Some(options) = options {
        startup_notice(&options, tray_notice::STARTUP_NOTICE_AFTER_SETUP_VISIBLE);
    }
}

fn open_setup(options: &Options) {
    if let Err(error) = crate::setup::open(options) {
        eprintln!("couldn't open setup: {error}");
    }
}

fn focus(hwnd: HWND) {
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
        let _ = SetForegroundWindow(hwnd);
    }
}

/// Handle tray events, forwarded launches and saved-shortcut changes. Call
/// from the host loop outside any window borrow.
pub fn service() {
    let Some(options) = RESIDENT.with(|r| r.borrow().as_ref().map(|r| r.options.clone())) else {
        return;
    };
    // Preferences saves in this process; any message wakes the loop after it.
    let changed = RESIDENT.with(|r| {
        let mut r = r.borrow_mut();
        let r = r.as_mut()?;
        let now = modified(&r.options);
        (now != r.settings_modified).then(|| r.settings_modified = now)
    });
    if changed.is_some() {
        let settings = captures_settings::load(&options.settings_file).unwrap_or_default();
        RESIDENT.with(|r| {
            if let Some(tray) = r.borrow_mut().as_mut().and_then(|r| r.tray.as_mut()) {
                tray.update(&settings);
            }
        });
        sync_shortcuts(&settings);
    }
    let recorder = crate::host::with_host(|h| h.scene.recording.is_some()).unwrap_or(false);
    suspend_shortcuts(recorder);
    match crate::capture::service() {
        Some(Outcome::Captured(result)) => match *result {
            Ok(Some(artifact)) => captured(&options, artifact),
            Ok(None) => {}
            Err(error) => show_error(&error),
        },
        None => {}
        Some(Outcome::Record(handoff)) => {
            let thread = RESIDENT.with(|r| r.borrow().as_ref().map(|r| r.thread));
            if let Some(thread) = thread {
                crate::recorder::start(&options, *handoff, wake_on(thread));
            }
        }
    }
    crate::recorder::service();
    crate::previews::set_suppressed(crate::capture::active());
    crate::recorder::protect_for_snapshot(crate::capture::preparing());
    while let Some(action) = RESIDENT.with(|r| r.borrow().as_ref()?.tray.as_ref()?.next_action()) {
        run(&options, action);
    }
    while let Some(shortcut) =
        RESIDENT.with(|r| r.borrow().as_ref()?.shortcuts.as_ref()?.next_action())
    {
        // As the tray rows: each opens the bar on its mode and target.
        use crate::capture::Target;
        let (target, mode) = match shortcut {
            CaptureShortcut::NewCapture | CaptureShortcut::Region => {
                (Target::Region, MenuMode::Screenshot)
            }
            CaptureShortcut::Window => (Target::Window, MenuMode::Screenshot),
            CaptureShortcut::Display => (Target::Display, MenuMode::Screenshot),
            CaptureShortcut::RecordRegion => (Target::Region, MenuMode::Recording),
            CaptureShortcut::RecordWindow => (Target::Window, MenuMode::Recording),
            CaptureShortcut::RecordDisplay => (Target::Display, MenuMode::Recording),
        };
        new_capture(&options, target, mode, false);
    }
    for _ in 0..32 {
        let next = RESIDENT.with(|r| r.borrow().as_ref().map(|r| r.instance.next_request()));
        match next {
            // A relaunch with files opens their editors, as shipping.
            Some(Ok(Some(request))) if !request.paths.is_empty() => open_media(request.paths),
            Some(Ok(Some(_))) => reactivate(&options),
            Some(Err(error)) => {
                eprintln!("Native instance forwarding failed: {error}");
                break;
            }
            _ => break,
        }
    }
    opened(&options);
    acknowledge(&options);
    service_updates();
}

/// Acknowledge the installer's or recovery companion's readiness file once
/// settings load, the renderer started and the media tools verified.
fn acknowledge(options: &Options) {
    let ready = RESIDENT.with(|r| {
        let mut r = r.borrow_mut();
        let health = r.as_mut()?.health.as_ref()?;
        let tools = match health.tools.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return None,
            Err(_) => Err("the media check stopped".into()),
        };
        Some((tools, health.renderer))
    });
    let Some((tools, renderer)) = ready else {
        return;
    };
    let health = RESIDENT.with(|r| r.borrow_mut().as_mut()?.health.take());
    let settings = captures_settings::load(&options.settings_file);
    match (tools, renderer, settings, health) {
        (Ok(()), true, Ok(_), Some(health)) => {
            if let Err(error) = health.acknowledgement.acknowledge() {
                eprintln!("Could not confirm startup: {error}");
            }
        }
        (tools, renderer, settings, _) => eprintln!(
            "Startup not confirmed: media tools {tools:?}, renderer {renderer}, settings {:?}",
            settings.err().map(|e| e.to_string())
        ),
    }
}

/// Open With: import each file into History through the shared `OpenMedia`
/// request on a worker, one at a time in arrival order, then open its
/// editor. A file already in History reuses its entry; one being edited
/// focuses its window.
fn open_media(paths: Vec<PathBuf>) {
    RESIDENT.with(|r| {
        if let Some(r) = r.borrow_mut().as_mut() {
            r.open_queue.extend(paths);
            open_next(r);
        }
    });
}

fn open_next(r: &mut Resident) {
    if r.opening.is_some() {
        return;
    }
    let Some(path) = r.open_queue.pop_front() else {
        return;
    };
    let root = r
        .options
        .history_root
        .clone()
        .unwrap_or_else(captures_app::default_history_root);
    let open_artifact_ids = crate::editor::ids()
        .into_iter()
        .chain(crate::recording::ids())
        .collect();
    let (ffmpeg, ffprobe) = crate::recording::tool_paths();
    let (sender, receiver) = channel();
    let wake = wake_on(r.thread);
    std::thread::spawn(move || {
        let result = captures_app::execute(captures_app::Request::OpenMedia {
            root,
            path: path.clone(),
            open_artifact_ids,
            ffmpeg: Some(ffmpeg),
            ffprobe: Some(ffprobe),
        })
        .map_err(|error| error.to_string())
        .and_then(|response| match response {
            captures_app::Response::OpenedMedia { artifact, .. } => Ok(artifact.entry),
            _ => Err("Captures could not open this file.".to_owned()),
        });
        let _ = sender.send((path, result));
        wake();
    });
    r.opening = Some(receiver);
}

/// A finished import: History reloads and the matching editor opens, or
/// shipping's "Captures" error shows.
fn opened(options: &Options) {
    let result = RESIDENT.with(|r| {
        let mut r = r.borrow_mut();
        let r = r.as_mut()?;
        let result = match r.opening.as_ref()?.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return None,
            Err(_) => (PathBuf::new(), Err("The import worker stopped.".into())),
        };
        r.opening = None;
        Some(result)
    });
    let Some((path, result)) = result else {
        return;
    };
    match result {
        Ok(entry) => {
            crate::history::with_history(|h| h.reload());
            let opened = if entry.kind.is_recording() {
                crate::recording::open(options, &entry.id)
            } else {
                crate::editor::open(options, &entry.id)
            };
            if let Err(error) = opened {
                show_error(&error.message());
            }
        }
        Err(error) => {
            eprintln!("Could not open {}: {error}", path.display());
            show_error(&error);
        }
    }
    RESIDENT.with(|r| {
        if let Some(r) = r.borrow_mut().as_mut() {
            open_next(r);
        }
    });
}

/// An update restart: the previews, editors and Preferences that were open.
fn restore(options: &Options, restart: &RestartIntent) {
    let root = options
        .history_root
        .clone()
        .unwrap_or_else(captures_app::default_history_root);
    let entries = captures_history::load(&root, chrono::Utc::now()).unwrap_or_default();
    for id in &restart.preview_artifact_ids {
        if let Some(entry) = entries.iter().find(|entry| &entry.id == id) {
            crate::previews::restore(options, entry);
        }
    }
    for window in &restart.capture_windows {
        let result = match window {
            RestartCaptureWindow::ScreenshotEditor { artifact_id, .. } => {
                crate::editor::open(options, artifact_id)
            }
            RestartCaptureWindow::RecordingEditor { artifact_id } => {
                crate::recording::open(options, artifact_id)
            }
        };
        if let Err(error) = result {
            eprintln!("Could not reopen a capture after the update: {error}");
        }
    }
    if restart.restore_preferences {
        open_preferences(options);
    }
}

/// The Preferences Updates card for the current update state; `None` in a
/// development build, which keeps "Updates unavailable".
pub fn update_card() -> Option<crate::updates::Card> {
    RESIDENT.with(|r| {
        let r = r.try_borrow().ok()?;
        let r = r.as_ref()?;
        let worker = r.updates.as_ref()?;
        Some(crate::updates::card(
            captures_app::preview_channel::version()?,
            worker.status(),
            r.update_install,
        ))
    })
}

/// Publish worker progress, run the five-minute check, act on the card's
/// button and hand a verified package to the update helper.
fn service_updates() {
    let step = crate::host::with_host(|h| h.update_step.take()).flatten();
    let install = RESIDENT.with(|r| {
        let mut r = r.borrow_mut();
        let r = r.as_mut()?;
        let worker = r.updates.as_mut()?;
        worker.poll();
        let now = Instant::now();
        if now >= r.next_update_check {
            r.next_update_check = now + UPDATE_INTERVAL;
            if matches!(
                worker.status(),
                CheckStatus::Idle | CheckStatus::UpToDate | CheckStatus::Error { .. }
            ) {
                worker.check_automatically();
            }
        }
        match step {
            Some(crate::updates::Step::Check) => {
                worker.check();
            }
            Some(crate::updates::Step::Download) => {
                r.update_install = worker.download();
            }
            Some(crate::updates::Step::Install) => r.update_install = true,
            None => {}
        }
        // A failed download or install ends this attempt.
        if matches!(worker.status(), CheckStatus::DownloadError { .. }) {
            r.update_install = false;
        }
        Some(r.update_install && matches!(worker.status(), CheckStatus::Staged { .. }))
    });
    if install == Some(true) {
        install_update();
    }
    if let Some(card) = update_card() {
        crate::host::with_host(|h| h.set_update(card));
    }
}

/// Shipping's Update now after verification: refuse during a capture or
/// recording, remember what is open, start the copied helper and quit. The
/// helper waits for this process's package lock before running the
/// installer, which relaunches with the readiness token.
fn install_update() {
    let blocker = if crate::recorder::active() {
        Some("Finish or cancel the active recording before installing the update.")
    } else if crate::capture::active() {
        Some("Finish the capture before installing the update.")
    } else {
        None
    };
    if let Some(message) = blocker {
        RESIDENT.with(|r| {
            if let Some(r) = r.borrow_mut().as_mut() {
                r.update_install = false;
            }
        });
        return show_error(message);
    }
    let restart = RestartIntent {
        restore_preferences: crate::host::with_host(|h| h.hwnd).is_some(),
        preview_artifact_ids: crate::previews::ids(),
        capture_windows: crate::editor::ids()
            .into_iter()
            .map(|artifact_id| RestartCaptureWindow::ScreenshotEditor {
                artifact_id,
                source_artifact_id: None,
            })
            .chain(
                crate::recording::ids()
                    .into_iter()
                    .map(|artifact_id| RestartCaptureWindow::RecordingEditor { artifact_id }),
            )
            .collect(),
    };
    let result = RESIDENT.with(|r| {
        let mut r = r.borrow_mut();
        let r = r.as_mut().ok_or("Captures is closing")?;
        let worker = r
            .updates
            .as_mut()
            .ok_or("Preview updates are unavailable")?;
        let result = worker
            .launch_preview_install(restart)
            .map_err(|error| error.to_string());
        if result.is_err() {
            r.update_install = false;
        }
        result
    });
    match result {
        // Quit drains editors (drafts are kept) before the process exits.
        Ok(()) => unsafe { PostQuitMessage(0) },
        // The worker now reports Update failed with the reason.
        Err(error) => eprintln!("Could not start the update: {error}"),
    }
}

fn run(options: &Options, action: Action) {
    match action {
        // A left click first dismisses a visible launch notice, as shipping.
        Action::LeftClick if crate::startup_notice::hide() => {}
        // Check for Updates checks now and shows the Updates card.
        Action::CheckUpdates => {
            open_preferences(options);
            crate::host::with_host(|h| {
                h.update_step = Some(crate::updates::Step::Check);
                h.show_updates();
            });
        }
        Action::Preferences | Action::LeftClick => open_preferences(options),
        Action::History => {
            if let Err(error) = crate::history::open(options, None) {
                eprintln!("couldn't open Capture History: {error}");
            }
        }
        Action::OpenOutputFolder => {
            let settings = captures_settings::load(&options.settings_file).unwrap_or_default();
            let path = std::path::PathBuf::from(settings.output_directory);
            let opened = std::fs::create_dir_all(&path).and_then(|()| {
                std::process::Command::new("explorer.exe")
                    .arg(&path)
                    .spawn()
                    .map(drop)
            });
            if let Err(error) = opened {
                eprintln!("Could not open the save location: {error}");
            }
        }
        Action::Quit => unsafe { PostQuitMessage(0) },
        // Like shipping, Screenshot Display opens the bar on Full screen.
        // Shipping's Screenshot Region and Window open their selector without
        // the bar; those direct overlays are not ported, so the bar opens on
        // that target instead.
        Action::NewCapture | Action::CaptureRegion => new_capture(
            options,
            crate::capture::Target::Region,
            MenuMode::Screenshot,
            true,
        ),
        Action::CaptureWindow => new_capture(
            options,
            crate::capture::Target::Window,
            MenuMode::Screenshot,
            true,
        ),
        Action::CaptureDisplay => new_capture(
            options,
            crate::capture::Target::Display,
            MenuMode::Screenshot,
            true,
        ),
        // The Record rows open the bar in Record on their target.
        Action::RecordRegion => new_capture(
            options,
            crate::capture::Target::Region,
            MenuMode::Recording,
            true,
        ),
        Action::RecordWindow => new_capture(
            options,
            crate::capture::Target::Window,
            MenuMode::Recording,
            true,
        ),
        Action::RecordDisplay => new_capture(
            options,
            crate::capture::Target::Display,
            MenuMode::Recording,
            true,
        ),
        Action::SendFeedback => {
            if let Err(error) = crate::feedback::open(options) {
                eprintln!("couldn't open Send Feedback: {error}");
            }
        }
    }
}

/// Register the saved capture shortcuts, or follow a change to them. Like
/// shipping, none are live until first-run setup finishes, and registering
/// takes over overlapping OS keys (Print Screen for Snipping Tool,
/// Win+Shift+S). While New Capture is open the shared capture flow drops
/// shortcut presses: switching its target in place is not ported yet.
fn sync_shortcuts(settings: &captures_settings::AppSettings) {
    RESIDENT.with(|r| {
        let mut r = r.borrow_mut();
        let Some(r) = r.as_mut() else { return };
        if !settings.onboarding_completed {
            r.shortcuts = None;
            return;
        }
        let result = match r.shortcuts.as_mut() {
            Some(shortcuts) => shortcuts.update(settings),
            None => CaptureShortcuts::new(settings, wake_on(r.thread)).map(|shortcuts| {
                r.shortcuts = Some(shortcuts);
                r.shortcuts_suspended = false;
            }),
        };
        // The shared error already says what failed.
        if let Err(error) = result {
            eprintln!("{error}");
        }
        // Shipping logs system-key takeover failures and keeps its bindings.
        if let Some(shortcuts) = r.shortcuts.as_mut() {
            for error in shortcuts.take_system_takeover().errors {
                eprintln!("{error}");
            }
        }
    });
}

/// Hidden recording controls keep New Capture as the chord that restores
/// them; the shared owner otherwise drops presses while a recording runs.
pub fn recording_hidden(hidden: bool) {
    RESIDENT.with(|r| {
        if let Ok(r) = r.try_borrow()
            && let Some(shortcuts) = r.as_ref().and_then(|r| r.shortcuts.as_ref())
        {
            shortcuts.set_restore_only(hidden);
        }
    });
}

/// Release the OS registrations while a Preferences recorder listens, and
/// restore them when it stops.
fn suspend_shortcuts(suspended: bool) {
    RESIDENT.with(|r| {
        let mut r = r.borrow_mut();
        let Some(r) = r.as_mut() else { return };
        if r.shortcuts_suspended == suspended {
            return;
        }
        let Some(shortcuts) = r.shortcuts.as_mut() else {
            return;
        };
        match shortcuts.set_suspended(suspended) {
            Ok(()) => r.shortcuts_suspended = suspended,
            Err(error) => eprintln!("Could not change capture shortcut registration: {error}"),
        }
    });
}

/// Shipping `app_reactivation` for a relaunch: focus the open window with
/// the highest priority (an editor, then History, then Preferences), or
/// open Preferences.
fn reactivate(options: &Options) {
    let editors: Vec<HWND> = crate::editor::windows()
        .into_iter()
        .chain(crate::recording::windows())
        .collect();
    let mut visible: Vec<_> = (0..editors.len()).map(PrimaryWindow::Editor).collect();
    let history = crate::history::with_history(|h| h.hwnd);
    if history.is_some() {
        visible.push(PrimaryWindow::App(AppWindow::History));
    }
    if crate::host::with_host(|h| h.hwnd).is_some() {
        visible.push(PrimaryWindow::App(AppWindow::Preferences));
    }
    let onboarded = captures_settings::load(&options.settings_file)
        .unwrap_or_default()
        .onboarding_completed;
    match app_windows::reactivation(onboarded, false, &visible) {
        Reactivation::Focus(PrimaryWindow::Editor(index)) => focus(editors[index]),
        Reactivation::Focus(PrimaryWindow::App(AppWindow::History)) => {
            if let Some(hwnd) = history {
                focus(hwnd);
            }
        }
        Reactivation::ShowSetup => open_setup(options),
        _ => open_preferences(options),
    }
}

/// Open New Capture, or first-run setup while it is unfinished.
fn new_capture(options: &Options, target: crate::capture::Target, mode: MenuMode, from_menu: bool) {
    let settings = captures_settings::load(&options.settings_file).unwrap_or_default();
    if !settings.onboarding_completed {
        return open_setup(options);
    }
    // A running recording owns the capture flow: bring its controls back.
    if crate::recorder::active() {
        return crate::recorder::restore();
    }
    open_capture(options, target, mode, from_menu);
}

/// The recording controls' Screenshot: the bar on Region, where the running
/// take keeps Record locked.
pub fn recording_screenshot() {
    if let Some(options) = RESIDENT.with(|r| r.borrow().as_ref().map(|r| r.options.clone())) {
        open_capture(
            &options,
            crate::capture::Target::Region,
            MenuMode::Screenshot,
            false,
        );
    }
}

fn open_capture(
    options: &Options,
    target: crate::capture::Target,
    mode: MenuMode,
    from_menu: bool,
) {
    let Some(thread) = RESIDENT.with(|r| r.borrow().as_ref().map(|r| r.thread)) else {
        return;
    };
    // Shipping hides the mini previews before the snapshot.
    crate::previews::set_suppressed(true);
    if let Err(error) = crate::capture::open(options, target, mode, from_menu, wake_on(thread)) {
        show_error(&error);
    }
}

/// A saved screenshot: an open History reloads, auto-copy puts it on the
/// clipboard as PNG and `CF_DIB`, and its mini preview appears.
fn captured(options: &Options, artifact: captures_app::Artifact) {
    crate::history::with_history(|h| h.reload());
    let settings = captures_settings::load(&options.settings_file).unwrap_or_default();
    let copied = settings.auto_copy_to_clipboard
        && match copy_png(&artifact.image_path) {
            Ok(()) => true,
            Err(error) => {
                eprintln!("Could not copy the screenshot: {error}");
                false
            }
        };
    crate::previews::show(options, &artifact.entry, &artifact.preview_path, copied);
}

/// Put a PNG on the clipboard as PNG and `CF_DIB`, owned by the resident
/// app's clipboard window.
pub fn copy_png(path: &std::path::Path) -> std::result::Result<(), String> {
    let owner = RESIDENT
        .with(|r| r.borrow().as_ref().and_then(|r| r.clipboard))
        .ok_or("The clipboard is unavailable")?;
    let png = std::fs::read(path).map_err(|e| e.to_string())?;
    let image = image::load_from_memory(&png)
        .map_err(|e| e.to_string())?
        .into_rgba8();
    crate::editor::set_clipboard(owner, &png, &crate::editor::dib(&image))
        .map_err(|e| e.message().to_string())
}

/// Capture failures use a system message box until the native error surface
/// is ported.
pub fn show_error(message: &str) {
    eprintln!("{message}");
    unsafe {
        MessageBoxW(
            None,
            &HSTRING::from(message),
            w!("Captures"),
            MB_OK | MB_ICONERROR,
        );
    }
}
