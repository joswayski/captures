//! Preferences' Updates card on the native Preview channel only. An
//! installed Preview build (`CAPTURES_NATIVE_PREVIEW_VERSION`) checks the
//! signed Preview metadata through the shared `CheckWorker::preview`, which
//! keeps the existing full and delta acquisition; development builds keep
//! the static "Updates unavailable" copy and never make a request. There is
//! no development endpoint, production or stable channel here.
//!
//! "Restart and install" shows disabled with a note: the Linux Preview
//! package is still the wgpu build, so installing it would replace this host.

/// Why a verified Preview cannot install from this host.
pub const INSTALL_NOTE: &str =
    "Installing is not available in this native build. Download the latest Preview from captur.es.";
use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::sync::Arc;

use captures_app::preferences;
use captures_app::update_notice::Action;
use captures_app::updater::checks::{CheckWorker, Presentation};
use gtk::glib;
use gtk::prelude::*;

thread_local! {
    /// The open card, woken on the main thread by the worker.
    static OPEN: RefCell<Weak<Updates>> = const { RefCell::new(Weak::new()) };
}

pub struct Updates {
    worker: RefCell<Option<CheckWorker>>,
    /// Shown when no worker could start (a Preview build without its key).
    error: Option<String>,
    version: gtk::Label,
    channel: gtk::Label,
    check: gtk::Button,
    status: gtk::Label,
    acquisition: gtk::Box,
    acquire: gtk::Button,
    note: gtk::Label,
}

impl Drop for Updates {
    fn drop(&mut self) {
        // Joining can wait on a blocked request; never on the UI thread.
        if let Some(worker) = self.worker.take() {
            std::thread::spawn(move || drop(worker));
        }
    }
}

pub struct Widgets {
    pub version: gtk::Label,
    pub channel: gtk::Label,
    pub check: gtk::Button,
    pub status: gtk::Label,
    /// The "Package verification" row and its button.
    pub acquisition: gtk::Box,
    pub acquire: gtk::Button,
    /// Shown under the row while install is unavailable.
    pub note: gtk::Label,
}

/// Start the Preview worker for an installed Preview build and keep the
/// card's widgets in step with it.
pub fn connect(widgets: Widgets) -> Rc<Updates> {
    let (worker, error) = if captures_app::preview_channel::version().is_some() {
        // The worker wakes from its own thread; hop to the main loop.
        let wake = Arc::new(|| {
            glib::MainContext::default().invoke(|| {
                if let Some(updates) = OPEN.with(|open| open.borrow().upgrade()) {
                    updates.refresh();
                }
            });
        });
        // `ReleaseInfo` labels the Linux Preview package, still wgpu-built.
        match CheckWorker::preview(captures_app::updater::Renderer::Wgpu, wake) {
            Ok(mut worker) => {
                worker.check_automatically();
                (Some(worker), None)
            }
            Err(error) => (None, Some(error.to_string())),
        }
    } else {
        (None, None)
    };
    let updates = Rc::new(Updates {
        worker: RefCell::new(worker),
        error,
        version: widgets.version,
        channel: widgets.channel,
        check: widgets.check,
        status: widgets.status,
        acquisition: widgets.acquisition,
        acquire: widgets.acquire,
        note: widgets.note,
    });
    OPEN.with(|open| *open.borrow_mut() = Rc::downgrade(&updates));
    let weak = Rc::downgrade(&updates);
    updates.check.connect_clicked(move |_| {
        if let Some(updates) = weak.upgrade() {
            if let Some(worker) = updates.worker.borrow_mut().as_mut() {
                worker.check();
            }
            updates.refresh();
        }
    });
    let weak = Rc::downgrade(&updates);
    updates.acquire.connect_clicked(move |_| {
        let Some(updates) = weak.upgrade() else {
            return;
        };
        let action = updates.presentation().and_then(|copy| copy.acquisition);
        if let (Some(worker), Some(button)) = (updates.worker.borrow_mut().as_mut(), action) {
            match button.action {
                Action::DownloadVerify => {
                    worker.download();
                }
                Action::CancelDownload => {
                    worker.cancel_download();
                }
                _ => {}
            }
        }
        updates.refresh();
    });
    updates.refresh();
    updates
}

impl Updates {
    fn presentation(&self) -> Option<Presentation> {
        let mut worker = self.worker.borrow_mut();
        let worker = worker.as_mut()?;
        worker.poll();
        Some(worker.presentation())
    }

    fn refresh(&self) {
        let copy = self.presentation();
        let (version, channel, status, action, detail, enabled, failed) = match &copy {
            Some(copy) => (
                copy.version.as_str(),
                copy.channel,
                copy.status.as_str(),
                copy.action,
                copy.detail,
                copy.enabled,
                copy.failed,
            ),
            None => (
                preferences::UPDATES_VERSION,
                preferences::UPDATES_CHANNEL,
                self.error.as_deref().unwrap_or(preferences::UPDATES_TITLE),
                preferences::UPDATES_ACTION,
                preferences::UPDATES_DETAIL,
                false,
                self.error.is_some(),
            ),
        };
        self.version.set_text(version);
        self.channel.set_text(channel);
        self.status.set_text(status);
        if failed {
            self.status.add_css_class("failed");
        } else {
            self.status.remove_css_class("failed");
        }
        self.check.set_label(action);
        self.check.set_sensitive(enabled);
        self.check.set_tooltip_text(Some(detail));
        self.check
            .update_property(&[gtk::accessible::Property::Description(detail)]);
        let button = copy.and_then(|copy| copy.acquisition);
        let install = button
            .as_ref()
            .is_some_and(|button| button.action == Action::Install);
        self.acquisition.set_visible(button.is_some());
        self.note.set_visible(install);
        if let Some(button) = button {
            if let Some(label) = self.acquire.child().and_downcast::<gtk::Label>() {
                label.set_text(&button.label);
            }
            self.acquire
                .update_property(&[gtk::accessible::Property::Label(&button.label)]);
            self.acquire.set_sensitive(button.enabled && !install);
            self.acquire
                .update_property(&[gtk::accessible::Property::Description(if install {
                    INSTALL_NOTE
                } else {
                    ""
                })]);
        }
    }
}
