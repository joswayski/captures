//! Explicit feedback UI. Nothing is collected or sent until Send is chosen.
use crate::{compat::prelude::*, ui};
use captures_feedback::{FeedbackClient, FeedbackContext, FeedbackDraft};
use gtk::prelude::*;
use std::{cell::RefCell, rc::Rc, sync::Arc};

pub fn open(application: &gtk::Application, client: Result<Arc<FeedbackClient>, Arc<str>>) {
    if let Some(existing) = gtk::Window::list_toplevels()
        .into_iter()
        .filter_map(|window| window.downcast::<gtk::Window>().ok())
        .find(|window| window.title().as_deref() == Some("Send feedback to Captures"))
    {
        existing.present();
        return;
    }

    let window = gtk::Window::new();
    application.add_window(&window);
    window.set_title(Some("Send feedback to Captures"));
    window.set_default_size(620, 600);
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.style_context().add_class("feedback-window");
    let header = gtk::Box::new(gtk::Orientation::Vertical, 6);
    header.style_context().add_class("page-header");
    header.pack_start(&ui::label("Send feedback", "title"), false, false, 0);
    header.pack_start(
        &ui::label(
            "Share a bug or idea. Captures sends only the fields shown below plus app and OS version.",
            "muted",
        ),
        false,
        false,
        0,
    );
    root.append(&header);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    content.set_margin_top(24);
    content.set_margin_bottom(24);
    content.set_margin_start(28);
    content.set_margin_end(28);
    let category = Rc::new(RefCell::new(String::from("bug")));
    content.append(&ui::label("Category", "section-title"));
    let choices = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let mut choices_buttons = Vec::new();
    let mut first_choice: Option<gtk::ToggleButton> = None;
    for (id, label) in [("bug", "Bug"), ("idea", "Idea"), ("other", "Other")] {
        let button = gtk::ToggleButton::with_label(label);
        if let Some(first) = &first_choice {
            button.set_group(Some(first));
        } else {
            first_choice = Some(button.clone());
        }
        button.set_active(id == "bug");
        let category = category.clone();
        let id = id.to_owned();
        button.connect_toggled(move |clicked| {
            if clicked.is_active() {
                *category.borrow_mut() = id.clone();
            }
        });
        choices_buttons.push(button.clone());
        choices.append(&button);
    }
    content.append(&choices);
    content.append(&ui::label(
        "What happened, or what should improve?",
        "section-title",
    ));
    let message = gtk::TextView::new();
    ui::named(&message, "Feedback message");
    message.set_wrap_mode(gtk::WrapMode::WordChar);
    message.set_top_margin(10);
    message.set_bottom_margin(10);
    message.set_left_margin(12);
    message.set_right_margin(12);
    message.set_vexpand(true);
    message.style_context().add_class("feedback-message");
    content.append(&message);
    content.append(&ui::label("Contact (optional)", "section-title"));
    let contact = gtk::Entry::new();
    ui::named(&contact, "Feedback contact");
    contact.set_placeholder_text(Some("Email or another way to follow up"));
    content.append(&contact);
    let privacy = ui::label(
        "No screenshots, recordings, logs, or crash details are attached.",
        "muted",
    );
    privacy.set_wrap(true);
    content.append(&privacy);
    let status = ui::label("", "muted");
    ui::named(&status, "Feedback status");
    content.append(&status);
    root.append(&content);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.set_halign(gtk::Align::End);
    actions.style_context().add_class("notice-actions");
    actions.set_margin_start(28);
    actions.set_margin_end(28);
    actions.set_margin_bottom(24);
    let cancel = ui::button("Cancel");
    let send = ui::icon_text_button("Send feedback", "check");
    send.style_context().add_class("primary");
    actions.append(&cancel);
    actions.append(&send);
    root.append(&actions);
    window.set_child(Some(&root));
    cancel.connect_clicked({
        let window = window.clone();
        move |_| window.close()
    });
    send.connect_clicked({
        let choices_buttons = choices_buttons.clone();
        let cancel = cancel.clone();
        move |send| {
            let client = match client.clone() {
                Ok(client) => client,
                Err(error) => {
                    status.set_text(&error);
                    return;
                }
            };
            let buffer = message.buffer();
            let draft = FeedbackDraft {
                message: buffer
                    .text(&buffer.start_iter(), &buffer.end_iter(), false)
                    .to_string(),
                contact: Some(contact.text().to_string()),
                category: category.borrow().clone(),
            };
            let context = FeedbackContext {
                app_version: env!("CARGO_PKG_VERSION").into(),
                os: "linux".into(),
                os_version: os_version(),
                arch: std::env::consts::ARCH.into(),
            };
            for button in &choices_buttons {
                button.set_sensitive(false);
            }
            message.set_editable(false);
            contact.set_editable(false);
            send.set_sensitive(false);
            status.set_text("Sending…");
            let send = send.clone();
            let status = status.clone();
            let cancel = cancel.clone();
            let choices_buttons = choices_buttons.clone();
            let message = message.clone();
            let contact = contact.clone();
            ui::job(
                move || client.submit(draft, context),
                move |result| match result {
                    Ok(()) => {
                        status.set_text("Thanks — your feedback was sent.");
                        cancel.set_label("Close");
                    }
                    Err(error) => {
                        status.set_text(&error);
                        send.set_sensitive(true);
                        for button in &choices_buttons {
                            button.set_sensitive(true);
                        }
                        message.set_editable(true);
                        contact.set_editable(true);
                    }
                },
            );
        }
    });
    window.present();
}

fn os_version() -> String {
    std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|contents| {
            contents.lines().find_map(|line| {
                line.strip_prefix("PRETTY_NAME=")
                    .map(|value| value.trim_matches('"').to_owned())
            })
        })
        .unwrap_or_else(|| "Linux".into())
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_nonempty_os_version() {
        assert!(!super::os_version().is_empty());
    }
}
