//! Native GTK preferences for the Linux and Windows experiments.
//!
//! The caller owns persistence and side effects. Changes are validated and
//! applied automatically, matching the shipping preferences contract.
use crate::{settings::Settings, ui};
use captures_recording::MaxResolution;
use gtk::{gdk, glib, prelude::*};
use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    rc::Rc,
};

thread_local! {
    static SAVE_TIMER: RefCell<Option<glib::SourceId>> = const { RefCell::new(None) };
}

const SECTIONS: [(&str, &str); 7] = [
    ("appearance", "Appearance"),
    ("general", "Capture"),
    ("shortcuts", "Shortcuts"),
    ("recording", "Recording"),
    ("gif", "GIF export"),
    ("updates", "Updates"),
    ("about", "About"),
];

const THEMES: [(&str, &str, &str, &str, &str); 10] = [
    (
        "mustard",
        "Mustard",
        "Captures mustard and signal red",
        "#ffca28",
        "#ef4650",
    ),
    (
        "ember",
        "Ember",
        "Warm orange and electric pink",
        "#ff7a45",
        "#ff3d71",
    ),
    (
        "rose",
        "Rose",
        "Bright rose and coral",
        "#ff5ba7",
        "#ff6b45",
    ),
    (
        "violet",
        "Violet",
        "Orchid violet and raspberry",
        "#c026d3",
        "#ff4f88",
    ),
    (
        "cobalt",
        "Cobalt",
        "True blue and coral",
        "#2563eb",
        "#ff5a64",
    ),
    (
        "aqua",
        "Aqua",
        "Clear cyan and watermelon",
        "#31cbd8",
        "#ff5176",
    ),
    (
        "mint",
        "Mint",
        "Fresh mint and vermilion",
        "#67d5a5",
        "#f15a48",
    ),
    (
        "lime",
        "Lime",
        "Crisp lime and vermilion",
        "#b6db45",
        "#f15a48",
    ),
    (
        "mono",
        "Mono",
        "Vercel-like black and white",
        "#ededed",
        "#a1a1aa",
    ),
    (
        "custom",
        "Custom",
        "Build your own RGB palette",
        "#32d3ff",
        "#ff4fc3",
    ),
];

fn spacing(token: &str, fallback: i32) -> i32 {
    ui::token(token)
        .trim_end_matches("px")
        .parse::<i32>()
        .unwrap_or(fallback)
}

fn card(title: &str, description: &str) -> gtk::Box {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
    card.style_context().add_class("native-preferences-section");
    let heading = gtk::Box::new(gtk::Orientation::Vertical, spacing("s-2", 6));
    heading
        .style_context()
        .add_class("native-preferences-section-header");
    let title = ui::label(title, "native-preferences-section-title");
    heading.pack_start(&title, false, false, 0);
    if !description.is_empty() {
        let copy = ui::label(description, "muted");
        copy.set_line_wrap(true);
        heading.pack_start(&copy, false, false, 0);
    }
    card.pack_start(&heading, false, false, 0);
    card
}

fn row(title: &str, description: &str, control: &impl IsA<gtk::Widget>) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, spacing("s-6", 20));
    row.style_context().add_class("native-preferences-row");
    let copy = gtk::Box::new(gtk::Orientation::Vertical, spacing("s-1", 4));
    copy.set_hexpand(true);
    copy.pack_start(
        &ui::label(title, "native-preferences-row-title"),
        false,
        false,
        0,
    );
    if !description.is_empty() {
        let help = ui::label(description, "muted");
        help.set_line_wrap(true);
        help.set_max_width_chars(58);
        copy.pack_start(&help, false, false, 0);
    }
    row.pack_start(&copy, true, true, 0);
    row.pack_end(control, false, false, 0);
    ui::named(&row, title);
    row
}

fn stacked_row(title: &str, description: &str, control: &impl IsA<gtk::Widget>) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Vertical, spacing("s-4", 8));
    row.style_context().add_class("native-preferences-row");
    row.pack_start(
        &ui::label(title, "native-preferences-row-title"),
        false,
        false,
        0,
    );
    let help = ui::label(description, "muted");
    help.set_line_wrap(true);
    row.pack_start(&help, false, false, 0);
    row.pack_start(control, false, false, 0);
    ui::named(&row, title);
    row
}

fn theme_swatch(accent: &str, signal: &str, custom: bool) -> gtk::DrawingArea {
    let swatch = gtk::DrawingArea::new();
    swatch.set_size_request(18, 18);
    swatch.style_context().add_class("native-theme-swatch");
    let accent = gdk::RGBA::parse(accent).unwrap();
    let signal = gdk::RGBA::parse(signal).unwrap();
    swatch.connect_draw(move |area, cr| {
        let width = area.allocated_width() as f64;
        let height = area.allocated_height() as f64;
        if custom {
            for (index, color) in [
                "#ff5c66", "#ff9f45", "#f4df38", "#49cd7d", "#31cbd8", "#a95ee8", "#ff5ba7",
            ]
            .into_iter()
            .enumerate()
            {
                let color = gdk::RGBA::parse(color).unwrap();
                cr.set_source_rgba(color.red(), color.green(), color.blue(), 1.0);
                cr.rectangle(width * index as f64 / 7.0, 0.0, width / 7.0 + 1.0, height);
                cr.fill().unwrap();
            }
        } else {
            cr.set_source_rgba(accent.red(), accent.green(), accent.blue(), 1.0);
            cr.paint().unwrap();
            cr.set_source_rgba(signal.red(), signal.green(), signal.blue(), 1.0);
            cr.move_to(width, 0.0);
            cr.line_to(width, height);
            cr.line_to(0.0, height);
            cr.close_path();
            cr.fill().unwrap();
        }
        glib::Propagation::Proceed
    });
    swatch
}

fn switch_row(
    title: &str,
    description: &str,
    initial: bool,
    changed: impl Fn(bool) + 'static,
) -> gtk::Box {
    let toggle = gtk::Switch::new();
    toggle.set_valign(gtk::Align::Center);
    toggle.set_active(initial);
    toggle.connect_state_set(move |_, value| {
        changed(value);
        glib::Propagation::Proceed
    });
    row(title, description, &toggle)
}

fn combo(
    options: &[(&str, &str)],
    active: &str,
    changed: impl Fn(String) + 'static,
) -> gtk::ComboBoxText {
    let combo = gtk::ComboBoxText::new();
    for (id, label) in options {
        combo.append(Some(id), label);
    }
    combo.set_active_id(Some(active));
    combo.connect_changed(move |combo| {
        if let Some(value) = combo.active_id() {
            changed(value.to_string());
        }
    });
    combo
}

fn spin(
    min: f64,
    max: f64,
    step: f64,
    initial: f64,
    changed: impl Fn(f64) + 'static,
) -> gtk::SpinButton {
    let spin = gtk::SpinButton::with_range(min, max, step);
    spin.set_value(initial);
    spin.connect_value_changed(move |spin| changed(spin.value()));
    spin
}

fn shortcut_entry(initial: &str, changed: impl Fn(String) + 'static) -> gtk::Entry {
    let entry = gtk::Entry::new();
    entry.set_width_chars(24);
    entry.set_text(initial);
    entry.set_placeholder_text(Some("Press a key combination"));
    let before_edit = Rc::new(RefCell::new(initial.to_owned()));
    let changed = Rc::new(changed);
    {
        let changed = changed.clone();
        entry.connect_changed(move |entry| {
            let value = entry.text().to_string();
            changed(value);
        });
    }
    {
        let before_edit = before_edit.clone();
        entry.connect_focus_in_event(move |entry, _| {
            *before_edit.borrow_mut() = entry.text().to_string();
            glib::Propagation::Proceed
        });
    }
    entry.connect_key_press_event(move |entry, event| {
        let key = event.keyval();
        if key == gdk::keys::constants::Escape {
            entry.set_text(&before_edit.borrow());
            return glib::Propagation::Stop;
        }
        let state = event.state()
            & (gdk::ModifierType::SHIFT_MASK
                | gdk::ModifierType::CONTROL_MASK
                | gdk::ModifierType::MOD1_MASK
                | gdk::ModifierType::SUPER_MASK);
        let modifier_only = matches!(
            key,
            gdk::keys::constants::Shift_L
                | gdk::keys::constants::Shift_R
                | gdk::keys::constants::Control_L
                | gdk::keys::constants::Control_R
                | gdk::keys::constants::Alt_L
                | gdk::keys::constants::Alt_R
                | gdk::keys::constants::Super_L
                | gdk::keys::constants::Super_R
        );
        if modifier_only {
            return glib::Propagation::Stop;
        }
        if let Some(name) = key.name() {
            let mut parts = Vec::new();
            for (mask, name) in [
                (gdk::ModifierType::CONTROL_MASK, "Control"),
                (gdk::ModifierType::SHIFT_MASK, "Shift"),
                (gdk::ModifierType::MOD1_MASK, "Alt"),
                (gdk::ModifierType::SUPER_MASK, "Super"),
            ] {
                if state.contains(mask) {
                    parts.push(name.to_owned());
                }
            }
            parts.push(
                match name.as_str() {
                    "Print" => "PrintScreen",
                    "Return" => "Enter",
                    "ISO_Left_Tab" => "Tab",
                    other => other,
                }
                .to_owned(),
            );
            let accelerator = parts.join("+");
            if accelerator.parse::<global_hotkey::hotkey::HotKey>().is_ok() {
                entry.set_text(&accelerator);
            }
        }
        entry.set_position(-1);
        glib::Propagation::Stop
    });
    entry
}

fn page(_title: &str, _subtitle: &str) -> (gtk::Box, gtk::Box) {
    let section = gtk::Box::new(gtk::Orientation::Vertical, spacing("s-6", 16));
    let content = gtk::Box::new(gtk::Orientation::Vertical, spacing("s-6", 16));
    section.pack_start(&content, false, false, 0);
    (section, content)
}

fn staged<T>(
    draft: &Rc<RefCell<Settings>>,
    committed: &Rc<RefCell<Settings>>,
    apply: &Rc<dyn Fn(Settings) -> Result<(), String>>,
    status: &gtk::Label,
    update: impl Fn(&mut Settings, T) + 'static,
) -> impl Fn(T) + 'static {
    let draft = draft.clone();
    let committed = committed.clone();
    let apply = apply.clone();
    let status = status.clone();
    move |value| {
        update(&mut draft.borrow_mut(), value);
        save_now(&draft, &committed, &apply, &status);
    }
}

fn save_now(
    draft: &Rc<RefCell<Settings>>,
    committed: &Rc<RefCell<Settings>>,
    apply: &Rc<dyn Fn(Settings) -> Result<(), String>>,
    status: &gtk::Label,
) {
    status.set_text("Saving changes…");
    status.style_context().remove_class("saved");
    status.style_context().remove_class("error");
    SAVE_TIMER.with_borrow_mut(|timer| {
        if let Some(pending) = timer.take() {
            pending.remove();
        }
        let draft = draft.clone();
        let committed = committed.clone();
        let apply = apply.clone();
        let status = status.clone();
        *timer = Some(glib::timeout_add_local_once(
            std::time::Duration::from_millis(250),
            move || {
                let candidate = draft.borrow().clone();
                match candidate.validate().and_then(|_| apply(candidate.clone())) {
                    Ok(()) => {
                        *committed.borrow_mut() = candidate;
                        status.set_text("✓  Changes saved");
                        status.style_context().add_class("saved");
                        let status = status.clone();
                        glib::timeout_add_local_once(
                            std::time::Duration::from_secs(2),
                            move || {
                                if status.text() == "✓  Changes saved" {
                                    status.set_text("");
                                    status.style_context().remove_class("saved");
                                }
                            },
                        );
                    }
                    Err(error) => {
                        status.set_text(&format!("Couldn’t save changes: {error}"));
                        status.style_context().add_class("error");
                    }
                }
                SAVE_TIMER.with_borrow_mut(|timer| *timer = None);
            },
        ));
    });
}

/// Opens a production-shaped GTK preferences window.
pub fn open(
    settings: Rc<RefCell<Settings>>,
    apply: Rc<dyn Fn(Settings) -> Result<(), String>>,
) -> gtk::Window {
    let draft = Rc::new(RefCell::new(settings.borrow().clone()));
    let committed = settings.clone();
    let initial = draft.borrow().clone();
    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_title("Captures Preferences");
    window.set_default_size(980, 720);
    window.set_position(gtk::WindowPosition::Center);

    let provider = gtk::CssProvider::new();
    provider
        .load_from_data(include_bytes!("preferences.css"))
        .expect("valid preferences CSS");
    if let Some(screen) = gdk::Screen::default() {
        gtk::StyleContext::add_provider_for_screen(
            &screen,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
        );
    }

    let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    body.style_context().add_class("native-preferences");
    let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 0);
    // GTK adds CSS padding and borders outside the content request. Keep the
    // complete sidebar at the source UI's 196px grid track.
    sidebar.set_size_request(171, -1);
    sidebar
        .style_context()
        .add_class("native-preferences-sidebar");
    let brand = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    brand.style_context().add_class("native-preferences-brand");
    let mark = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    mark.style_context().add_class("native-preferences-mark");
    mark.set_valign(gtk::Align::Center);
    mark.add(&ui::icon("capture", 16));
    brand.pack_start(&mark, false, false, 0);
    brand.pack_start(&ui::label("Captures", "section-title"), false, false, 0);
    sidebar.pack_start(&brand, false, false, 0);
    let navigation = gtk::Box::new(gtk::Orientation::Vertical, 1);
    sidebar.pack_start(&navigation, false, false, 0);
    let buttons = Rc::new(RefCell::new(Vec::<(String, gtk::ToggleButton)>::new()));
    for (id, label) in SECTIONS {
        let button = gtk::ToggleButton::with_label(label);
        button.set_halign(gtk::Align::Fill);
        button.style_context().add_class("native-preferences-nav");
        ui::named(&button, &format!("{label} preferences section"));
        if let Some(label) = button.child().and_then(|c| c.downcast::<gtk::Label>().ok()) {
            label.set_xalign(0.);
        }
        navigation.pack_start(&button, false, false, 0);
        buttons.borrow_mut().push((id.to_owned(), button));
    }

    let main = gtk::Box::new(gtk::Orientation::Vertical, 0);
    main.set_hexpand(true);
    main.style_context().add_class("native-preferences-body");
    let header = gtk::Box::new(gtk::Orientation::Horizontal, spacing("s-6", 16));
    header
        .style_context()
        .add_class("native-preferences-header");
    let heading = gtk::Box::new(gtk::Orientation::Vertical, 2);
    heading.set_hexpand(true);
    heading.pack_start(
        &ui::label("Preferences", "native-preferences-title"),
        false,
        false,
        0,
    );
    heading.pack_start(
        &ui::label("Changes save automatically.", "muted"),
        false,
        false,
        0,
    );
    header.pack_start(&heading, true, true, 0);
    let history = ui::button("Capture History…");
    ui::named(&history, "Open Capture History");
    let status = ui::label("", "native-preferences-status");
    ui::named(&status, "Preferences save status");
    header.pack_end(&status, false, false, 0);
    header.pack_end(&history, false, false, 0);
    main.pack_start(&header, false, false, 0);

    let scroll = gtk::ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
    scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroll.set_overlay_scrolling(false);
    let sections = gtk::Box::new(gtk::Orientation::Vertical, spacing("s-6", 16));
    sections
        .style_context()
        .add_class("native-preferences-content");
    scroll.add(&sections);
    main.pack_start(&scroll, true, true, 0);

    // General
    let (general_scroll, general) = page(
        "General",
        "Capture behavior, storage, startup, and native availability.",
    );
    let capture = card(
        "Capture behavior",
        "Where captures go and what happens after taking one.",
    );
    let directory_line = gtk::Box::new(gtk::Orientation::Horizontal, spacing("s-2", 6));
    let directory = gtk::Entry::new();
    directory.set_width_chars(30);
    directory.set_text(&initial.output_directory.to_string_lossy());
    {
        let draft = draft.clone();
        let committed = committed.clone();
        let apply = apply.clone();
        let status = status.clone();
        directory.connect_changed(move |entry| {
            draft.borrow_mut().output_directory = PathBuf::from(entry.text().as_str());
            save_now(&draft, &committed, &apply, &status);
        });
    }
    let choose = ui::button("Choose…");
    {
        let parent = window.clone();
        let entry = directory.clone();
        choose.connect_clicked(move |_| {
            let dialog = gtk::FileChooserDialog::new(
                Some("Save captures to"),
                Some(&parent),
                gtk::FileChooserAction::SelectFolder,
            );
            dialog.add_buttons(&[
                ("Cancel", gtk::ResponseType::Cancel),
                ("Choose", gtk::ResponseType::Accept),
            ]);
            if dialog.run() == gtk::ResponseType::Accept
                && let Some(path) = dialog.filename()
            {
                entry.set_text(&path.to_string_lossy());
            }
            dialog.close();
        });
    }
    directory_line.pack_start(&directory, true, true, 0);
    directory_line.pack_start(&choose, false, false, 0);
    capture.pack_start(
        &row(
            "Save captures to",
            "Choose an absolute folder.",
            &directory_line,
        ),
        false,
        false,
        0,
    );
    capture.pack_start(
        &switch_row(
            "Copy captures to clipboard",
            "Preserve existing clipboard contents when off.",
            initial.auto_copy_to_clipboard,
            staged(&draft, &committed, &apply, &status, |s, v| {
                s.auto_copy_to_clipboard = v
            }),
        ),
        false,
        false,
        0,
    );
    capture.pack_start(
        &switch_row(
            "Start on target selection",
            "Immediately capture after choosing a region, window, or display.",
            initial.auto_start_on_selection,
            staged(&draft, &committed, &apply, &status, |s, v| {
                s.auto_start_on_selection = v
            }),
        ),
        false,
        false,
        0,
    );
    capture.pack_start(
        &switch_row(
            "Show mini previews",
            "Keep the quick-access preview stack visible.",
            initial.show_mini_previews,
            staged(&draft, &committed, &apply, &status, |s, v| {
                s.show_mini_previews = v
            }),
        ),
        false,
        false,
        0,
    );
    let corners = combo(
        &[
            ("0", "Bottom right"),
            ("1", "Bottom left"),
            ("2", "Top right"),
            ("3", "Top left"),
        ],
        &initial.mini_preview_placement.to_string(),
        staged(&draft, &committed, &apply, &status, |s, v: String| {
            s.mini_preview_placement = v.parse().unwrap_or(0)
        }),
    );
    capture.pack_start(
        &row(
            "Mini preview position",
            "Choose the screen corner used for previews.",
            &corners,
        ),
        false,
        false,
        0,
    );
    capture.pack_start(
        &switch_row(
            "Include mini previews in captures",
            "When off, previews stay out of screenshots and recordings.",
            initial.include_mini_previews_in_captures,
            staged(&draft, &committed, &apply, &status, |s, v| {
                s.include_mini_previews_in_captures = v
            }),
        ),
        false,
        false,
        0,
    );
    general.pack_start(&capture, false, false, 0);
    let system = card(
        "System",
        "Desktop integration is applied when settings are saved.",
    );
    system.pack_start(
        &switch_row(
            "Launch at login",
            "Start Captures when you sign in.",
            initial.launch_at_login,
            staged(&draft, &committed, &apply, &status, |s, v| {
                s.launch_at_login = v
            }),
        ),
        false,
        false,
        0,
    );
    let changelog = switch_row(
        "Show update changelog",
        "Unavailable until the native build has an update channel.",
        initial.show_update_changelog,
        staged(&draft, &committed, &apply, &status, |s, v| {
            s.show_update_changelog = v
        }),
    );
    changelog.set_sensitive(false);
    system.pack_start(&changelog, false, false, 0);
    system.pack_start(&row("Updates", "This native build has no automatic update channel. Install updates using the same source used to install Captures.", &ui::label("Managed externally", "muted")),false,false,0);
    let (updates_scroll, updates) = page("Updates", "Update behavior for this build.");
    updates.pack_start(&system, false, false, 0);

    // Appearance
    let (appearance_scroll, appearance) = page(
        "Appearance",
        "A neutral interface with a focused accent; overlays remain dark.",
    );
    let appearance_card = card(
        "Appearance",
        "One look across every Captures window. Capture overlays stay dark so they read on any desktop.",
    );
    let mode = combo(
        &[("system", "System"), ("light", "Light"), ("dark", "Dark")],
        &initial.appearance,
        staged(&draft, &committed, &apply, &status, |s, v| s.appearance = v),
    );
    appearance_card.pack_start(
        &row(
            "Interface theme",
            "Follow the system, or lock light or dark mode.",
            &mode,
        ),
        false,
        false,
        0,
    );
    let custom_rows = Rc::new(RefCell::new(Vec::<gtk::Box>::new()));
    let theme = gtk::Grid::new();
    theme.set_hexpand(true);
    theme.set_column_homogeneous(true);
    theme.set_column_spacing(spacing("s-2", 4) as u32);
    theme.set_row_spacing(spacing("s-2", 4) as u32);
    theme.style_context().add_class("native-theme-options");
    ui::named(&theme, "Color theme");
    let theme_buttons = Rc::new(RefCell::new(
        Vec::<(String, gtk::ToggleButton, gtk::Label)>::new(),
    ));
    for (index, (id, name, description, accent, signal)) in THEMES.into_iter().enumerate() {
        let button = gtk::ToggleButton::new();
        button.style_context().add_class("native-theme-option");
        button.set_relief(gtk::ReliefStyle::None);
        button.set_active(initial.theme == id);
        ui::named(&button, &format!("{name}: {description}"));
        button.set_tooltip_text(Some(description));
        let content = gtk::Box::new(gtk::Orientation::Horizontal, spacing("s-3", 6));
        content.pack_start(
            &theme_swatch(accent, signal, id == "custom"),
            false,
            false,
            0,
        );
        let label = ui::label(name, "native-theme-option-label");
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        content.pack_start(&label, true, true, 0);
        let check = ui::label("✓", "native-theme-option-check");
        check.set_opacity(if initial.theme == id { 1.0 } else { 0.0 });
        content.pack_end(&check, false, false, 0);
        button.add(&content);
        theme.attach(&button, (index % 5) as i32, (index / 5) as i32, 1, 1);
        theme_buttons
            .borrow_mut()
            .push((id.to_owned(), button, check));
    }
    for (id, button, _) in theme_buttons.borrow().iter() {
        let id = id.clone();
        let all = theme_buttons.clone();
        let custom_rows = custom_rows.clone();
        let draft = draft.clone();
        let committed = committed.clone();
        let apply = apply.clone();
        let status = status.clone();
        button.connect_toggled(move |clicked| {
            if !clicked.is_active() {
                if draft.borrow().theme == id {
                    clicked.set_active(true);
                }
                return;
            }
            // Publish the selection before deactivating siblings: their toggled
            // handlers consult it to distinguish replacement from deselection.
            draft.borrow_mut().theme = id.clone();
            for (other_id, other, check) in all.borrow().iter() {
                let selected = other_id == &id;
                if other != clicked {
                    other.set_active(selected);
                }
                check.set_opacity(if selected { 1.0 } else { 0.0 });
            }
            for row in custom_rows.borrow().iter() {
                row.set_visible(id == "custom");
            }
            save_now(&draft, &committed, &apply, &status);
        });
    }
    appearance_card.pack_start(
        &stacked_row(
            "Accent color",
            "Used for the capture action, selection, and focus. Status colors keep their meaning.",
            &theme,
        ),
        false,
        false,
        0,
    );
    for (title, value, assign) in [
        ("Custom accent", initial.custom_accent.clone(), 0u8),
        ("Recording signal", initial.custom_signal.clone(), 1u8),
    ] {
        let color = gtk::ColorButton::new();
        if let Ok(rgba) = gdk::RGBA::parse(&value) {
            color.set_rgba(&rgba);
        }
        let draft = draft.clone();
        let committed = committed.clone();
        let apply = apply.clone();
        let status = status.clone();
        color.connect_color_set(move |button| {
            let rgba = button.rgba();
            let hex = format!(
                "#{:02x}{:02x}{:02x}",
                (rgba.red() * 255.).round() as u8,
                (rgba.green() * 255.).round() as u8,
                (rgba.blue() * 255.).round() as u8
            );
            if assign == 0 {
                draft.borrow_mut().custom_accent = hex;
            } else {
                draft.borrow_mut().custom_signal = hex;
            }
            save_now(&draft, &committed, &apply, &status);
        });
        let custom_row = row(title, "Used when Custom is selected.", &color);
        custom_row.set_visible(initial.theme == "custom");
        appearance_card.pack_start(&custom_row, false, false, 0);
        custom_rows.borrow_mut().push(custom_row);
    }
    appearance.pack_start(&appearance_card, false, false, 0);

    // Screenshots
    capture.pack_start(
        &switch_row(
            "Freeze screen when capturing",
            "Hold menus, hover states, and motion still while selecting.",
            initial.freeze_screen,
            staged(&draft, &committed, &apply, &status, |s, v| {
                s.freeze_screen = v
            }),
        ),
        false,
        false,
        0,
    );
    capture.pack_start(
        &switch_row(
            "Show cursor",
            "Include the pointer in still captures.",
            initial.show_cursor_in_screenshots,
            staged(&draft, &committed, &apply, &status, |s, v| {
                s.show_cursor_in_screenshots = v
            }),
        ),
        false,
        false,
        0,
    );
    let shot_format = combo(
        &[("png", "PNG"), ("jpeg", "JPEG"), ("webp", "WebP")],
        &initial.screenshot_format,
        staged(&draft, &committed, &apply, &status, |s, v| {
            s.screenshot_format = v
        }),
    );
    capture.pack_start(
        &row(
            "Screenshot format",
            "Used when saving or exporting.",
            &shot_format,
        ),
        false,
        false,
        0,
    );
    let shot_delay = spin(
        0.,
        10.,
        1.,
        initial.screenshot_countdown_seconds.into(),
        staged(&draft, &committed, &apply, &status, |s, v| {
            s.screenshot_countdown_seconds = v as u8
        }),
    );
    capture.pack_start(
        &row(
            "Countdown",
            "Seconds before capture; 0 is off.",
            &shot_delay,
        ),
        false,
        false,
        0,
    );

    // Recording
    let (record_scroll, record) = page("Recording", "Defaults for new screen recordings.");
    let video = card(
        "Recording",
        "Defaults for new screen recordings. You can still change them in the capture menu.",
    );
    #[cfg(target_os = "windows")]
    video.pack_start(
        &switch_row(
            "Include recording controls in captures",
            "Show controls in new recordings for feedback or demos. Off excludes them using Windows capture protection.",
            initial.include_recording_controls_in_captures,
            staged(&draft, &committed, &apply, &status, |s, v| {
                s.include_recording_controls_in_captures = v;
            }),
        ), false, false, 0,
    );
    let video_format = combo(
        &[("mp4", "MP4"), ("gif", "GIF")],
        &initial.recording.video_format,
        staged(&draft, &committed, &apply, &status, |s, v| {
            s.recording.video_format = v
        }),
    );
    video.pack_start(
        &row(
            "Recording format",
            "MP4 and GIF export are supported. WebM is unavailable in this native media path.",
            &video_format,
        ),
        false,
        false,
        0,
    );
    let fps = combo(
        &[("15", "15 fps"), ("30", "30 fps"), ("60", "60 fps")],
        &initial.recording.video_fps.to_string(),
        staged(&draft, &committed, &apply, &status, |s, v: String| {
            s.recording.video_fps = v.parse().expect("video FPS choices are integers")
        }),
    );
    video.pack_start(
        &row(
            "Frames per second",
            "Higher values create smoother, larger recordings.",
            &fps,
        ),
        false,
        false,
        0,
    );
    let resolution = combo(
        &[
            ("original", "Original"),
            ("p1080", "1080p"),
            ("p720", "720p"),
        ],
        match initial.recording.video_max_resolution {
            MaxResolution::Original => "original",
            MaxResolution::P1080 => "p1080",
            MaxResolution::P720 => "p720",
        },
        staged(&draft, &committed, &apply, &status, |s, v: String| {
            s.recording.video_max_resolution = match v.as_str() {
                "p1080" => MaxResolution::P1080,
                "p720" => MaxResolution::P720,
                _ => MaxResolution::Original,
            }
        }),
    );
    video.pack_start(
        &row(
            "Maximum resolution",
            "Constrain output height while preserving aspect ratio.",
            &resolution,
        ),
        false,
        false,
        0,
    );
    let delay = spin(
        0.,
        10.,
        1.,
        initial.recording.countdown_seconds.into(),
        staged(&draft, &committed, &apply, &status, |s, v| {
            s.recording.countdown_seconds = v as u8
        }),
    );
    video.pack_start(
        &row("Countdown", "Seconds before recording; 0 is off.", &delay),
        false,
        false,
        0,
    );
    let microphones = gtk::ComboBoxText::new();
    microphones.append(Some("off"), "Off");
    microphones.append(Some("loading"), "Loading microphones…");
    microphones.set_active_id(Some(
        initial
            .recording
            .microphone_device_id
            .as_deref()
            .unwrap_or("off"),
    ));
    microphones.set_sensitive(false);
    {
        let draft = draft.clone();
        let committed = committed.clone();
        let apply = apply.clone();
        let status = status.clone();
        microphones.connect_changed(move |c| {
            if let Some(id) = c.active_id()
                && id != "loading"
            {
                let next = (id != "off").then(|| id.to_string());
                if draft.borrow().recording.microphone_device_id == next {
                    return;
                }
                draft.borrow_mut().recording.microphone_device_id = next;
                save_now(&draft, &committed, &apply, &status);
            }
        });
    }
    video.pack_start(
        &row(
            "Default microphone",
            "Enumerated asynchronously; choose Off for no microphone.",
            &microphones,
        ),
        false,
        false,
        0,
    );
    for (title, desc, value, setter) in [
        (
            "Record desktop audio",
            "Capture sound playing through the system output.",
            initial.recording.capture_system_audio,
            0u8,
        ),
        (
            "Export audio in mono",
            "Combine recording audio into one channel.",
            initial.recording.mono_audio,
            1,
        ),
        (
            "Show cursor",
            "Include the pointer in recordings.",
            initial.recording.show_cursor,
            2,
        ),
        (
            "Highlight clicks",
            "Show click feedback in recordings.",
            initial.recording.highlight_clicks,
            3,
        ),
        (
            "Show keystrokes",
            "Display typed key feedback in recordings.",
            initial.recording.show_keystrokes,
            4,
        ),
        (
            "Open editor after recording",
            "Open the editor when a recording finishes.",
            initial.recording.open_editor_after_recording,
            5,
        ),
    ] {
        let d = draft.clone();
        let committed = committed.clone();
        let apply = apply.clone();
        let status = status.clone();
        video.pack_start(
            &switch_row(title, desc, value, move |v| {
                let mut s = d.borrow_mut();
                match setter {
                    0 => s.recording.capture_system_audio = v,
                    1 => s.recording.mono_audio = v,
                    2 => s.recording.show_cursor = v,
                    3 => s.recording.highlight_clicks = v,
                    4 => s.recording.show_keystrokes = v,
                    _ => s.recording.open_editor_after_recording = v,
                }
                drop(s);
                save_now(&d, &committed, &apply, &status);
            }),
            false,
            false,
            0,
        );
    }
    record.pack_start(&video, false, false, 0);
    {
        let microphones = microphones.clone();
        let selected = initial.recording.microphone_device_id.clone();
        ui::job(
            || Ok(captures_recording_xcap::microphone_devices()),
            move |result| {
                microphones.remove_all();
                microphones.append(Some("off"), "Off");
                let available = result.is_ok();
                match result {
                    Ok(devices) => {
                        for device in devices {
                            microphones.append(Some(&device.id), &device.name);
                        }
                    }
                    Err(error) => {
                        microphones.append(Some("error"), &format!("Unavailable — {error}"))
                    }
                }
                microphones.set_active_id(Some(selected.as_deref().unwrap_or("off")));
                microphones.set_sensitive(available);
            },
        );
    }

    // GIF
    let (gif_scroll, gif) = page("GIF", "Defaults for animated GIF exports.");
    let gif_card = card(
        "GIF export",
        "Balance motion, dimensions, and palette size.",
    );
    let gif_fps = spin(
        1.,
        30.,
        1.,
        initial.recording.gif_fps.into(),
        staged(&draft, &committed, &apply, &status, |s, v| {
            s.recording.gif_fps = v as u16
        }),
    );
    gif_card.pack_start(
        &row(
            "Frames per second",
            "Motion smoothness for exported GIFs.",
            &gif_fps,
        ),
        false,
        false,
        0,
    );
    let gif_width = spin(
        16.,
        4096.,
        16.,
        initial.recording.gif_max_width.into(),
        staged(&draft, &committed, &apply, &status, |s, v| {
            s.recording.gif_max_width = v as u32
        }),
    );
    gif_card.pack_start(
        &row(
            "Maximum width",
            "Scale wider exports to this pixel width.",
            &gif_width,
        ),
        false,
        false,
        0,
    );
    let gif_colors = spin(
        2.,
        256.,
        1.,
        initial.recording.gif_max_colors.into(),
        staged(&draft, &committed, &apply, &status, |s, v| {
            s.recording.gif_max_colors = v as u16
        }),
    );
    gif_card.pack_start(
        &row(
            "Palette colors",
            "More colors improve fidelity and increase size.",
            &gif_colors,
        ),
        false,
        false,
        0,
    );
    gif.pack_start(&gif_card, false, false, 0);

    // Shortcuts
    let (keys_scroll, keys) = page(
        "Shortcuts",
        "Select a field and press a key combination. Conflict checks run when you save.",
    );
    let key_card = card(
        "Global shortcuts",
        if cfg!(target_os = "windows") {
            "Shortcuts apply while this native build is running. Win+Shift+S replaces Snipping Tool only while assigned here. Quit the other Captures app to avoid conflicts."
        } else {
            "X11 scope: shortcuts apply only to this native X11 build; desktop-reserved combinations may be unavailable."
        },
    );
    let shortcuts = [
        ("New Capture", initial.new_capture_shortcut.clone(), 0u8),
        ("Screenshot Region", initial.region_shortcut.clone(), 1),
        ("Screenshot Window", initial.window_shortcut.clone(), 2),
        (
            "Screenshot Full Screen",
            initial.display_shortcut.clone(),
            3,
        ),
        ("Record Region", initial.recording.video_shortcut.clone(), 4),
        (
            "Record Window",
            initial.recording.window_shortcut.clone(),
            5,
        ),
        (
            "Record Full Screen",
            initial.recording.display_shortcut.clone(),
            6,
        ),
        ("Record GIF", initial.recording.gif_shortcut.clone(), 7),
    ];
    for (title, value, index) in shortcuts {
        let d = draft.clone();
        let committed = committed.clone();
        let apply = apply.clone();
        let status = status.clone();
        let entry = shortcut_entry(&value, move |v| {
            let mut s = d.borrow_mut();
            match index {
                0 => s.new_capture_shortcut = v,
                1 => s.region_shortcut = v,
                2 => s.window_shortcut = v,
                3 => s.display_shortcut = v,
                4 => s.recording.video_shortcut = v,
                5 => s.recording.window_shortcut = v,
                6 => s.recording.display_shortcut = v,
                _ => s.recording.gif_shortcut = v,
            }
            drop(s);
            save_now(&d, &committed, &apply, &status);
        });
        key_card.pack_start(
            &row(title, "Click, then press the desired combination.", &entry),
            false,
            false,
            0,
        );
    }
    keys.pack_start(&key_card, false, false, 0);

    let (about_scroll, about) = page("About", "About this build.");
    about.pack_start(
        &card(
            "About",
            "Captures is in active development. Telling us what breaks is the fastest way to fix it.",
        ),
        false,
        false,
        0,
    );

    let targets = [
        ("appearance", appearance_scroll.clone()),
        ("general", general_scroll.clone()),
        ("shortcuts", keys_scroll.clone()),
        ("recording", record_scroll.clone()),
        ("gif", gif_scroll.clone()),
        ("updates", updates_scroll.clone()),
        ("about", about_scroll.clone()),
    ];
    for (_, section) in &targets {
        sections.pack_start(section, false, false, 0);
    }
    let active_section = Rc::new(Cell::new(0usize));
    for (index, (id, button)) in buttons.borrow().iter().enumerate() {
        let target = targets
            .iter()
            .find(|(name, _)| name == id)
            .unwrap()
            .1
            .clone();
        let adjustment = scroll.vadjustment();
        let all = buttons.clone();
        let active_section = active_section.clone();
        button.connect_toggled(move |clicked| {
            if !clicked.is_active() {
                if active_section.get() == index {
                    clicked.set_active(true);
                }
                return;
            }
            active_section.set(index);
            for (_, other) in all.borrow().iter() {
                if other != clicked {
                    other.set_active(false);
                }
            }
            adjustment.set_value(target.allocation().y() as f64);
        });
    }
    history.connect_clicked(move |_| {
        if let Ok(executable) = std::env::current_exe() {
            let _ = std::process::Command::new(executable)
                .arg("--history")
                .spawn();
        }
    });

    body.pack_start(&sidebar, false, false, 0);
    body.pack_start(&main, true, true, 0);
    window.add(&body);
    buttons.borrow()[0].1.set_active(true);
    window.show_all();
    for row in custom_rows.borrow().iter() {
        row.set_visible(initial.theme == "custom");
    }
    window
}
