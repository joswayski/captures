//! `--live`: Captures as the resident app rather than one fixture window.
//!
//! The first launch for a profile wins the shared `captures_app::instance`
//! election and stays running without a window; later launches forward
//! their files (or a bare relaunch) to it and exit before GTK starts.
//! Windows open as shipping's `interactive_launch_action` and
//! `app_reactivation` decide.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};

use captures_app::app_windows::{self, AppWindow, InteractiveLaunch, PrimaryWindow, Reactivation};
use captures_app::instance::{Instance, Launch, OpenRequest};
use captures_app::shortcuts::{CaptureShortcut, CaptureShortcuts};
use gtk::prelude::*;
use gtk::{gio, glib};

use crate::{Host, emit};

thread_local! {
    /// The resident host, for instance wakes and tray events scheduled from
    /// other threads.
    static HOST: RefCell<Weak<Host>> = RefCell::default();
    /// Watches the settings file so the tray and global shortcuts follow
    /// saved shortcuts.
    static SETTINGS: RefCell<Option<gio::FileMonitor>> = RefCell::default();
    /// The shared global capture shortcuts (X11 keys or the Wayland
    /// GlobalShortcuts portal), once setup is finished.
    static SHORTCUTS: RefCell<Option<CaptureShortcuts>> = RefCell::default();
}

/// A tray menu row, on the main loop. Until setup finishes, every row but
/// Quit shows setup, as shipping.
pub fn tray_action(action: crate::tray::Action) {
    if let Some(host) = HOST.with(|resident| resident.borrow().upgrade()) {
        if action != crate::tray::Action::Quit && !onboarded(&host) {
            host.show_setup();
            return;
        }
        crate::tray::run(&host, action);
    }
}

/// Run before GTK, workers or windows start. `None` means another process
/// already owns this profile and accepted `open` (empty for a relaunch).
pub fn elect(history_root: &Path, open: &[PathBuf]) -> std::io::Result<Option<Instance>> {
    let request = OpenRequest::from_paths(open.to_vec())?;
    Ok(match Instance::start(history_root, request)? {
        Launch::Primary(instance) => Some(instance),
        Launch::Forwarded => None,
    })
}

/// Make `host` the resident app: keep running with no window, receive
/// forwarded launches, and show the window shipping shows at launch.
pub fn start(host: &Rc<Host>, instance: Instance, quiet: bool, opening_files: bool) {
    *host.resident.borrow_mut() = Some(host.app.hold());
    HOST.with(|resident| *resident.borrow_mut() = Rc::downgrade(host));
    // The wake runs on the IPC worker; drain on the main loop, never inside
    // `set_wake` itself.
    instance.set_wake(|| {
        glib::MainContext::default().invoke(|| {
            glib::idle_add_local_once(drain);
        })
    });
    *host.instance.borrow_mut() = Some(instance);
    // Diagnostics belong to the elected primary only.
    match captures_app::crash::Session::start(&host.history_root) {
        Ok(session) => *host.crash.borrow_mut() = Some(std::sync::Arc::new(session)),
        Err(error) => eprintln!("{error} Capture remains available."),
    }
    let settings = captures_settings::load(&host.settings_file).unwrap_or_default();
    let tray = crate::tray::start(&settings);
    // Without a tray a windowless app is unreachable: open Preferences, and
    // let closing the last window quit, as the wgpu host.
    let tray_missing = tray.is_none();
    *host.tray.borrow_mut() = tray;
    if tray_missing {
        host.resident.borrow_mut().take();
    }
    sync_shortcuts(host, &settings);
    // Settings save atomically (a rename over the file): react to any event,
    // once per main-loop pass.
    let file = gio::File::for_path(&host.settings_file);
    if let Ok(monitor) =
        file.monitor_file(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE)
    {
        let weak = Rc::downgrade(host);
        let queued = Rc::new(std::cell::Cell::new(false));
        monitor.connect_changed(move |_, _, _, _| {
            if queued.replace(true) {
                return;
            }
            let (weak, queued) = (weak.clone(), queued.clone());
            glib::idle_add_local_once(move || {
                queued.set(false);
                if let Some(host) = weak.upgrade() {
                    let settings = captures_settings::load(&host.settings_file).unwrap_or_default();
                    if let Some(tray) = host.tray.borrow_mut().as_mut() {
                        tray.update(&settings);
                    }
                    sync_shortcuts(&host, &settings);
                }
            });
        });
        SETTINGS.with(|watch| *watch.borrow_mut() = Some(monitor));
    }
    let launch = app_windows::interactive_launch(onboarded(host), quiet, opening_files, false);
    // A visible launch after an unclean exit opens Send Feedback with the
    // local review, as the wgpu host; nothing is sent or added unasked.
    let review = !quiet
        && launch.is_some_and(|launch| launch != InteractiveLaunch::Setup)
        && host
            .crash
            .borrow()
            .as_ref()
            .is_some_and(|session| session.preview().is_some());
    emit(
        "launch",
        serde_json::json!({"action": launch.map(|launch| format!("{launch:?}")),
        "crashReview": review}),
    );
    if review {
        host.show_feedback();
    }
    match launch {
        Some(InteractiveLaunch::Setup) => {
            host.show_setup();
        }
        Some(InteractiveLaunch::Preferences | InteractiveLaunch::StartupNoticeAndPreferences) => {
            host.show_preferences();
        }
        Some(InteractiveLaunch::StartupNotice) if tray_missing => {
            host.show_preferences();
        }
        // The launch notice is a later slice; stay in the tray.
        Some(InteractiveLaunch::StartupNotice) | None => {}
    }
}

/// Stop accepting forwarded launches and release the profile.
pub fn stop(host: &Host) {
    SETTINGS.with(|watch| watch.borrow_mut().take());
    SHORTCUTS.with(|shortcuts| shortcuts.borrow_mut().take());
    host.tray.borrow_mut().take();
    // Mark a clean exit before releasing the profile election.
    if let Some(session) = host.crash.borrow_mut().take()
        && let Err(error) = session.clean_exit()
    {
        eprintln!("{error}");
    }
    host.instance.borrow_mut().take();
    host.resident.borrow_mut().take();
}

/// Register the saved capture shortcuts, or follow a change to them. Like
/// shipping, none are live until first-run setup finishes.
fn sync_shortcuts(host: &Rc<Host>, settings: &captures_settings::AppSettings) {
    SHORTCUTS.with(|cell| {
        let mut owner = cell.borrow_mut();
        if !settings.onboarding_completed {
            owner.take();
            return;
        }
        let wake = || {
            glib::MainContext::default().invoke(|| {
                glib::idle_add_local_once(drain_shortcuts);
            });
        };
        let result = match owner.as_mut() {
            Some(shortcuts) => shortcuts.update(settings),
            None if crate::capture::wayland(host) => CaptureShortcuts::new_wayland(settings, wake)
                .map(|shortcuts| {
                    *owner = Some(shortcuts);
                }),
            None => CaptureShortcuts::new(settings, wake).map(|shortcuts| {
                *owner = Some(shortcuts);
            }),
        };
        // Shipping logs system-key takeover failures and keeps its bindings.
        if let Some(shortcuts) = owner.as_mut() {
            for error in shortcuts.take_system_takeover().errors {
                eprintln!("{error}");
            }
        }
        emit(
            "shortcuts",
            serde_json::json!({"ok": result.is_ok(), "error": result.err()}),
        );
    });
}

/// Run queued shortcut presses on the main loop.
fn drain_shortcuts() {
    let Some(host) = HOST.with(|resident| resident.borrow().upgrade()) else {
        return;
    };
    while let Some(action) = SHORTCUTS.with(|cell| cell.borrow().as_ref()?.next_action()) {
        emit("shortcut", serde_json::json!({"action": action.id()}));
        match action {
            CaptureShortcut::Display => host.capture_display(),
            // Not in the GTK host yet; their tray rows are disabled too.
            CaptureShortcut::NewCapture
            | CaptureShortcut::Region
            | CaptureShortcut::Window
            | CaptureShortcut::RecordRegion
            | CaptureShortcut::RecordWindow
            | CaptureShortcut::RecordDisplay => {}
        }
    }
}

fn onboarded(host: &Host) -> bool {
    captures_settings::load(&host.settings_file)
        .unwrap_or_default()
        .onboarding_completed
}

/// Handle queued forwarded launches, at most 32 per main-loop pass.
fn drain() {
    let Some(host) = HOST.with(|resident| resident.borrow().upgrade()) else {
        return;
    };
    for _ in 0..32 {
        let next = {
            let instance = host.instance.borrow();
            let Some(instance) = instance.as_ref() else {
                return;
            };
            instance.next_request()
        };
        match next {
            Ok(Some(request)) if request.paths.is_empty() => {
                reactivate(&host);
                emit("instance-relaunch", serde_json::json!({}));
            }
            Ok(Some(request)) => {
                emit(
                    "instance-request",
                    serde_json::json!({"paths": request.paths.len()}),
                );
                host.open_media(request.paths);
            }
            Ok(None) => return,
            Err(error) => {
                host.show_error(&format!("Native instance forwarding failed: {error}"));
                return;
            }
        }
    }
    glib::idle_add_local_once(drain);
}

/// Shipping `app_reactivation` for a bare relaunch: focus the open window
/// with the highest priority (an editor before History before
/// Preferences), or open Preferences.
fn reactivate(host: &Rc<Host>) {
    let editors: Vec<gtk::ApplicationWindow> = host
        .editors
        .borrow()
        .iter()
        .map(|(_, editor)| editor.window.clone())
        .chain(
            host.recordings
                .borrow()
                .iter()
                .map(|(_, editor)| editor.window.clone()),
        )
        .collect();
    let mut visible: Vec<_> = (0..editors.len()).map(PrimaryWindow::Editor).collect();
    if host.setup.borrow().is_some() {
        visible.push(PrimaryWindow::App(AppWindow::Setup));
    }
    if host.history.borrow().is_some() {
        visible.push(PrimaryWindow::App(AppWindow::History));
    }
    if host.preferences.borrow().is_some() {
        visible.push(PrimaryWindow::App(AppWindow::Preferences));
    }
    match app_windows::reactivation(onboarded(host), false, &visible) {
        Reactivation::Focus(PrimaryWindow::Editor(index)) => editors[index].present(),
        Reactivation::Focus(PrimaryWindow::App(AppWindow::History)) => {
            host.show_history();
        }
        Reactivation::Focus(PrimaryWindow::App(AppWindow::Setup)) | Reactivation::ShowSetup => {
            host.show_setup();
        }
        Reactivation::Focus(PrimaryWindow::App(AppWindow::Preferences))
        | Reactivation::ShowPreferences
        | Reactivation::RestoreRecordingControls => {
            host.show_preferences();
        }
    }
}
