//! User-driven recovery of privately retained native recording sessions.
use crate::compat::prelude::*;
use crate::{recording, ui};
use gtk::prelude::*;
use std::{path::PathBuf, rc::Rc};

pub fn open(directory: PathBuf, saved: Rc<dyn Fn(PathBuf)>) {
    const TITLE: &str = "Captures — Recover recordings";
    for window in gtk::Window::list_toplevels()
        .into_iter()
        .filter_map(|w| w.downcast::<gtk::Window>().ok())
    {
        if window.title().as_deref() == Some(TITLE) {
            window.present();
            return;
        }
    }
    let drafts = match recording::list_recording_drafts(&directory) {
        Ok(drafts) => drafts,
        Err(error) => {
            ui::error(&gtk::Window::new(), &error);
            return;
        }
    };
    if drafts.is_empty() {
        return;
    }
    let window = gtk::Window::new();
    window.set_title(Some(TITLE));
    window.set_default_size(620, 440);
    let root = gtk::Box::new(gtk::Orientation::Vertical, 16);
    root.set_border_width(24);
    root.add(&ui::label("Recover unfinished recordings", "title"));
    let help = ui::label(
        "Completed segments survived an interrupted session. Recovery saves a new file; an incomplete final segment may be unavailable.",
        "muted",
    );
    help.set_wrap(true);
    root.add(&help);
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_vexpand(true);
    let list = gtk::Box::new(gtk::Orientation::Vertical, 12);
    scroll.add(&list);
    root.pack_start(&scroll, true, true, 0);
    for draft in drafts {
        let row = gtk::Box::new(gtk::Orientation::Vertical, 8);
        row.style_context().add_class("settings-card");
        row.set_border_width(12);
        row.add(&ui::label(
            &format!(
                "{:?} · {} completed segments",
                draft.options.kind,
                draft.segments.iter().filter(|s| s.complete).count()
            ),
            "section-title",
        ));
        let status = ui::label(
            draft.last_error.as_deref().unwrap_or("Ready to recover"),
            "muted",
        );
        status.set_wrap(true);
        row.add(&status);
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let recover = ui::button("Recover recording");
        recover.style_context().add_class("primary");
        let discard = ui::button("Discard draft");
        actions.add(&recover);
        actions.add(&discard);
        row.add(&actions);
        list.add(&row);
        {
            let (directory, id, saved, actions, row, status) = (
                directory.clone(),
                draft.session_id.clone(),
                saved.clone(),
                actions.clone(),
                row.clone(),
                status.clone(),
            );
            recover.connect_clicked(move |_| {
                actions.set_sensitive(false);
                status.set_text(
                    "Recovering… Your source segments remain intact until the new file is saved.",
                );
                let (directory, id, saved, actions, row, status) = (
                    directory.clone(),
                    id.clone(),
                    saved.clone(),
                    actions.clone(),
                    row.clone(),
                    status.clone(),
                );
                ui::job(
                    move || recording::recover_recording_draft(&directory, &id),
                    move |result| match result {
                        Ok(path) => {
                            row.hide();
                            saved(path);
                        }
                        Err(error) => {
                            status.set_text(&error);
                            actions.set_sensitive(true);
                        }
                    },
                );
            });
        }
        {
            let (directory, id, row, status, window) = (
                directory.clone(),
                draft.session_id,
                row.clone(),
                status.clone(),
                window.clone(),
            );
            discard.connect_clicked(move |_| {
                let (directory, id, row, status, window) = (
                    directory.clone(),
                    id.clone(),
                    row.clone(),
                    status.clone(),
                    window.clone(),
                );
                ui::confirm(
                    &window,
                    "Discard unfinished recording?",
                    "Its unsaved segments cannot be recovered afterward.",
                    "Discard",
                    move || match recording::discard_recording_draft(&directory, &id) {
                        Ok(()) => row.hide(),
                        Err(error) => status.set_text(&error),
                    },
                );
            });
        }
    }
    let later = ui::button("Later");
    {
        let window = window.clone();
        later.connect_clicked(move |_| window.close());
    }
    root.add(&later);
    window.add(&root);
    window.show_all();
    window.present();
}
