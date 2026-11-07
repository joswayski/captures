//! Send Feedback (`?view=feedback`): shipping's `Feedback.tsx` and its
//! `.feedback*` rules. Copy, categories and limits come from
//! `captures_app::feedback`; context and sending from `captures-feedback`,
//! always on a worker. Nothing sends unless the host was given a feedback
//! endpoint; otherwise the form is a fixture whose Send stays off.
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, OnceLock};

use captures_app::feedback as copy;
use captures_feedback::{FeedbackClient, FeedbackContext, FeedbackDraft};
use gtk::prelude::*;
use gtk::{gio, glib};

use crate::preferences::{capped, checked, description, label, measure_ch, role_button};
use crate::tokens::Tokens;
use crate::widgets::{MaxWidth, WidthWatch};

/// One client per process, so the cooldown survives a closed form.
fn client(endpoint: &str) -> Result<Arc<FeedbackClient>, String> {
    static CLIENT: OnceLock<Result<Arc<FeedbackClient>, String>> = OnceLock::new();
    CLIENT
        .get_or_init(|| FeedbackClient::new(endpoint).map(Arc::new))
        .clone()
}

struct Form {
    endpoint: Option<String>,
    category: Cell<usize>,
    categories: Vec<gtk::Button>,
    message: gtk::TextView,
    placeholder: gtk::Label,
    contact: gtk::Entry,
    context: RefCell<Option<FeedbackContext>>,
    sending: Cell<bool>,
    send: gtk::Button,
    /// Shipping's footer note: sent (status), failure (alert) or why a
    /// fixture cannot send.
    sent: gtk::Label,
    error: gtk::Label,
    note: gtk::Label,
    /// Holds Send at the end while no note fills the row.
    spacer: gtk::Box,
}

pub fn build(
    app: &gtk::Application,
    tokens: &Tokens,
    endpoint: Option<String>,
) -> gtk::ApplicationWindow {
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title(copy::WINDOW_TITLE)
        .default_width(copy::WINDOW_WIDTH as i32)
        .default_height(copy::WINDOW_HEIGHT as i32)
        .build();
    window.add_css_class("captures");
    window.set_size_request(
        copy::WINDOW_MIN_WIDTH as i32,
        copy::WINDOW_MIN_HEIGHT as i32,
    );
    let space = |name: &str| tokens.number(name) as i32;
    measure_ch(tokens);

    // `.feedback-header`: eyebrow, title and intro.
    let header = gtk::Box::new(gtk::Orientation::Vertical, 0);
    header.append(&label(&copy::EYEBROW.to_uppercase(), "eyebrow"));
    let title = label(copy::TITLE, "feedback-title");
    title.set_accessible_role(gtk::AccessibleRole::Heading);
    header.append(&title);
    let intro = description(copy::INTRO, "help-text");
    intro.add_css_class("feedback-intro");
    header.append(&capped(&intro, 56.));

    // `.feedback-meta-card`: what is sent besides the form.
    let meta = gtk::Box::new(gtk::Orientation::Vertical, space("s-4"));
    meta.add_css_class("settings-card");
    meta.append(&label(copy::META_TITLE, "feedback-meta-title"));
    let rows = gtk::Grid::new();
    rows.set_row_spacing(space("s-3") as u32);
    rows.set_column_spacing(space("s-4") as u32);
    let mut values = Vec::new();
    for (row, name) in [copy::APP_VERSION_LABEL, copy::SYSTEM_LABEL]
        .into_iter()
        .enumerate()
    {
        let term = label(name, "feedback-meta-term");
        term.set_size_request(112, -1);
        let value = label(copy::LOADING, "feedback-meta-value");
        value.set_wrap(true);
        value.set_wrap_mode(gtk::pango::WrapMode::Char);
        value.set_hexpand(true);
        value.update_relation(&[gtk::accessible::Relation::LabelledBy(&[term.upcast_ref()])]);
        rows.attach(&term, 0, row as i32, 1, 1);
        rows.attach(&value, 1, row as i32, 1, 1);
        values.push(value);
    }
    // Shipping's `.settings-card > * + *` rule.
    rows.add_css_class("setting-row");
    meta.append(&rows);

    // The form card: category, message and contact.
    let form_card = gtk::Box::new(gtk::Orientation::Vertical, space("s-5"));
    form_card.add_css_class("settings-card");
    let field = || gtk::Box::new(gtk::Orientation::Vertical, space("s-3"));

    let category_field = field();
    let category_label = label(copy::CATEGORY_LABEL, "field-label");
    category_field.append(&category_label);
    let grid = glib::Object::builder::<gtk::Grid>()
        .property("accessible-role", gtk::AccessibleRole::RadioGroup)
        .build();
    grid.update_property(&[gtk::accessible::Property::Label("Feedback category")]);
    grid.set_column_homogeneous(true);
    grid.set_column_spacing(space("s-3") as u32);
    grid.set_row_spacing(space("s-3") as u32);
    let categories: Vec<gtk::Button> = copy::CATEGORIES
        .iter()
        .map(|category| {
            let button = role_button(gtk::AccessibleRole::Radio);
            button.add_css_class("feedback-category");
            let body = gtk::Box::new(gtk::Orientation::Vertical, 3);
            body.append(&label(category.label, "feedback-category-name"));
            let detail = description(category.description, "feedback-category-detail");
            body.append(&detail);
            body.set_valign(gtk::Align::Start);
            button.set_child(Some(&body));
            button.update_property(&[
                gtk::accessible::Property::Label(category.label),
                gtk::accessible::Property::Description(category.description),
            ]);
            button
        })
        .collect();
    category_field.append(&grid);
    form_card.append(&category_field);

    let message_field = field();
    let message_label = label(copy::MESSAGE_LABEL, "field-label");
    message_field.append(&message_label);
    let message = gtk::TextView::new();
    message.add_css_class("feedback-message");
    message.set_wrap_mode(gtk::WrapMode::WordChar);
    message.set_accepts_tab(false);
    message.update_relation(&[gtk::accessible::Relation::LabelledBy(&[
        message_label.upcast_ref()
    ])]);
    let placeholder = label("", "feedback-placeholder");
    placeholder.set_valign(gtk::Align::Start);
    placeholder.set_halign(gtk::Align::Start);
    placeholder.set_can_target(false);
    placeholder.set_accessible_role(gtk::AccessibleRole::Presentation);
    let message_box = gtk::Overlay::new();
    message_box.set_child(Some(&message));
    message_box.add_overlay(&placeholder);
    message_field.append(&message_box);
    message_field.add_css_class("setting-row");
    form_card.append(&message_field);

    let contact_field = field();
    // `margin-left: var(--s-2)` after the label's trailing space.
    let contact_title = gtk::Box::new(gtk::Orientation::Horizontal, space("s-4"));
    let contact_label = label(copy::CONTACT_LABEL, "field-label");
    contact_title.append(&contact_label);
    let optional = label(&copy::OPTIONAL_BADGE.to_uppercase(), "feedback-optional");
    optional.set_valign(gtk::Align::Center);
    contact_title.append(&optional);
    contact_field.append(&contact_title);
    let contact = gtk::Entry::new();
    contact.add_css_class("feedback-contact");
    contact.set_placeholder_text(Some(copy::CONTACT_PLACEHOLDER));
    contact.set_max_length(copy::CONTACT_LIMIT as i32);
    contact.set_input_hints(gtk::InputHints::NO_SPELLCHECK);
    contact.update_relation(&[gtk::accessible::Relation::LabelledBy(&[
        contact_label.upcast_ref()
    ])]);
    contact.update_property(&[gtk::accessible::Property::Description(copy::CONTACT_HELP)]);
    contact_field.add_css_class("setting-row");
    contact_field.append(&contact);
    contact_field.append(&description(copy::CONTACT_HELP, "help-text"));
    form_card.append(&contact_field);

    // `.feedback-actions`: the note fills the row before Send.
    let actions = gtk::Box::new(gtk::Orientation::Horizontal, space("s-5"));
    let note = |class: &str, role: gtk::AccessibleRole| {
        let text = description("", "feedback-status");
        text.add_css_class(class);
        text.set_hexpand(true);
        text.set_visible(false);
        text.set_accessible_role(role);
        actions.append(&text);
        text
    };
    let error = note("feedback-status-error", gtk::AccessibleRole::Alert);
    let sent = note("feedback-status-ok", gtk::AccessibleRole::Status);
    let fixture = note("feedback-status-note", gtk::AccessibleRole::Status);
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    actions.append(&spacer);
    let send = gtk::Button::with_label(copy::SEND);
    send.add_css_class("ui-btn");
    send.add_css_class("primary");
    send.add_css_class("feedback-submit");
    send.set_valign(gtk::Align::Center);
    actions.append(&send);

    let shell = gtk::Box::new(gtk::Orientation::Vertical, space("s-6"));
    for part in [
        header.upcast_ref::<gtk::Widget>(),
        meta.upcast_ref(),
        form_card.upcast_ref(),
        actions.upcast_ref(),
    ] {
        shell.append(part);
    }
    let page = gtk::Box::new(gtk::Orientation::Vertical, 0);
    page.add_css_class("feedback");
    page.set_vexpand(true);
    page.append(&MaxWidth::centered(&shell, copy::SHELL_MAX_WIDTH as i32));
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&page)
        .build();
    // Shipping's `@media (max-width: 720px)`: two category columns.
    let columns: Rc<Cell<i32>> = Rc::default();
    let watch = WidthWatch::new(&scroller, {
        let (grid, categories) = (grid.clone(), categories.clone());
        move |width| {
            let count = if captures_app::app_windows::compact(width as f32) {
                2
            } else {
                3
            };
            if columns.replace(count) != count {
                let (grid, categories) = (grid.clone(), categories.clone());
                glib::idle_add_local_once(move || place_categories(&grid, &categories, count));
            }
        }
    });
    place_categories(&grid, &categories, 2);
    window.set_child(Some(&watch));

    let form = Rc::new(Form {
        endpoint,
        category: Cell::new(0),
        categories,
        message,
        placeholder,
        contact,
        context: RefCell::default(),
        sending: Cell::new(false),
        send,
        sent,
        error,
        note: fixture,
        spacer,
    });
    for (index, button) in form.categories.iter().enumerate() {
        let weak = Rc::downgrade(&form);
        button.connect_clicked(move |_| {
            if let Some(form) = weak.upgrade() {
                form.category.set(index);
                form.refresh();
            }
        });
    }
    let weak = Rc::downgrade(&form);
    form.message.buffer().connect_changed(move |buffer| {
        let Some(form) = weak.upgrade() else {
            return;
        };
        // `maxLength`: never hold more than the backend accepts.
        let mut text: String = buffer
            .text(&buffer.start_iter(), &buffer.end_iter(), false)
            .into();
        if copy::clamp(&mut text, copy::MESSAGE_LIMIT) {
            buffer.set_text(&text);
            return;
        }
        // Editing after a result clears it, as shipping returns to idle.
        form.sent.set_visible(false);
        form.error.set_visible(false);
        form.refresh();
    });
    let weak = Rc::downgrade(&form);
    form.contact.connect_changed(move |_| {
        if let Some(form) = weak.upgrade() {
            form.refresh();
        }
    });
    let weak = Rc::downgrade(&form);
    form.send.connect_clicked(move |_| {
        if let Some(form) = weak.upgrade() {
            form.submit();
        }
    });
    form.refresh();
    // Shipping opens with nothing focused; GTK would focus the message.
    window.connect_map(|window| {
        let window = window.downgrade();
        glib::idle_add_local_once(move || {
            if let Some(window) = window.upgrade() {
                gtk::prelude::GtkWindowExt::set_focus(&window, None::<&gtk::Widget>);
            }
        });
    });
    // The window owns the form; its handlers hold weak references.
    let owner = form.clone();
    window.connect_destroy(move |_| {
        let _ = &owner;
    });

    // Shipping loads the context after the window shows.
    let weak = Rc::downgrade(&form);
    glib::spawn_future_local(async move {
        let Ok(context) = gio::spawn_blocking(captures_feedback::native::context).await else {
            return;
        };
        let Some(form) = weak.upgrade() else {
            return;
        };
        values[0].set_text(&context.app_version);
        values[1].set_text(&copy::system_label(
            &context.os,
            &context.os_version,
            &context.arch,
        ));
        *form.context.borrow_mut() = Some(context);
        form.refresh();
    });
    window
}

/// Fill the radio group row by row in `columns` columns.
fn place_categories(grid: &gtk::Grid, categories: &[gtk::Button], columns: i32) {
    for (index, button) in categories.iter().enumerate() {
        if button.parent().is_some() {
            grid.remove(button);
        }
        let index = index as i32;
        grid.attach(button, index % columns, index / columns, 1, 1);
    }
}

impl Form {
    fn text(&self) -> String {
        let buffer = self.message.buffer();
        buffer
            .text(&buffer.start_iter(), &buffer.end_iter(), false)
            .into()
    }

    fn refresh(&self) {
        let selected = self.category.get();
        for (index, button) in self.categories.iter().enumerate() {
            checked(button, index == selected);
        }
        let message = self.text();
        let placeholder = copy::CATEGORIES[selected].placeholder;
        self.placeholder.set_text(placeholder);
        self.placeholder.set_visible(message.is_empty());
        self.message
            .update_property(&[gtk::accessible::Property::Placeholder(placeholder)]);
        let sending = self.sending.get();
        self.message.set_sensitive(!sending);
        self.contact.set_sensitive(!sending);
        self.send
            .set_label(if sending { copy::SENDING } else { copy::SEND });
        let ready = self.endpoint.is_some() && self.context.borrow().is_some();
        self.send
            .set_sensitive(ready && copy::can_submit(&message, &self.contact.text(), sending));
        if self.endpoint.is_none() {
            self.note.set_text(copy::FIXTURE_DISABLED);
        }
        self.note.set_visible(
            self.endpoint.is_none() && !self.sent.get_visible() && !self.error.get_visible(),
        );
        self.spacer.set_visible(
            !self.note.get_visible() && !self.sent.get_visible() && !self.error.get_visible(),
        );
    }

    fn submit(self: &Rc<Self>) {
        let (Some(endpoint), Some(context)) =
            (self.endpoint.clone(), self.context.borrow().clone())
        else {
            return;
        };
        let message = self.text();
        if !copy::can_submit(&message, &self.contact.text(), self.sending.get()) {
            return;
        }
        let contact = self.contact.text().trim().to_owned();
        let draft = FeedbackDraft {
            message: message.trim().to_owned(),
            contact: (!contact.is_empty()).then_some(contact),
            category: copy::CATEGORIES[self.category.get()].id.to_owned(),
        };
        self.sending.set(true);
        self.sent.set_visible(false);
        self.error.set_visible(false);
        self.refresh();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = gio::spawn_blocking(move || client(&endpoint)?.submit(draft, context))
                .await
                .unwrap_or_else(|_| Err("Feedback could not be sent.".into()));
            let Some(form) = weak.upgrade() else {
                return;
            };
            form.sending.set(false);
            match result {
                Ok(()) => {
                    // Shipping clears the message once it is sent.
                    form.message.buffer().set_text("");
                    form.sent.set_text(copy::SENT);
                    form.sent.set_visible(true);
                }
                Err(message) => {
                    form.error.set_text(&message);
                    form.error.set_visible(true);
                }
            }
            form.refresh();
        });
    }
}
