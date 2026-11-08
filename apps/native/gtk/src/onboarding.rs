//! First-run setup (`?view=onboarding`): shipping's `Onboarding.tsx` and
//! its `.onboarding*` rules. Copy and per-state presentation come from
//! `captures_app::onboarding`, permission checks and completion from its
//! `Session`, always on a worker.

use std::rc::Rc;
use std::sync::{Arc, Mutex};

use captures_app::onboarding::{self as copy, Action, Presentation, Session, State};
use gtk::prelude::*;
use gtk::{gio, glib};

use crate::preferences::{capped, description, label, measure_ch};
use crate::tokens::Tokens;
use crate::widgets::{Icon, MaxWidth};

struct Setup {
    settings_file: std::path::PathBuf,
    session: Arc<Mutex<Session>>,
    window: gtk::ApplicationWindow,
    title: gtk::Label,
    screen_description: gtk::Label,
    /// The status pill or action beside the screen row.
    screen_side: gtk::Box,
    error: gtk::Label,
    primary: gtk::Button,
    state: std::cell::RefCell<Option<Presentation>>,
    busy: std::cell::Cell<bool>,
    finished: Box<dyn Fn()>,
}

/// The setup window. `finished` runs after setup completes and the window
/// closes.
pub fn build(
    app: &gtk::Application,
    tokens: &Tokens,
    settings_file: std::path::PathBuf,
    finished: impl Fn() + 'static,
) -> gtk::ApplicationWindow {
    let spec = captures_app::app_windows::SETUP;
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title(spec.title)
        .default_width(spec.width as i32)
        .default_height(spec.height as i32)
        .build();
    window.add_css_class("captures");
    window.set_size_request(spec.min_width as i32, spec.min_height as i32);
    measure_ch(tokens);
    let text = copy::copy();

    // `.onboarding-copy`: the accent mark, eyebrow, title and lede.
    let header = gtk::Box::new(gtk::Orientation::Vertical, 0);
    header.add_css_class("onboarding-copy");
    let mark = gtk::Box::new(gtk::Orientation::Vertical, 0);
    mark.add_css_class("onboarding-mark");
    mark.set_halign(gtk::Align::Start);
    let glyph = Icon::new("capture", 24).with_stroke(1.7);
    glyph.set_halign(gtk::Align::Center);
    glyph.set_valign(gtk::Align::Center);
    glyph.set_vexpand(true);
    mark.append(&glyph);
    header.append(&mark);
    header.append(&label(&text.eyebrow.to_uppercase(), "eyebrow"));
    let title = label(text.title, "onboarding-title");
    title.set_accessible_role(gtk::AccessibleRole::Heading);
    header.append(&title);
    let lede = description(text.lede, "onboarding-lede");
    // `max-width: 54ch` at `--text-md`; `capped` measures `ch` at `--text-sm`.
    let lede_cap = capped(
        &lede,
        54. * tokens.number("text-md") / tokens.number("text-sm"),
    );
    header.append(&lede_cap);

    // `.onboarding-permissions`: one card with the screen row (macOS adds a
    // microphone row; no other platform shows it).
    let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
    card.add_css_class("onboarding-permissions");
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    row.add_css_class("onboarding-permission");
    let icon = Icon::new("screen-access", 20).with_stroke(1.7);
    icon.add_css_class("onboarding-permission-icon");
    icon.set_valign(gtk::Align::Start);
    row.append(&icon);
    let copy_column = gtk::Box::new(gtk::Orientation::Vertical, 0);
    copy_column.set_hexpand(true);
    let heading = label(text.screen_title, "onboarding-permission-title");
    heading.set_accessible_role(gtk::AccessibleRole::Heading);
    copy_column.append(&heading);
    let screen_description = description(text.checking, "onboarding-permission-description");
    copy_column.append(&capped(&screen_description, 44.));
    row.append(&copy_column);
    let screen_side = gtk::Box::new(gtk::Orientation::Vertical, 0);
    screen_side.add_css_class("onboarding-permission-actions");
    screen_side.set_valign(gtk::Align::Start);
    row.append(&screen_side);
    card.append(&row);

    let error = description("", "onboarding-error");
    error.set_accessible_role(gtk::AccessibleRole::Alert);
    error.set_visible(false);

    let primary = gtk::Button::with_label(text.start);
    crate::widgets::action_cursor(&primary);
    primary.add_css_class("onboarding-primary-button");
    primary.set_halign(gtk::Align::End);
    primary.set_sensitive(false);

    let panel = gtk::Box::new(gtk::Orientation::Vertical, 0);
    panel.add_css_class("onboarding-panel");
    panel.append(&card);
    panel.append(&error);
    panel.append(&primary);

    // `.onboarding-stage`: centered, at most 620 px wide.
    let stage = gtk::Box::new(gtk::Orientation::Vertical, 0);
    stage.add_css_class("onboarding-stage");
    stage.set_valign(gtk::Align::Center);
    stage.set_vexpand(true);
    stage.append(&header);
    stage.append(&panel);
    let shell = MaxWidth::centered(&stage, spec.width as i32);
    shell.add_css_class("onboarding-shell");
    window.set_child(Some(&shell));

    // Shipping `@media (max-height: 600px)`: tighter stage, no lede. The
    // default 560 px window is always short.
    let short = {
        let (stage, lede_cap) = (stage.clone(), lede_cap.clone());
        move |height: i32| {
            let short = captures_app::app_windows::short(height as f32);
            if short {
                stage.add_css_class("short");
            } else {
                stage.remove_css_class("short");
            }
            lede_cap.set_visible(!short);
        }
    };
    short(spec.height as i32);
    window.connect_default_height_notify(move |window| short(window.default_height()));

    let setup = Rc::new(Setup {
        settings_file,
        session: Arc::new(Mutex::new(Session::new())),
        window: window.clone(),
        title,
        screen_description,
        screen_side,
        error,
        primary: primary.clone(),
        state: Default::default(),
        busy: Default::default(),
        finished: Box::new(finished),
    });
    primary.connect_clicked({
        let setup = Rc::downgrade(&setup);
        move |_| {
            if let Some(setup) = setup.upgrade() {
                setup.run(Action::Complete);
            }
        }
    });
    // The window owns the setup state until it is destroyed.
    window.connect_destroy({
        let setup = setup.clone();
        move |_| {
            let _ = &setup;
        }
    });
    setup.run(Action::Check);
    window
}

impl Setup {
    /// One shared onboarding action on a worker; the reply updates the page.
    fn run(self: &Rc<Self>, action: Action) {
        if self.busy.replace(true) {
            return;
        }
        let text = copy::copy();
        if action == Action::Complete {
            self.primary.set_label(text.finishing);
        }
        self.sync_sensitivity();
        let (path, session) = (self.settings_file.clone(), self.session.clone());
        let setup = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = gio::spawn_blocking(move || {
                session
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .execute(&path, action)
            })
            .await
            .unwrap_or_else(|_| Err("the setup worker stopped".into()));
            let Some(setup) = setup.upgrade() else { return };
            setup.busy.set(false);
            match (action, result) {
                (Action::Complete, Ok(_)) => {
                    crate::emit("setup-complete", serde_json::json!({}));
                    setup.window.close();
                    (setup.finished)();
                }
                (_, Ok(state)) => setup.show(&state),
                (Action::Complete, Err(error)) => {
                    setup.primary.set_label(text.start);
                    setup.fail(&format!("Couldn’t finish setup: {error}"));
                }
                (_, Err(error)) => {
                    setup.fail(&format!("Couldn’t check setup status: {error}"));
                }
            }
        });
    }

    fn fail(&self, error: &str) {
        self.error.set_label(error);
        self.error.set_visible(true);
        self.sync_sensitivity();
    }

    fn show(self: &Rc<Self>, state: &State) {
        let presentation = state.presentation();
        self.error.set_visible(false);
        self.title.set_label(presentation.title);
        self.screen_description
            .set_label(presentation.screen_description);
        while let Some(child) = self.screen_side.first_child() {
            self.screen_side.remove(&child);
        }
        if let Some(status) = presentation.screen_status {
            self.screen_side
                .append(&status_pill(status.label, status.ready));
        }
        if let Some(action) = presentation.screen_action {
            let button = gtk::Button::with_label(action);
            crate::widgets::action_cursor(&button);
            button.add_css_class("ui-btn");
            button.add_css_class("onboarding-permission-button");
            button.set_halign(gtk::Align::End);
            let setup = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                if let Some(setup) = setup.upgrade() {
                    setup.run(Action::RequestScreen);
                }
            });
            self.screen_side.append(&button);
        }
        self.primary.set_label(presentation.primary_label);
        crate::emit(
            "setup",
            serde_json::json!({"title": presentation.title,
            "ready": presentation.screen_ready}),
        );
        *self.state.borrow_mut() = Some(presentation);
        self.sync_sensitivity();
    }

    /// Start is enabled once screen access is usable and nothing runs;
    /// shipping's calm halo pulses only then.
    fn sync_sensitivity(&self) {
        let ready = self
            .state
            .borrow()
            .as_ref()
            .is_some_and(|state| state.screen_ready);
        let enabled = ready && !self.busy.get();
        self.primary.set_sensitive(enabled);
        if enabled {
            self.primary.add_css_class("cta-pulse");
        } else {
            self.primary.remove_css_class("cta-pulse");
        }
        let mut child = self.screen_side.first_child();
        while let Some(widget) = child {
            if widget.is::<gtk::Button>() {
                widget.set_sensitive(!self.busy.get());
            }
            child = widget.next_sibling();
        }
    }
}

/// `.onboarding-permission-status`: a ready pill carries a check mark.
fn status_pill(text: &str, ready: bool) -> gtk::Box {
    let pill = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    pill.add_css_class("onboarding-permission-status");
    pill.set_halign(gtk::Align::End);
    pill.append(&label(text, "onboarding-permission-status-label"));
    if ready {
        pill.add_css_class("ready");
        pill.append(&Icon::new("setup-check", 14).with_stroke(3.));
    }
    pill
}
