//! `--live`: Captures as the resident app instead of one fixture window.
//!
//! The first launch for a profile wins the shared `captures_app::instance`
//! election and stays running in the notification area; later launches
//! forward to it and exit. Windows open as shipping's
//! `interactive_launch_action` and `app_reactivation` decide.

use std::cell::RefCell;
use std::time::SystemTime;

use captures_app::app_windows::{self, AppWindow, InteractiveLaunch, PrimaryWindow, Reactivation};
use captures_app::instance::{Instance, Launch, OpenRequest};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, HWND_MESSAGE, MB_ICONERROR, MB_OK, MessageBoxW, PostQuitMessage,
    PostThreadMessageW, SW_SHOWNORMAL, SetForegroundWindow, ShowWindow, WINDOW_EX_STYLE,
    WINDOW_STYLE, WM_APP,
};
use windows::core::{HSTRING, w};

use crate::Options;
use crate::tray::{Action, Tray};

/// Posted to the UI thread so its wait returns; carries no data.
const WM_APP_LIVE: u32 = WM_APP + 20;

struct Resident {
    options: Options,
    instance: Instance,
    /// `None` when the notification area is unavailable: like the other
    /// hosts, closing the last window then quits.
    tray: Option<Tray>,
    settings_modified: Option<SystemTime>,
    thread: u32,
    /// A message-only window that owns the clipboard for auto-copy.
    clipboard: Option<HWND>,
}

thread_local! {
    static RESIDENT: RefCell<Option<Resident>> = const { RefCell::new(None) };
}

/// Run before any window opens. `None` means another process owns this
/// profile and accepted the (bare) relaunch.
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
        match Instance::start(&root, OpenRequest::from_paths(Vec::new())?)? {
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
    RESIDENT.with(|r| {
        *r.borrow_mut() = Some(Resident {
            options: options.clone(),
            instance,
            tray,
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
    // The Win32 host has no first-run setup window yet; Preferences stands in.
    let launch =
        app_windows::interactive_launch(settings.onboarding_completed, options.quiet, false, false);
    match launch {
        // A quiet launch stays in the notification area. Shipping also shows
        // its 5 s launch notice there; that notice is not ported yet.
        Some(InteractiveLaunch::StartupNotice) if !tray_missing => {}
        Some(_) => open_preferences(options),
        None => {}
    }
}

/// Release the tray and the profile election.
pub fn stop() {
    RESIDENT.with(|r| r.borrow_mut().take());
}

fn open_preferences(options: &Options) {
    if let Err(error) = crate::host::open(options) {
        eprintln!("couldn't open Preferences: {error}");
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
    }
    match crate::capture::service() {
        Some(Ok(Some(artifact))) => captured(&options, artifact),
        Some(Ok(None)) | None => {}
        Some(Err(error)) => show_error(&error),
    }
    while let Some(action) = RESIDENT.with(|r| r.borrow().as_ref()?.tray.as_ref()?.next_action()) {
        run(&options, action);
    }
    for _ in 0..32 {
        let next = RESIDENT.with(|r| r.borrow().as_ref().map(|r| r.instance.next_request()));
        match next {
            Some(Ok(Some(request))) => {
                if !request.paths.is_empty() {
                    // Opening files by path is not ported to the Win32 host.
                    eprintln!("Opening files in the Win32 host is not supported yet.");
                }
                reactivate(&options);
            }
            Some(Err(error)) => {
                eprintln!("Native instance forwarding failed: {error}");
                break;
            }
            _ => break,
        }
    }
}

fn run(options: &Options, action: Action) {
    match action {
        Action::Preferences | Action::CheckUpdates | Action::LeftClick => open_preferences(options),
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
        Action::NewCapture => new_capture(options, crate::capture::Target::Region),
        Action::CaptureDisplay => new_capture(options, crate::capture::Target::Display),
        // Disabled rows: the direct overlays, recording and feedback are not
        // ported yet.
        Action::CaptureRegion
        | Action::CaptureWindow
        | Action::RecordRegion
        | Action::RecordWindow
        | Action::RecordDisplay
        | Action::SendFeedback => {}
    }
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
        _ => open_preferences(options),
    }
}

/// Open New Capture unless first-run setup is unfinished (Preferences stands
/// in for setup, as shipping opens setup instead).
fn new_capture(options: &Options, target: crate::capture::Target) {
    let settings = captures_settings::load(&options.settings_file).unwrap_or_default();
    if !settings.onboarding_completed {
        return open_preferences(options);
    }
    let Some(thread) = RESIDENT.with(|r| r.borrow().as_ref().map(|r| r.thread)) else {
        return;
    };
    if let Err(error) = crate::capture::open(options, target, wake_on(thread)) {
        show_error(&error);
    }
}

/// A saved screenshot: an open History reloads and auto-copy puts it on
/// the clipboard as PNG and `CF_DIB`.
fn captured(options: &Options, artifact: captures_app::Artifact) {
    crate::history::with_history(|h| h.reload());
    let settings = captures_settings::load(&options.settings_file).unwrap_or_default();
    if !settings.auto_copy_to_clipboard {
        return;
    }
    let Some(owner) = RESIDENT.with(|r| r.borrow().as_ref().and_then(|r| r.clipboard)) else {
        return;
    };
    let copied = std::fs::read(&artifact.image_path)
        .map_err(|e| e.to_string())
        .and_then(|png| {
            let image = image::load_from_memory(&png)
                .map_err(|e| e.to_string())?
                .into_rgba8();
            crate::editor::set_clipboard(owner, &png, &crate::editor::dib(&image))
                .map_err(|e| e.message().to_string())
        });
    if let Err(error) = copied {
        eprintln!("Could not copy the screenshot: {error}");
    }
}

/// Capture failures use a system message box until the native error surface
/// is ported.
fn show_error(message: &str) {
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
