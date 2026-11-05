//! The Preferences window on GTK4, following shipping `.preferences`
//! (`styles/windows.css`) and the shared copy in `captures_app::preferences`.
//!
//! GTK owns layout, text, input and AT-SPI; every surface, control and colour
//! comes from the Captures tokens. Widgets are retained, so a hover or a switch
//! tween redraws only the widget that changed.
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use captures_app::preferences::{self, SECTIONS};
use gtk::glib;
use gtk::prelude::*;

use crate::switch::TokenSwitch;
use crate::tokens::Tokens;
use crate::widgets::{Icon, MaxWidth};

pub struct Options {
    pub settings_file: PathBuf,
    pub history_root: Option<PathBuf>,
}

struct State {
    settings: captures_settings::AppSettings,
    settings_file: PathBuf,
}

fn label(text: &str, class: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.add_css_class(class);
    label.set_xalign(0.);
    label
}

/// Shipping `max-width: 52ch` copy that wraps.
fn description(text: &str, class: &str) -> gtk::Label {
    let label = label(text, class);
    label.set_wrap(true);
    label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    label.set_max_width_chars(52);
    label.set_width_chars(1);
    label
}

pub fn build(
    app: &gtk::Application,
    tokens: Rc<Tokens>,
    options: Options,
) -> gtk::ApplicationWindow {
    let spec = captures_app::app_windows::PREFERENCES;
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title(spec.title)
        .default_width(spec.width as i32)
        .default_height(spec.height as i32)
        .build();
    window.add_css_class("captures");
    window.set_size_request(spec.min_width as i32, spec.min_height as i32);

    let settings = captures_settings::load(&options.settings_file).unwrap_or_default();
    let state = Rc::new(RefCell::new(State {
        settings,
        settings_file: options.settings_file,
    }));

    let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    window.set_child(Some(&root));

    // `.preferences-nav`: brand, then the section list.
    let nav = gtk::Box::new(gtk::Orientation::Vertical, tokens.number("s-6") as i32);
    nav.add_css_class("preferences-nav");
    nav.set_size_request(196, -1);
    let brand = gtk::Box::new(gtk::Orientation::Horizontal, tokens.number("s-4") as i32);
    brand.add_css_class("preferences-nav-brand");
    let mark = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    mark.add_css_class("brand-mark");
    mark.set_valign(gtk::Align::Center);
    let glyph = Icon::new("capture", 16);
    glyph.set_halign(gtk::Align::Center);
    glyph.set_valign(gtk::Align::Center);
    glyph.set_size_request(26, 26);
    mark.append(&glyph);
    brand.append(&mark);
    brand.append(&label("Captures", "brand-title"));
    nav.append(&brand);
    let list = gtk::Box::new(gtk::Orientation::Vertical, 1);
    list.update_property(&[gtk::accessible::Property::Label("Preferences sections")]);
    nav.append(&list);
    root.append(&nav);

    // `.preferences-body`: header over the scrolling cards.
    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.set_hexpand(true);
    // `.preferences-header`: title and autosave note, then the History action.
    let header = gtk::Box::new(gtk::Orientation::Horizontal, tokens.number("s-6") as i32);
    header.add_css_class("preferences-header");
    let heading = gtk::Box::new(gtk::Orientation::Vertical, 0);
    heading.set_hexpand(true);
    heading.set_valign(gtk::Align::Center);
    heading.append(&label(preferences::TITLE, "header-title"));
    heading.append(&label(preferences::SUBTITLE, "header-description"));
    header.append(&heading);
    let history = gtk::Button::with_label(preferences::HISTORY_ACTION);
    history.add_css_class("preferences-history-button");
    history.set_valign(gtk::Align::Center);
    header.append(&history);
    body.append(&header);

    let sections = gtk::Box::new(gtk::Orientation::Vertical, tokens.number("s-6") as i32);
    sections.add_css_class("preferences-sections");
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&MaxWidth::new(&sections, 720))
        .build();
    body.append(&scroller);
    root.append(&body);

    let margin = tokens.number("s-6");
    let general = card(&tokens, 0);
    general.append(&login_item_row(&tokens, options.history_root, &state));
    sections.append(&general);

    let capture = card(&tokens, 2);
    for (index, key) in [
        "auto_copy_to_clipboard",
        "auto_start_on_selection",
        "freeze_screen",
        "show_cursor_in_screenshots",
    ]
    .into_iter()
    .enumerate()
    {
        let on = setting_flag(&state.borrow().settings, key);
        let row = switch_row(
            &tokens,
            preferences::row(key).title,
            preferences::row(key).description,
            on,
            index == 0,
        );
        let state = state.clone();
        row.1.connect_toggled(move |on| {
            let mut state = state.borrow_mut();
            set_setting_flag(&mut state.settings, key, on);
            let saved = captures_settings::save(&state.settings_file, &state.settings);
            if let Ok(saved) = saved {
                state.settings = saved;
            }
        });
        capture.append(&row.0);
    }
    sections.append(&capture);
    let cards = Rc::new(vec![("general", general), ("capture", capture)]);

    for (index, section) in SECTIONS.iter().enumerate() {
        let button = gtk::Button::with_label(section.title);
        button.add_css_class("nav-item");
        if let Some(child) = button.child().and_downcast::<gtk::Label>() {
            child.set_xalign(0.);
        }
        if index == 0 {
            button.add_css_class("active");
        }
        button.update_property(&[gtk::accessible::Property::Label(section.title)]);
        let (cards, scroller) = (cards.clone(), scroller.clone());
        button.connect_clicked(move |_| {
            // Shipping scrolls the card to `scroll-margin-top: var(--s-6)`.
            let Some(card) = cards
                .iter()
                .find(|(id, _)| *id == section.id)
                .map(|(_, card)| card)
            else {
                return;
            };
            if let Some(bounds) = card.compute_bounds(&scroller.child().expect("content")) {
                let adjustment = scroller.vadjustment();
                adjustment.set_value(f64::from(bounds.y()) - f64::from(margin));
            }
        });
        list.append(&button);
    }
    // The section whose card sits under the probe line is the active entry.
    let follow = {
        let (list, scroller, cards) = (list.clone(), scroller.clone(), cards.clone());
        move || {
            let Some(content) = scroller.child() else {
                return;
            };
            let adjustment = scroller.vadjustment();
            // Cards in shipping order, as (section index, top).
            let shown: Vec<(usize, f32)> = SECTIONS
                .iter()
                .enumerate()
                .filter_map(|(index, section)| {
                    let (_, card) = cards.iter().find(|(id, _)| *id == section.id)?;
                    Some((index, card.compute_bounds(&content)?.y()))
                })
                .collect();
            let tops: Vec<f32> = shown.iter().map(|(_, top)| *top).collect();
            let probe = adjustment.value() as f32 + margin + 1.;
            let at_end = adjustment.value() + adjustment.page_size() >= adjustment.upper() - 1.;
            let active = shown[preferences::visible_section(&tops, probe, at_end)].0;
            let mut child = list.first_child();
            let mut index = 0;
            while let Some(widget) = child {
                if index == active {
                    widget.add_css_class("active");
                } else {
                    widget.remove_css_class("active");
                }
                index += 1;
                child = widget.next_sibling();
            }
        }
    };
    scroller
        .vadjustment()
        .connect_value_changed(move |_| follow());
    window
}

/// `.settings-card` with its `.settings-card-header`.
fn card(tokens: &Tokens, section: usize) -> gtk::Box {
    let card = gtk::Box::new(gtk::Orientation::Vertical, tokens.number("s-5") as i32);
    card.add_css_class("settings-card");
    let header = gtk::Box::new(gtk::Orientation::Vertical, tokens.number("s-2") as i32);
    header.append(&label(SECTIONS[section].title, "card-title"));
    header.append(&description(
        SECTIONS[section].description,
        "card-description",
    ));
    card.append(&header);
    card.update_property(&[gtk::accessible::Property::Label(SECTIONS[section].title)]);
    card
}

/// `.check-row.switch-row`: copy on the left, the switch on the right, and the
/// whole row toggles.
fn switch_row(
    tokens: &Rc<Tokens>,
    title: &str,
    detail: &str,
    on: bool,
    first: bool,
) -> (gtk::Box, TokenSwitch) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, tokens.number("s-6") as i32);
    row.add_css_class("setting-row");
    if first {
        row.add_css_class("first");
    }
    let copy = gtk::Box::new(gtk::Orientation::Vertical, 3);
    copy.add_css_class("setting-copy");
    copy.set_hexpand(true);
    copy.append(&label(title, "setting-title"));
    if !detail.is_empty() {
        let detail = description(detail, "setting-description");
        detail.set_max_width_chars(-1);
        copy.append(&detail);
    }
    row.append(&copy);
    let switch = TokenSwitch::new(tokens.clone(), on);
    switch.update_property(&[
        gtk::accessible::Property::Label(title),
        gtk::accessible::Property::Description(detail),
    ]);
    row.append(&switch);
    let click = gtk::GestureClick::new();
    let target = switch.clone();
    click.connect_released(move |gesture, _, _, _| {
        gesture.set_state(gtk::EventSequenceState::Claimed);
        target.activate_toggle();
        target.grab_focus();
    });
    row.add_controller(click);
    row.set_cursor_from_name(Some("pointer"));
    (row, switch)
}

/// General: Start Captures on login, queried and changed off the UI thread.
fn login_item_row(
    tokens: &Rc<Tokens>,
    history_root: Option<PathBuf>,
    state: &Rc<RefCell<State>>,
) -> gtk::Box {
    let platform = captures_app::shortcuts::ShortcutPlatform::Linux;
    let (detail, available) = match &history_root {
        Some(_) => (preferences::LOGIN_ITEM_CHECKING, true),
        None => (preferences::login_item_unavailable(platform), false),
    };
    let (row, switch) = switch_row(tokens, preferences::LOGIN_ITEM_TITLE, detail, false, true);
    switch.set_sensitive(false);
    if !available {
        row.add_css_class("disabled");
        return row;
    }
    let root = history_root.expect("checked above");
    let settings_file = state.borrow().settings_file.clone();
    let detail_label = row
        .first_child()
        .and_then(|copy| copy.last_child())
        .and_downcast::<gtk::Label>()
        .expect("detail label");
    let request = Rc::new({
        let (switch, detail_label) = (switch.clone(), detail_label.clone());
        move |enabled: Option<bool>| {
            switch.set_sensitive(false);
            let (root, settings_file) = (root.clone(), settings_file.clone());
            let (switch, detail_label) = (switch.clone(), detail_label.clone());
            glib::spawn_future_local(async move {
                let result = gtk::gio::spawn_blocking(move || {
                    captures_app::login_item::configure(&root, &settings_file, enabled)
                })
                .await
                .unwrap_or_else(|_| Err("Login item service stopped.".into()));
                match result {
                    Ok(on) => {
                        if switch.is_on() != on {
                            switch.activate_toggle_quietly(on);
                        }
                        detail_label.set_label(preferences::LOGIN_ITEM_DETAIL);
                        switch.set_sensitive(true);
                    }
                    Err(error) => detail_label.set_label(&preferences::login_item_error(&error)),
                }
            });
        }
    });
    let toggle = request.clone();
    switch.connect_toggled(move |on| toggle(Some(on)));
    request(None);
    row
}

fn setting_flag(settings: &captures_settings::AppSettings, key: &str) -> bool {
    match key {
        "auto_copy_to_clipboard" => settings.auto_copy_to_clipboard,
        "auto_start_on_selection" => settings.auto_start_on_selection,
        "freeze_screen" => settings.freeze_screen,
        "show_cursor_in_screenshots" => settings.show_cursor_in_screenshots,
        _ => unreachable!("unknown switch {key}"),
    }
}

fn set_setting_flag(settings: &mut captures_settings::AppSettings, key: &str, on: bool) {
    match key {
        "auto_copy_to_clipboard" => settings.auto_copy_to_clipboard = on,
        "auto_start_on_selection" => settings.auto_start_on_selection = on,
        "freeze_screen" => settings.freeze_screen = on,
        "show_cursor_in_screenshots" => settings.show_cursor_in_screenshots = on,
        _ => unreachable!("unknown switch {key}"),
    }
}
