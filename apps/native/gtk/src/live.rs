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
use gtk::glib;
use gtk::prelude::*;

use crate::{Host, emit};

thread_local! {
    /// The resident host, for instance wakes scheduled from the IPC worker.
    static HOST: RefCell<Weak<Host>> = RefCell::default();
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
    let launch = app_windows::interactive_launch(onboarded(host), quiet, opening_files, false);
    emit(
        "launch",
        serde_json::json!({"action": launch.map(|launch| format!("{launch:?}"))}),
    );
    match launch {
        // First-run setup has no GTK window yet; Preferences stands in.
        Some(
            InteractiveLaunch::Setup
            | InteractiveLaunch::Preferences
            | InteractiveLaunch::StartupNoticeAndPreferences,
        ) => {
            host.show_preferences();
        }
        // The launch notice belongs to the tray slice; stay windowless.
        Some(InteractiveLaunch::StartupNotice) | None => {}
    }
}

/// Stop accepting forwarded launches and release the profile.
pub fn stop(host: &Host) {
    host.instance.borrow_mut().take();
    host.resident.borrow_mut().take();
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
        // Setup stands in as Preferences until the onboarding slice.
        Reactivation::Focus(PrimaryWindow::App(AppWindow::Preferences | AppWindow::Setup))
        | Reactivation::ShowSetup
        | Reactivation::ShowPreferences
        | Reactivation::RestoreRecordingControls => {
            host.show_preferences();
        }
    }
}
