//! The Preferences window on GTK4, following shipping `.preferences`
//! (`styles/windows.css`, `PreferencesSections` in `App.tsx`) and the shared
//! copy in `captures_app::preferences`.
//!
//! GTK owns layout, text, input and AT-SPI; every surface, control and colour
//! comes from the Captures tokens. Widgets are retained, so a hover or a switch
//! tween redraws only the widget that changed. Each control writes one setting
//! by its persisted path (`recording.gif_fps`) and saves immediately.
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use captures_app::preferences::{self, SECTIONS};
use captures_app::shortcuts::{
    ShortcutKeyEvent, ShortcutPlatform, ShortcutRecording, record_shortcut, shortcut_display_tokens,
};
use captures_settings::AppSettings;
use gtk::prelude::*;
use gtk::{gdk, gio, glib, graphene};
use serde_json::Value;

use crate::editor::controls::{Choice, pressed};
use crate::switch::TokenSwitch;
use crate::tokens::Tokens;
use crate::widgets::{Icon, MaxWidth, SnapshotArea, WidthWatch};

mod find;
mod updates;

const PLATFORM: ShortcutPlatform = ShortcutPlatform::Linux;

/// Restyle the app for new look settings; see [`Options::on_look`].
pub type OnLook = Box<dyn Fn(&AppSettings, bool) -> Rc<Tokens>>;
type Listener = Box<dyn Fn(&AppSettings)>;
/// Scroll a section's card into view; false before it has a layout.
type Jump = Rc<dyn Fn(&str) -> bool>;

pub struct Options {
    pub settings_file: PathBuf,
    pub history_root: Option<PathBuf>,
    /// Restyle the app for new appearance or accent settings (and whether the
    /// user picked the appearance itself), returning the new tokens.
    pub on_look: OnLook,
    /// Open Find with this query (`--find`, for parity screenshots).
    pub find: Option<String>,
    /// About's Send feedback.
    pub open_feedback: Box<dyn Fn()>,
}

/// The page's settings and the widgets that follow them.
struct Prefs {
    tokens: RefCell<Rc<Tokens>>,
    appearance: RefCell<String>,
    settings: RefCell<AppSettings>,
    settings_file: PathBuf,
    on_look: OnLook,
    open_feedback: Box<dyn Fn()>,
    /// Called after every save with the settings as written.
    listeners: RefCell<Vec<Listener>>,
    /// Token-drawn widgets that restyle with the look.
    switches: RefCell<Vec<TokenSwitch>>,
    swatches: RefCell<Vec<SnapshotArea>>,
    /// Selects stay alive with the page (their triggers hold weak refs).
    choices: RefCell<Vec<Rc<Choice>>>,
    status: gtk::Box,
    status_label: gtk::Label,
    status_generation: Rc<Cell<u64>>,
    /// The Updates card, while the page is open.
    updates: RefCell<Option<Rc<updates::Updates>>>,
    /// Scroll a section's card into view; false before it has a layout.
    jump: RefCell<Option<Jump>>,
}

impl Prefs {
    /// The persisted value at a dotted path, as JSON.
    fn get(&self, path: &str) -> Value {
        let json = serde_json::to_value(&*self.settings.borrow()).unwrap_or_default();
        json.pointer(&pointer(path)).cloned().unwrap_or(Value::Null)
    }

    /// Write one setting, save, then refresh the look and dependent rows.
    fn set(&self, path: &str, value: Value) {
        let mut json = serde_json::to_value(&*self.settings.borrow()).unwrap_or_default();
        let Some(slot) = json.pointer_mut(&pointer(path)) else {
            return;
        };
        if *slot == value {
            return;
        }
        *slot = value;
        let Ok(next) = serde_json::from_value::<AppSettings>(json) else {
            return;
        };
        let saved = match captures_settings::save(&self.settings_file, &next) {
            Ok(saved) => {
                self.show_status(true, preferences::SAVED);
                saved
            }
            Err(error) => {
                self.show_status(false, &preferences::save_error(&error.to_string()));
                next
            }
        };
        *self.settings.borrow_mut() = saved.clone();
        if matches!(
            path.split('.').next(),
            Some("appearance" | "theme" | "custom_theme")
        ) {
            let tokens = (self.on_look)(&saved, path == "appearance");
            self.restyle(tokens);
        }
        for listener in self.listeners.borrow().iter() {
            listener(&saved);
        }
    }

    /// Token-drawn widgets follow a new look.
    fn restyle(&self, tokens: Rc<Tokens>) {
        // Dark tokens have light text.
        *self.appearance.borrow_mut() = if tokens.rgba("text").red() < 0.5 {
            "light"
        } else {
            "dark"
        }
        .into();
        for switch in self.switches.borrow().iter() {
            switch.set_tokens(tokens.clone());
        }
        for swatch in self.swatches.borrow().iter() {
            swatch.queue_draw();
        }
        *self.tokens.borrow_mut() = tokens;
    }

    fn listen(&self, f: impl Fn(&AppSettings) + 'static) {
        self.listeners.borrow_mut().push(Box::new(f));
    }

    /// Shipping `.preferences-save-status`: Saved clears after 2 s; an error
    /// stays until the next save.
    fn show_status(&self, ok: bool, message: &str) {
        let generation = self.status_generation.get() + 1;
        self.status_generation.set(generation);
        self.status.remove_css_class("saved");
        self.status.remove_css_class("error");
        self.status
            .add_css_class(if ok { "saved" } else { "error" });
        if let Some(mark) = self.status.first_child().and_downcast::<gtk::Label>() {
            mark.set_label(if ok { "✓" } else { "!" });
        }
        self.status_label.set_label(message);
        self.status.set_visible(true);
        if ok {
            let (status, current) = (self.status.clone(), self.status_generation.clone());
            glib::timeout_add_local_once(Duration::from_millis(2000), move || {
                if current.get() == generation {
                    status.set_visible(false);
                }
            });
        }
    }
}

fn pointer(path: &str) -> String {
    format!("/{}", path.replace('.', "/"))
}

pub(crate) fn label(text: &str, class: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.add_css_class(class);
    label.set_xalign(0.);
    label
}

/// Wrapping secondary copy.
pub(crate) fn description(text: &str, class: &str) -> gtk::Label {
    let label = label(text, class);
    label.set_wrap(true);
    label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    label.set_width_chars(1);
    label
}

thread_local! {
    /// CSS `1ch` at `--text-sm`: the width of "0" in the UI font.
    static CH: Cell<f32> = const { Cell::new(7.) };
    /// The open Preferences page, for looks that change outside it.
    static OPEN: RefCell<std::rc::Weak<Prefs>> = const { RefCell::new(std::rc::Weak::new()) };
}

/// Restyle an open Preferences window's token-drawn widgets for a look the
/// page did not pick (the desktop's light or dark for System).
pub fn restyle(tokens: Rc<Tokens>) {
    if let Some(prefs) = OPEN.with(|open| open.borrow().upgrade()) {
        prefs.restyle(tokens);
    }
}

/// The resident app's update state changed: redraw an open Updates card.
pub fn refresh_updates() {
    updates::refresh_open();
}

/// Bring an open page's section into view (the tray's Check for Updates),
/// once a newly opened window has laid out.
pub fn show_section(section: &'static str) {
    let mut tries = 0;
    glib::timeout_add_local(Duration::from_millis(50), move || {
        tries += 1;
        let jump = OPEN.with(|open| open.borrow().upgrade()?.jump.borrow().clone());
        match jump {
            Some(jump) if jump(section) => glib::ControlFlow::Break,
            _ if tries >= 40 => glib::ControlFlow::Break,
            _ => glib::ControlFlow::Continue,
        }
    });
}

/// Measure `1ch` for [`capped`] once the stylesheet is installed.
pub(crate) fn measure_ch(tokens: &Tokens) {
    let layout = gtk::Label::new(None).create_pango_layout(Some("0"));
    let mut font = layout.context().font_description().unwrap_or_default();
    // Measured at 100× and scaled back: hinted metrics round each advance
    // to a whole pixel, and a rounded 8 px widens a 56ch cap by 20 px.
    let size = f64::from(tokens.number("text-sm")) * 100.;
    font.set_absolute_size(size * f64::from(gtk::pango::SCALE));
    layout.set_font_description(Some(&font));
    CH.set(layout.size().0 as f32 / gtk::pango::SCALE as f32 / 100.);
}

/// Shipping `max-width: {chars}ch` around wrapping copy. GTK's
/// `max-width-chars` measures with the default font, not the label's.
pub(crate) fn capped(label: &gtk::Label, chars: f32) -> MaxWidth {
    MaxWidth::new(label, (chars * CH.get()).round() as i32)
}

pub fn build(
    app: &gtk::Application,
    tokens: Rc<Tokens>,
    options: Options,
    open_history: impl Fn() + 'static,
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

    measure_ch(&tokens);
    let settings = captures_settings::load(&options.settings_file).unwrap_or_default();
    let status = gtk::Box::new(gtk::Orientation::Horizontal, tokens.number("s-3") as i32);
    status.add_css_class("preferences-save-status");
    status.set_valign(gtk::Align::Center);
    status.set_visible(false);
    status.set_accessible_role(gtk::AccessibleRole::Status);
    let mark = label("", "preferences-save-mark");
    mark.set_xalign(0.5);
    mark.set_size_request(13, 13);
    mark.set_valign(gtk::Align::Center);
    mark.set_accessible_role(gtk::AccessibleRole::Presentation);
    status.append(&mark);
    let status_label = label("", "preferences-save-text");
    status_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    status.append(&status_label);
    let prefs = Rc::new(Prefs {
        appearance: RefCell::new(
            if tokens.rgba("text").red() < 0.5 {
                "light"
            } else {
                "dark"
            }
            .into(),
        ),
        tokens: RefCell::new(tokens.clone()),
        settings: RefCell::new(settings),
        settings_file: options.settings_file,
        on_look: options.on_look,
        open_feedback: options.open_feedback,
        listeners: RefCell::default(),
        switches: RefCell::default(),
        swatches: RefCell::default(),
        choices: RefCell::default(),
        status,
        status_label,
        status_generation: Rc::default(),
        updates: RefCell::default(),
        jump: RefCell::default(),
    });
    OPEN.with(|open| *open.borrow_mut() = Rc::downgrade(&prefs));

    let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    // Shipping's `@media (max-width: 720px)` layout, applied after allocation.
    let compact_now: Rc<Cell<Option<bool>>> = Rc::default();
    let root_for_realize = root.clone();
    let watch = WidthWatch::new(&root, {
        let (root, compact_now) = (root.clone(), compact_now.clone());
        move |width| {
            let compact = captures_app::app_windows::compact(width as f32);
            if compact_now.replace(Some(compact)) != Some(compact) {
                let root = root.clone();
                glib::idle_add_local_once(move || apply_compact(root.upcast_ref(), compact));
            }
        }
    });
    window.set_child(Some(&watch));
    // Also decide from the requested width before layout: the wide layout's
    // minimum (nav plus page) would otherwise hold a narrow window wider.
    let from_default = move |window: &gtk::ApplicationWindow| {
        let compact = captures_app::app_windows::compact(window.default_width() as f32);
        if compact_now.replace(Some(compact)) != Some(compact) {
            apply_compact(root_for_realize.upcast_ref(), compact);
        }
    };
    let from_default = Rc::new(from_default);
    window.connect_realize({
        let from_default = from_default.clone();
        move |window| from_default(window)
    });
    window.connect_default_width_notify(move |window| from_default(window));

    // `.preferences-nav`: brand, then the section list.
    let nav = gtk::Box::new(gtk::Orientation::Vertical, tokens.number("s-6") as i32);
    nav.set_widget_name("preferences-nav");
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
    // `.preferences-header`: title and autosave note, then the History action
    // and the save status.
    // Find opens as a second row (`.preferences-find { grid-column: 1 / -1 }`).
    let header = gtk::Box::new(gtk::Orientation::Vertical, tokens.number("s-6") as i32);
    header.add_css_class("preferences-header");
    let top = gtk::Box::new(gtk::Orientation::Horizontal, tokens.number("s-6") as i32);
    header.append(&top);
    let heading = gtk::Box::new(gtk::Orientation::Vertical, 0);
    heading.set_hexpand(true);
    heading.set_valign(gtk::Align::Center);
    heading.append(&label(preferences::TITLE, "header-title"));
    heading.append(&label(preferences::SUBTITLE, "header-description"));
    top.append(&heading);
    let actions = gtk::Box::new(gtk::Orientation::Horizontal, tokens.number("s-4") as i32);
    actions.set_valign(gtk::Align::Center);
    let history = gtk::Button::with_label(preferences::HISTORY_ACTION);
    history.add_css_class("preferences-history-button");
    history.set_valign(gtk::Align::Center);
    history.connect_clicked(move |_| open_history());
    actions.append(&history);
    actions.append(&prefs.status);
    top.append(&actions);
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
    let find = find::build(&window, &sections, &scroller, tokens.number("s-3") as i32);
    header.append(&find.bar);

    let margin = tokens.number("s-6");
    let general = card(&tokens, 0, None);
    add(&general, &login_item_row(&prefs, options.history_root));
    let cards = Rc::new(vec![
        ("general", general),
        ("appearance", appearance_card(&prefs)),
        ("capture", capture_card(&prefs, &window)),
        ("shortcuts", shortcuts_card(&prefs)),
        ("recording", recording_card(&prefs)),
        ("gif", gif_card(&prefs)),
        ("updates", updates_card(&prefs)),
        ("about", about_card(&prefs)),
    ]);
    for (_, card) in cards.iter() {
        sections.append(card);
    }

    // Shipping scrolls the card to `scroll-margin-top: var(--s-6)`.
    let jump: Jump = {
        let (cards, scroller) = (cards.clone(), scroller.clone());
        Rc::new(move |section: &str| {
            let Some((_, card)) = cards.iter().find(|(id, _)| *id == section) else {
                return false;
            };
            let Some(bounds) = card.compute_bounds(&content(&scroller)) else {
                return false;
            };
            if card.height() == 0 {
                return false;
            }
            let adjustment = scroller.vadjustment();
            adjustment.set_value(f64::from(bounds.y()) - f64::from(margin));
            true
        })
    };
    *prefs.jump.borrow_mut() = Some(jump.clone());
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
        let jump = jump.clone();
        button.connect_clicked(move |_| {
            jump(section.id);
        });
        list.append(&button);
    }
    // The section whose card sits under the probe line is the active entry.
    let follow = {
        let (list, scroller, cards) = (list.clone(), scroller.clone(), cards.clone());
        move || {
            let content = content(&scroller);
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
    if let Some(query) = options.find {
        find.open_with(&query);
    }
    // The window owns the page state until it closes; controls hold weak refs.
    let keep = RefCell::new(Some((prefs, find)));
    window.connect_close_request(move |_| {
        if let Some((prefs, _)) = keep.borrow_mut().take() {
            prefs.jump.borrow_mut().take();
        }
        glib::Propagation::Proceed
    });
    window
}

/// Shipping's compact rules (`windows.css`, `@media (max-width: 720px)`):
/// the nav hides, inline rows stack with stretched controls (the corner
/// picker stays at the end), three-select grids and accent chips use two
/// columns, and segmented controls share the track equally. Switch,
/// shortcut and utility rows are not inline rows, so they never stack.
fn apply_compact(widget: &gtk::Widget, compact: bool) {
    if widget.widget_name() == "preferences-nav" {
        widget.set_visible(!compact);
        return;
    }
    if widget.has_css_class("setting-row-inline")
        && let Some(row) = widget.downcast_ref::<gtk::Box>()
    {
        row.set_orientation(if compact {
            gtk::Orientation::Vertical
        } else {
            gtk::Orientation::Horizontal
        });
        // `gap: var(--s-4) var(--s-6)`.
        row.set_spacing(if compact { 8 } else { 24 });
        if let Some(control) = row.last_child() {
            control.set_halign(if compact && !control.has_css_class("keep-end") {
                gtk::Align::Fill
            } else {
                gtk::Align::End
            });
        }
    }
    if widget.has_css_class("ui-segmented")
        && let Some(segments) = widget.downcast_ref::<gtk::Box>()
    {
        segments.set_homogeneous(compact);
    }
    let columns = if widget.has_css_class("setting-grid-three") {
        Some(if compact { 2 } else { 3 })
    } else if widget.has_css_class("theme-options") {
        Some(preferences::theme_columns(compact))
    } else {
        None
    };
    if let (Some(columns), Some(grid)) = (columns, widget.downcast_ref::<gtk::Grid>()) {
        let mut children = Vec::new();
        let mut child = grid.first_child();
        while let Some(widget) = child {
            child = widget.next_sibling();
            children.push(widget);
        }
        for (index, child) in children.iter().enumerate() {
            grid.remove(child);
            grid.attach(
                child,
                (index % columns) as i32,
                (index / columns) as i32,
                1,
                1,
            );
        }
    }
    let mut child = widget.first_child();
    while let Some(widget) = child {
        apply_compact(&widget, compact);
        child = widget.next_sibling();
    }
}

/// The scrolled page inside the viewport, whose coordinates do not move
/// with the scroll offset.
fn content(scroller: &gtk::ScrolledWindow) -> gtk::Widget {
    let viewport = scroller.child().expect("viewport");
    viewport.first_child().unwrap_or(viewport)
}

/// `.settings-card` with its `.settings-card-header`.
fn card(tokens: &Tokens, section: usize, detail: Option<&str>) -> gtk::Box {
    let card = gtk::Box::new(gtk::Orientation::Vertical, tokens.number("s-5") as i32);
    card.add_css_class("settings-card");
    let header = gtk::Box::new(gtk::Orientation::Vertical, tokens.number("s-2") as i32);
    header.add_css_class("settings-card-header");
    header.append(&label(SECTIONS[section].title, "card-title"));
    header.append(&capped(
        &description(
            detail.unwrap_or(SECTIONS[section].description),
            "card-description",
        ),
        52.,
    ));
    card.append(&header);
    card.update_property(&[gtk::accessible::Property::Label(SECTIONS[section].title)]);
    card
}

/// Append a card row: shipping `.settings-card > * + *` draws a rule above
/// every row except the one right under the header.
fn add(card: &gtk::Box, row: &impl IsA<gtk::Widget>) {
    let row = row.as_ref();
    row.add_css_class("setting-row");
    if card.first_child() == card.last_child() {
        row.add_css_class("first");
    }
    card.append(row);
}

/// `.setting-copy`: title over an optional description.
/// `cap` applies shipping's 52ch measure (`.setting-copy small`); switch
/// rows (`.check-row small`) run the full width.
fn copy(title: &str, detail: &str, cap: bool) -> (gtk::Box, gtk::Label) {
    let copy = gtk::Box::new(gtk::Orientation::Vertical, 3);
    copy.add_css_class("setting-copy");
    copy.set_hexpand(true);
    copy.set_valign(gtk::Align::Center);
    copy.append(&label(title, "setting-title"));
    let detail_label = description(detail, "setting-description");
    detail_label.set_visible(!detail.is_empty());
    if cap {
        // The wrapper follows the description, so a hidden one adds no gap.
        let capped = capped(&detail_label, 52.);
        detail_label
            .bind_property("visible", &capped, "visible")
            .sync_create()
            .build();
        copy.append(&capped);
    } else {
        copy.append(&detail_label);
    }
    (copy, detail_label)
}

/// `.setting-row-inline`: copy on the left, the control on the right.
fn inline_row(title: &str, detail: &str, control: &impl IsA<gtk::Widget>) -> gtk::Box {
    // `.setting-row` column gap: `var(--s-6)`.
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    row.add_css_class("setting-row-inline");
    row.append(&copy(title, detail, true).0);
    let control = control.as_ref();
    control.add_css_class("setting-control");
    control.set_valign(gtk::Align::Center);
    control.set_halign(gtk::Align::End);
    row.append(control);
    row
}

/// `.setting-row-stack`: the title over a full-width control.
fn stack_row(title: &str, detail: &str, control: &impl IsA<gtk::Widget>) -> gtk::Box {
    // `.setting-row` row gap: `var(--s-4)`.
    let row = gtk::Box::new(gtk::Orientation::Vertical, 8);
    row.append(&copy(title, detail, true).0);
    row.append(control);
    row
}

/// `.check-row.switch-row`: copy on the left, the switch on the right, and the
/// whole row toggles.
fn switch_row(
    tokens: &Rc<Tokens>,
    title: &str,
    detail: &str,
    on: bool,
) -> (gtk::Box, TokenSwitch, gtk::Label) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, tokens.number("s-6") as i32);
    let (copy, detail_label) = copy(title, detail, false);
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
    (row, switch, detail_label)
}

/// A switch row bound to a boolean setting.
fn setting_switch(
    prefs: &Rc<Prefs>,
    card: &gtk::Box,
    path: &'static str,
    detail: &str,
) -> (gtk::Box, TokenSwitch, gtk::Label) {
    let copy = preferences::row(path);
    let on = prefs.get(path).as_bool().unwrap_or_default();
    let detail = if detail.is_empty() {
        copy.description
    } else {
        detail
    };
    let (row, switch, detail_label) = switch_row(&prefs.tokens.borrow(), copy.title, detail, on);
    let weak = Rc::downgrade(prefs);
    switch.connect_toggled(move |on| {
        if let Some(prefs) = weak.upgrade() {
            prefs.set(path, Value::Bool(on));
        }
    });
    prefs.switches.borrow_mut().push(switch.clone());
    add(card, &row);
    (row, switch, detail_label)
}

/// A shipping `CustomSelect` bound to a setting. Numeric settings store the
/// chosen option as a number; `off` stores null (Default microphone).
fn select(prefs: &Rc<Prefs>, path: &'static str, options: Vec<(String, String)>) -> Rc<Choice> {
    let current = match prefs.get(path) {
        Value::Null => "off".into(),
        Value::String(value) => value,
        other => other.to_string(),
    };
    let numeric = prefs.get(path).is_number();
    let weak = Rc::downgrade(prefs);
    let choice = Choice::new(
        None,
        preferences::select_accessibility_label(path),
        false,
        move |value| {
            let Some(prefs) = weak.upgrade() else { return };
            let value = if numeric {
                value.parse::<u64>().map_or(Value::Null, Value::from)
            } else if value == "off" {
                Value::Null
            } else {
                Value::String(value)
            };
            prefs.set(path, value);
        },
    );
    choice.set_options(options);
    choice.set(&current);
    prefs.choices.borrow_mut().push(choice.clone());
    choice
}

fn options<T: ToString>(
    values: impl IntoIterator<Item = T>,
    label: impl Fn(&T) -> String,
) -> Vec<(String, String)> {
    values
        .into_iter()
        .map(|value| (value.to_string(), label(&value)))
        .collect()
}

fn pairs(values: &[(&str, &str)]) -> Vec<(String, String)> {
    values
        .iter()
        .map(|(id, name)| ((*id).to_owned(), (*name).to_owned()))
        .collect()
}

/// An inline select row (`minmax(160px, max-content)` control).
fn select_row(
    prefs: &Rc<Prefs>,
    card: &gtk::Box,
    path: &'static str,
    options: Vec<(String, String)>,
) {
    let choice = select(prefs, path, options);
    choice.root.set_size_request(160, -1);
    let copy = preferences::row(path);
    add(
        card,
        &inline_row(copy.title, copy.description, &choice.root),
    );
}

/// `.setting-grid`: stacked selects in equal columns.
fn select_grid(
    prefs: &Rc<Prefs>,
    card: &gtk::Box,
    fields: Vec<(&'static str, Vec<(String, String)>)>,
) {
    let grid = gtk::Grid::new();
    grid.add_css_class("setting-grid");
    // `.setting-grid-three` drops to two columns in a compact window.
    if fields.len() == 3 {
        grid.add_css_class("setting-grid-three");
    }
    grid.set_column_homogeneous(true);
    // `.setting-grid { gap: var(--s-5) }`.
    let gap = prefs.tokens.borrow().number("s-5") as u32;
    grid.set_column_spacing(gap);
    grid.set_row_spacing(gap);
    let columns = fields.len() as i32;
    for (index, (path, options)) in fields.into_iter().enumerate() {
        let choice = select(prefs, path, options);
        let field = stack_row(preferences::row(path).title, "", &choice.root);
        // `.setting-grid .setting-row { gap: var(--s-3) }`.
        field.set_spacing(prefs.tokens.borrow().number("s-3") as i32);
        grid.attach(&field, index as i32 % columns, index as i32 / columns, 1, 1);
    }
    add(card, &grid);
}

/// `.settings-utility-row`: title and body, then an action button.
fn utility_row(title: &str, body: &str, action: &str) -> (gtk::Box, gtk::Button) {
    // A label child, not `with_label`: GTK 4.14 names a label-mode button
    // after its text and drops the accessible label, so both rows read "Open".
    let button = gtk::Button::new();
    button.set_child(Some(&gtk::Label::new(Some(action))));
    button.add_css_class("ui-btn");
    button.update_property(&[gtk::accessible::Property::Label(&format!(
        "{action}: {title}"
    ))]);
    // Shipping's utility rows are their own grid and never stack.
    let row = inline_row(title, body, &button);
    row.remove_css_class("setting-row-inline");
    row.add_css_class("settings-utility-row");
    (row, button)
}

/// A button with a construct-time accessible role (radio choices).
pub(crate) fn role_button(role: gtk::AccessibleRole) -> gtk::Button {
    glib::Object::builder::<gtk::Button>()
        .property("accessible-role", role)
        .build()
}

pub(crate) fn checked(widget: &impl IsA<gtk::Widget>, on: bool) {
    let widget = widget.as_ref();
    if on {
        widget.add_css_class("active");
    } else {
        widget.remove_css_class("active");
    }
    widget.update_state(&[gtk::accessible::State::Checked(if on {
        gtk::AccessibleTristate::True
    } else {
        gtk::AccessibleTristate::False
    })]);
}

fn theme_id(settings: &AppSettings) -> String {
    serde_json::to_value(settings.theme)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| "mustard".into())
}

fn appearance_id(settings: &AppSettings) -> String {
    serde_json::to_value(settings.appearance)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| "system".into())
}

fn rgba(hex: &str) -> Option<gdk::RGBA> {
    gdk::RGBA::parse(hex).ok()
}

/// Appearance: the Interface theme segments, the accent chips and, for
/// Custom, the colour editor.
fn appearance_card(prefs: &Rc<Prefs>) -> gtk::Box {
    let tokens = prefs.tokens.borrow().clone();
    let card = card(&tokens, 1, None);

    // `.ui-segmented.appearance-switch`.
    let segments = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    segments.add_css_class("ui-segmented");
    segments.set_accessible_role(gtk::AccessibleRole::Group);
    segments.update_property(&[gtk::accessible::Property::Label(
        preferences::row("appearance").title,
    )]);
    let current = appearance_id(&prefs.settings.borrow());
    let mut buttons = Vec::new();
    for (id, name) in preferences::APPEARANCE_MODES {
        let button = gtk::Button::with_label(name);
        button.add_css_class("segment");
        pressed(&button, current == id);
        let weak = Rc::downgrade(prefs);
        button.connect_clicked(move |_| {
            if let Some(prefs) = weak.upgrade() {
                prefs.set("appearance", Value::String(id.into()));
            }
        });
        segments.append(&button);
        buttons.push((id, button));
    }
    prefs.listen(move |settings| {
        let current = appearance_id(settings);
        for (id, button) in &buttons {
            pressed(button, current == *id);
        }
    });
    let row = preferences::row("appearance");
    add(&card, &inline_row(row.title, row.description, &segments));

    // `.theme-options`: a radio group of chips, five per row.
    let grid = glib::Object::builder::<gtk::Grid>()
        .property("accessible-role", gtk::AccessibleRole::RadioGroup)
        .build();
    grid.add_css_class("theme-options");
    grid.set_column_homogeneous(true);
    grid.set_column_spacing(tokens.number("s-2") as u32);
    grid.set_row_spacing(tokens.number("s-2") as u32);
    grid.update_property(&[gtk::accessible::Property::Label(
        preferences::row("theme").title,
    )]);
    let current = theme_id(&prefs.settings.borrow());
    let mut chips = Vec::new();
    let columns = preferences::theme_columns(false);
    for (index, theme) in preferences::THEMES.iter().enumerate() {
        let button = role_button(gtk::AccessibleRole::Radio);
        button.add_css_class("theme-option");
        button.set_tooltip_text(Some(theme.description));
        button.update_property(&[gtk::accessible::Property::Label(
            &preferences::theme_accessibility_label(theme),
        )]);
        let content = gtk::Box::new(gtk::Orientation::Horizontal, tokens.number("s-3") as i32);
        let swatch = theme_swatch(prefs, theme.id);
        content.append(&swatch);
        let name = label(theme.name, "theme-option-name");
        name.set_hexpand(true);
        name.set_ellipsize(gtk::pango::EllipsizeMode::End);
        content.append(&name);
        let check = label("✓", "theme-option-check");
        check.set_size_request(10, -1);
        content.append(&check);
        button.set_child(Some(&content));
        checked(&button, current == theme.id);
        let (weak, id) = (Rc::downgrade(prefs), theme.id);
        button.connect_clicked(move |_| {
            if let Some(prefs) = weak.upgrade() {
                prefs.set("theme", Value::String(id.into()));
            }
        });
        grid.attach(
            &button,
            (index % columns) as i32,
            (index / columns) as i32,
            1,
            1,
        );
        chips.push((theme.id, button));
    }
    let row = preferences::row("theme");
    add(&card, &stack_row(row.title, row.description, &grid));

    let editor = custom_theme_editor(prefs);
    editor.set_visible(current == "custom");
    // Not a separated row: shipping nests the editor under the chips.
    card.append(&editor);
    prefs.listen(move |settings| {
        let current = theme_id(settings);
        for (id, button) in &chips {
            checked(button, current == *id);
        }
        editor.set_visible(current == "custom");
    });
    card
}

/// `.theme-option-preview`: the accent with the signal hue as a corner
/// wedge, or Custom's rainbow.
fn theme_swatch(prefs: &Rc<Prefs>, id: &'static str) -> SnapshotArea {
    let weak = Rc::downgrade(prefs);
    let swatch = SnapshotArea::new(move |snapshot, w, h| {
        let Some(prefs) = weak.upgrade() else { return };
        let radius = prefs.tokens.borrow().number("r-sm") as f64;
        let cr = snapshot.append_cairo(&graphene::Rect::new(0., 0., w, h));
        let (w, h) = (f64::from(w), f64::from(h));
        rounded(&cr, 0., 0., w, h, radius);
        cr.clip();
        if id == "custom" {
            // 120deg: towards the lower right.
            let gradient = gtk::cairo::LinearGradient::new(0., h * 0.2, w, h * 0.8);
            for (stop, [r, g, b]) in [
                (0., [1., 0.282, 0.392]),
                (0.18, [1., 0.6, 0.24]),
                (0.34, [0.957, 0.902, 0.306]),
                (0.5, [0.31, 0.875, 0.533]),
                (0.66, [0.282, 0.808, 0.937]),
                (0.84, [0.722, 0.337, 0.957]),
                (1., [1., 0.32, 0.68]),
            ] {
                gradient.add_color_stop_rgb(stop, r, g, b);
            }
            let _ = cr.set_source(&gradient);
            let _ = cr.paint();
        } else {
            let tokens = Tokens::variant(&prefs.appearance.borrow(), id);
            let fill = |name: &str| {
                let c = tokens.rgba(name);
                cr.set_source_rgba(
                    f64::from(c.red()),
                    f64::from(c.green()),
                    f64::from(c.blue()),
                    f64::from(c.alpha()),
                );
            };
            fill("theme-accent");
            let _ = cr.paint();
            fill("theme-signal");
            cr.move_to(w, 0.);
            cr.line_to(w, h);
            cr.line_to(w / 2., h);
            cr.close_path();
            let _ = cr.fill();
        }
        rounded(&cr, 0.5, 0.5, w - 1., h - 1., radius - 0.5);
        cr.set_source_rgba(0., 0., 0., 0.16);
        cr.set_line_width(1.);
        let _ = cr.stroke();
    });
    swatch.set_size_request(18, 18);
    swatch.set_valign(gtk::Align::Center);
    prefs.swatches.borrow_mut().push(swatch.clone());
    swatch
}

fn rounded(cr: &gtk::cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    use std::f64::consts::{FRAC_PI_2, PI};
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -FRAC_PI_2, 0.);
    cr.arc(x + w - r, y + h - r, r, 0., FRAC_PI_2);
    cr.arc(x + r, y + h - r, r, FRAC_PI_2, PI);
    cr.arc(x + r, y + r, r, PI, 1.5 * PI);
    cr.close_path();
}

/// Shipping `.custom-theme-editor`: Accent and Recording signal, each an RGB
/// picker plus a hex field, and Reset colors.
fn custom_theme_editor(prefs: &Rc<Prefs>) -> gtk::Box {
    let copy = &preferences::CUSTOM_THEME;
    let editor = gtk::Box::new(gtk::Orientation::Vertical, 20);
    editor.add_css_class("custom-theme-editor");
    editor.set_accessible_role(gtk::AccessibleRole::Group);
    editor.update_property(&[gtk::accessible::Property::Label("Custom theme colors")]);
    let heading = gtk::Box::new(gtk::Orientation::Horizontal, 24);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 3);
    text.set_hexpand(true);
    text.append(&label(copy.title, "setting-title"));
    text.append(&capped(
        &description(copy.description, "setting-description"),
        44.,
    ));
    heading.append(&text);
    let reset = gtk::Button::with_label(copy.reset);
    reset.add_css_class("ui-btn");
    reset.set_valign(gtk::Align::Start);
    let weak = Rc::downgrade(prefs);
    reset.connect_clicked(move |_| {
        if let Some(prefs) = weak.upgrade() {
            let defaults = captures_settings::CustomThemeSettings::default();
            prefs.set(
                "custom_theme",
                serde_json::to_value(defaults).unwrap_or_default(),
            );
        }
    });
    heading.append(&reset);
    editor.append(&heading);

    let fields = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    fields.set_homogeneous(true);
    for (key, name, detail) in copy.fields {
        let path: &'static str = if key == "accent" {
            "custom_theme.accent"
        } else {
            "custom_theme.signal"
        };
        let field = gtk::Box::new(gtk::Orientation::Vertical, 0);
        field.add_css_class("custom-theme-field");
        field.set_valign(gtk::Align::Start);
        field.append(&label(name, "custom-theme-label"));
        let control = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        control.add_css_class("custom-theme-color-control");
        let value = prefs.get(path).as_str().unwrap_or_default().to_uppercase();
        let entry = gtk::Entry::new();
        entry.add_css_class("custom-theme-hex");
        entry.set_hexpand(true);
        entry.set_max_length(7);
        entry.set_text(&value);
        entry.update_property(&[gtk::accessible::Property::Label(&format!(
            "{name} hex value"
        ))]);
        let picker = gtk::Button::new();
        picker.add_css_class("custom-theme-picker");
        picker.update_property(&[gtk::accessible::Property::Label(&format!(
            "{name} color picker"
        ))]);
        let sample = {
            let entry = entry.clone();
            SnapshotArea::new(move |snapshot, w, h| {
                if let Some(color) = rgba(entry.text().as_str()) {
                    let rect = graphene::Rect::new(0., 0., w, h);
                    let rounded = gtk::gsk::RoundedRect::from_rect(rect, 3.);
                    snapshot.push_rounded_clip(&rounded);
                    snapshot.append_color(&color, &rect);
                    snapshot.pop();
                }
            })
        };
        // `input[type=color]`: 36 × 32 with a 3 px inset swatch.
        sample.set_size_request(28, 24);
        picker.set_valign(gtk::Align::Center);
        picker.set_child(Some(&sample));
        control.append(&picker);
        control.append(&entry);
        field.append(&control);
        let help = description(detail, "custom-theme-help");
        field.append(&help);
        fields.append(&field);

        let weak = Rc::downgrade(prefs);
        let redraw = sample.clone();
        entry.connect_changed(move |entry| {
            redraw.queue_draw();
            let Some(prefs) = weak.upgrade() else { return };
            if let Ok(hex) = captures_settings::theme::normalize_hex_color(entry.text().as_str()) {
                prefs.set(path, Value::String(hex));
            }
        });
        let target = entry.clone();
        prefs.listen(move |settings| {
            let hex = if key == "accent" {
                &settings.custom_theme.accent
            } else {
                &settings.custom_theme.signal
            };
            let shown = captures_settings::theme::normalize_hex_color(target.text().as_str());
            if shown.as_deref().ok() != Some(hex.as_str()) && !target.has_focus() {
                target.set_text(&hex.to_uppercase());
            }
        });
        let entry = entry.clone();
        picker.connect_clicked(move |picker| {
            let window = picker.root().and_downcast::<gtk::Window>();
            #[allow(deprecated)] // `GtkColorDialog` needs GTK 4.10.
            let dialog = gtk::ColorChooserDialog::new(Some(name), window.as_ref());
            #[allow(deprecated)]
            {
                dialog.set_use_alpha(false);
                if let Some(color) = rgba(entry.text().as_str()) {
                    dialog.set_rgba(&color);
                }
                let entry = entry.clone();
                dialog.connect_response(move |dialog, response| {
                    if response == gtk::ResponseType::Ok {
                        entry
                            .set_text(&crate::editor::controls::hex(&dialog.rgba()).to_uppercase());
                    }
                    dialog.destroy();
                });
                dialog.present();
            }
        });
    }
    editor.append(&fields);
    editor
}

/// Capture: the save folder, switches, the preview corner and the
/// screenshot selects.
fn capture_card(prefs: &Rc<Prefs>, window: &gtk::ApplicationWindow) -> gtk::Box {
    let tokens = prefs.tokens.borrow().clone();
    let card = card(&tokens, 2, None);

    // `.directory-input`: the path field and Choose….
    let field = gtk::Box::new(gtk::Orientation::Horizontal, tokens.number("s-4") as i32);
    field.add_css_class("directory-input");
    let entry = gtk::Entry::new();
    entry.add_css_class("directory-entry");
    entry.set_hexpand(true);
    entry.set_text(prefs.get("output_directory").as_str().unwrap_or_default());
    let title = preferences::row("output_directory").title;
    entry.update_property(&[gtk::accessible::Property::Label(title)]);
    let weak = Rc::downgrade(prefs);
    entry.connect_changed(move |entry| {
        if let Some(prefs) = weak.upgrade() {
            prefs.set("output_directory", Value::String(entry.text().into()));
        }
    });
    field.append(&entry);
    let choose = gtk::Button::with_label("Choose…");
    choose.add_css_class("ui-btn");
    let (target, window) = (entry.clone(), window.downgrade());
    #[allow(deprecated)] // `GtkFileDialog` needs GTK 4.10.
    choose.connect_clicked(move |_| {
        let window = window.upgrade();
        let chooser = gtk::FileChooserNative::new(
            Some("Choose capture folder"),
            window.as_ref(),
            gtk::FileChooserAction::SelectFolder,
            Some("Select"),
            Some("Cancel"),
        );
        let _ = chooser.set_current_folder(Some(&gio::File::for_path(target.text().as_str())));
        let target = target.clone();
        chooser.connect_response(move |chooser, response| {
            if response == gtk::ResponseType::Accept
                && let Some(path) = chooser.file().and_then(|file| file.path())
            {
                target.set_text(&path.to_string_lossy());
            }
            chooser.destroy();
        });
        chooser.show();
    });
    field.append(&choose);
    add(&card, &stack_row(title, "", &field));

    setting_switch(prefs, &card, "auto_copy_to_clipboard", "");
    setting_switch(prefs, &card, "auto_start_on_selection", "");
    setting_switch(prefs, &card, "show_mini_previews", "");

    // `.mini-preview-placement`: four corners on a little screen.
    let picker = gtk::Box::new(gtk::Orientation::Vertical, tokens.number("s-2") as i32);
    picker.add_css_class("mini-preview-placement");
    let screen = glib::Object::builder::<gtk::Grid>()
        .property("accessible-role", gtk::AccessibleRole::RadioGroup)
        .build();
    screen.add_css_class("mini-preview-placement-screen");
    screen.set_size_request(108, 72);
    screen.set_halign(gtk::Align::End);
    screen.set_row_homogeneous(true);
    screen.set_column_homogeneous(true);
    screen.update_property(&[gtk::accessible::Property::Label(
        preferences::MINI_PREVIEW_POSITION_LABEL,
    )]);
    let placement = gtk::Label::new(None);
    placement.add_css_class("setting-description");
    placement.set_xalign(1.);
    let current = prefs
        .get("mini_preview_placement")
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let mut corners = Vec::new();
    for (index, (id, name)) in preferences::MINI_PREVIEW_PLACEMENTS.iter().enumerate() {
        let corner = role_button(gtk::AccessibleRole::Radio);
        corner.add_css_class("mini-preview-placement-corner");
        corner.set_size_request(22, 22);
        corner.set_halign(if index % 2 == 0 {
            gtk::Align::Start
        } else {
            gtk::Align::End
        });
        corner.set_valign(if index < 2 {
            gtk::Align::Start
        } else {
            gtk::Align::End
        });
        corner.update_property(&[gtk::accessible::Property::Label(name)]);
        checked(&corner, current == *id);
        let (weak, id) = (Rc::downgrade(prefs), *id);
        corner.connect_clicked(move |_| {
            if let Some(prefs) = weak.upgrade() {
                prefs.set("mini_preview_placement", Value::String(id.into()));
            }
        });
        screen.attach(&corner, (index % 2) as i32, (index / 2) as i32, 1, 1);
        corners.push((id, corner));
    }
    placement.set_label(preferences::mini_preview_placement_name(&current));
    picker.append(&screen);
    picker.append(&placement);
    let row = preferences::row("mini_preview_placement");
    // `.mini-preview-placement { justify-items: end }`, even when stacked.
    picker.add_css_class("keep-end");
    let placement_row = inline_row(row.title, row.description, &picker);
    add(&card, &placement_row);

    let show = prefs.settings.borrow().show_mini_previews;
    let include = prefs.settings.borrow().include_mini_previews_in_captures;
    let (include_row, include_switch, include_detail) = setting_switch(
        prefs,
        &card,
        "include_mini_previews_in_captures",
        preferences::mini_previews_in_captures_description(show, include),
    );
    // Recording controls cannot be kept out of Linux captures (shipping
    // `canExcludeRecordingControls` is false here).
    let (controls_row, controls_switch, controls_detail) =
        setting_switch(prefs, &card, "include_recording_controls_in_captures", " ");
    controls_switch.set_sensitive(false);
    controls_row.add_css_class("disabled");
    let controls_copy = |include: bool| {
        let copy = preferences::recording_controls_description(false, include);
        let escape = |text: &str| glib::markup_escape_text(text).to_string();
        let markup = if copy.emphasis.is_empty() {
            escape(copy.lead)
        } else {
            format!(
                "{}<b>{}</b>{}",
                escape(copy.lead),
                escape(copy.emphasis),
                escape(copy.trail)
            )
        };
        (markup, copy.text())
    };
    let sync_controls = move |include: bool| {
        let (markup, text) = controls_copy(include);
        controls_detail.set_markup(&markup);
        controls_switch.update_property(&[gtk::accessible::Property::Description(&text)]);
    };
    sync_controls(
        prefs
            .settings
            .borrow()
            .include_recording_controls_in_captures,
    );

    let sync_previews = move |settings: &AppSettings| {
        let show = settings.show_mini_previews;
        let current = serde_json::to_value(settings.mini_preview_placement)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_default();
        for (id, corner) in &corners {
            checked(corner, current == *id);
            corner.set_sensitive(show);
        }
        placement.set_label(preferences::mini_preview_placement_name(&current));
        if show {
            picker.remove_css_class("disabled");
            include_row.remove_css_class("disabled");
        } else {
            picker.add_css_class("disabled");
            include_row.add_css_class("disabled");
        }
        include_switch.set_sensitive(show);
        let detail = preferences::mini_previews_in_captures_description(
            show,
            settings.include_mini_previews_in_captures,
        );
        include_detail.set_label(detail);
        include_switch.update_property(&[gtk::accessible::Property::Description(detail)]);
    };
    sync_previews(&prefs.settings.borrow());
    prefs.listen(move |settings| {
        sync_previews(settings);
        sync_controls(settings.include_recording_controls_in_captures);
    });

    setting_switch(prefs, &card, "freeze_screen", "");
    setting_switch(prefs, &card, "show_cursor_in_screenshots", "");
    select_row(
        prefs,
        &card,
        "screenshot_format",
        pairs(&preferences::SCREENSHOT_FORMATS),
    );
    select_row(
        prefs,
        &card,
        "screenshot_countdown_seconds",
        options(preferences::COUNTDOWN_SECONDS, |s| {
            preferences::countdown_label(*s)
        }),
    );
    card
}

/// Shortcuts: the system-keys utility row and the seven recorders.
fn shortcuts_card(prefs: &Rc<Prefs>) -> gtk::Box {
    let tokens = prefs.tokens.borrow().clone();
    let help = preferences::shortcut_help(PLATFORM);
    let intro = format!("{} {}", SECTIONS[3].description, help.intro);
    let card = card(&tokens, 3, Some(&intro));
    let (row, open) = utility_row(help.system_title, help.system_body, help.system_action);
    open.connect_clicked(|button| {
        if let Err(error) = preferences::open_keyboard_settings(PLATFORM) {
            button.set_tooltip_text(Some(&error));
        }
    });
    add(&card, &row);
    let list = gtk::Box::new(gtk::Orientation::Vertical, tokens.number("s-2") as i32);
    list.add_css_class("shortcut-list");
    for (title, path) in preferences::SHORTCUT_ROWS {
        list.append(&shortcut_row(prefs, title, path.join(".")));
    }
    add(&card, &list);
    card
}

/// Shipping `ShortcutInput`: click, then press the combination; Esc or
/// leaving the recorder cancels.
fn shortcut_row(prefs: &Rc<Prefs>, title: &'static str, path: String) -> gtk::Box {
    // `.shortcut-row`: `gap: var(--s-6)`.
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    row.add_css_class("shortcut-row");
    let name = label(title, "shortcut-label");
    name.set_hexpand(true);
    row.append(&name);
    let control = gtk::Box::new(gtk::Orientation::Vertical, 0);
    control.set_size_request(260, -1);
    let recorder = gtk::Button::new();
    recorder.add_css_class("shortcut-recorder");
    recorder.update_relation(&[gtk::accessible::Relation::LabelledBy(&[name.upcast_ref()])]);
    // `.shortcut-recorder`: `gap: var(--s-3)` between key caps.
    let keys = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    recorder.set_child(Some(&keys));
    control.append(&recorder);
    let error = label("", "shortcut-error");
    error.set_wrap(true);
    error.set_visible(false);
    error.set_accessible_role(gtk::AccessibleRole::Status);
    control.append(&error);
    row.append(&control);

    let show = {
        let (keys, recorder) = (keys.clone(), recorder.clone());
        move |tokens: &[String]| {
            while let Some(child) = keys.first_child() {
                keys.remove(&child);
            }
            if tokens.is_empty() {
                keys.append(&label(preferences::SHORTCUT_PROMPT, "shortcut-prompt"));
            }
            for token in tokens {
                let cap = label(token, "kbd");
                cap.set_xalign(0.5);
                keys.append(&cap);
            }
            recorder.update_property(&[gtk::accessible::Property::Description(&tokens.join("+"))]);
        }
    };
    let saved = {
        let (weak, path) = (Rc::downgrade(prefs), path.clone());
        move || {
            weak.upgrade()
                .and_then(|prefs| prefs.get(&path).as_str().map(str::to_owned))
                .map(|value| shortcut_display_tokens(&value, PLATFORM))
                .unwrap_or_default()
        }
    };
    show(&saved());
    let recording = Rc::new(Cell::new(false));
    let stop = Rc::new({
        let (recording, recorder, error, show, saved) = (
            recording.clone(),
            recorder.clone(),
            error.clone(),
            show.clone(),
            saved.clone(),
        );
        move || {
            recording.set(false);
            recorder.remove_css_class("shortcut-recording");
            pressed(&recorder, false);
            error.set_visible(false);
            show(&saved());
        }
    });
    {
        let (recording, error, show) = (recording.clone(), error.clone(), show.clone());
        recorder.connect_clicked(move |recorder| {
            recording.set(true);
            recorder.add_css_class("shortcut-recording");
            pressed(recorder, true);
            error.set_visible(false);
            show(&[]);
            recorder.grab_focus();
        });
    }
    let keyboard = gtk::EventControllerKey::new();
    keyboard.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let (recording, stop, error, show) =
            (recording.clone(), stop.clone(), error.clone(), show.clone());
        let weak = Rc::downgrade(prefs);
        keyboard.connect_key_pressed(move |controller, keyval, keycode, state| {
            if !recording.get() {
                return glib::Propagation::Proceed;
            }
            // `KeyboardEvent.code` is the unshifted key, as shipping records.
            let base = controller
                .widget()
                .map_or_else(
                    || gdk::Display::default().expect("a display"),
                    |w| w.display(),
                )
                .translate_key(keycode, gdk::ModifierType::empty(), 0)
                .map_or(keyval, |(key, ..)| key);
            let Some(code) = dom_code(base) else {
                return glib::Propagation::Stop;
            };
            let event = ShortcutKeyEvent {
                code,
                ctrl_key: state.contains(gdk::ModifierType::CONTROL_MASK),
                shift_key: state.contains(gdk::ModifierType::SHIFT_MASK),
                alt_key: state.contains(gdk::ModifierType::ALT_MASK),
                meta_key: state
                    .intersects(gdk::ModifierType::SUPER_MASK | gdk::ModifierType::META_MASK),
            };
            match record_shortcut(&event, PLATFORM) {
                ShortcutRecording::Cancel => stop(),
                ShortcutRecording::Waiting { keys } => {
                    error.set_visible(false);
                    show(&keys);
                }
                ShortcutRecording::Invalid { keys, message } => {
                    show(&keys);
                    error.set_label(&message);
                    error.set_visible(true);
                }
                ShortcutRecording::Complete { shortcut, .. } => {
                    if let Some(prefs) = weak.upgrade() {
                        prefs.set(&path, Value::String(shortcut));
                    }
                    stop();
                }
            }
            glib::Propagation::Stop
        });
    }
    recorder.add_controller(keyboard);
    let focus = gtk::EventControllerFocus::new();
    {
        let (recording, stop) = (recording.clone(), stop.clone());
        focus.connect_leave(move |_| {
            if recording.get() {
                stop();
            }
        });
    }
    recorder.add_controller(focus);
    row
}

/// A GDK key as the DOM `KeyboardEvent.code` shipping records shortcuts in.
fn dom_code(key: gdk::Key) -> Option<String> {
    let name = key.name()?.to_string();
    if let Some(c) = key.to_unicode().filter(char::is_ascii_alphanumeric) {
        return Some(if c.is_ascii_digit() {
            format!("Digit{c}")
        } else {
            format!("Key{}", c.to_ascii_uppercase())
        });
    }
    if let Some(digit) = name.strip_prefix("KP_").filter(|d| d.len() == 1) {
        return Some(format!("Numpad{digit}"));
    }
    if name.len() > 1 && name.starts_with('F') && name[1..].parse::<u8>().is_ok() {
        return Some(name);
    }
    Some(
        match name.as_str() {
            "Control_L" => "ControlLeft",
            "Control_R" => "ControlRight",
            "Shift_L" => "ShiftLeft",
            "Shift_R" => "ShiftRight",
            "Alt_L" => "AltLeft",
            "Alt_R" | "ISO_Level3_Shift" => "AltRight",
            "Super_L" | "Meta_L" => "MetaLeft",
            "Super_R" | "Meta_R" => "MetaRight",
            "Escape" => "Escape",
            "Print" | "Sys_Req" => "PrintScreen",
            "space" => "Space",
            "Return" => "Enter",
            "KP_Enter" => "NumpadEnter",
            "Tab" | "ISO_Left_Tab" => "Tab",
            "BackSpace" => "Backspace",
            "Delete" => "Delete",
            "Insert" => "Insert",
            "Home" => "Home",
            "End" => "End",
            "Page_Up" | "Prior" => "PageUp",
            "Page_Down" | "Next" => "PageDown",
            "Up" => "ArrowUp",
            "Down" => "ArrowDown",
            "Left" => "ArrowLeft",
            "Right" => "ArrowRight",
            "minus" => "Minus",
            "equal" => "Equal",
            "bracketleft" => "BracketLeft",
            "bracketright" => "BracketRight",
            "backslash" => "Backslash",
            "semicolon" => "Semicolon",
            "apostrophe" => "Quote",
            "grave" => "Backquote",
            "comma" => "Comma",
            "period" => "Period",
            "slash" => "Slash",
            "Caps_Lock" => "CapsLock",
            "Num_Lock" => "NumLock",
            "Scroll_Lock" => "ScrollLock",
            "Pause" => "Pause",
            "KP_Add" => "NumpadAdd",
            "KP_Subtract" => "NumpadSubtract",
            "KP_Multiply" => "NumpadMultiply",
            "KP_Divide" => "NumpadDivide",
            "KP_Decimal" => "NumpadDecimal",
            "KP_Equal" => "NumpadEqual",
            "XF86AudioLowerVolume" => "AudioVolumeDown",
            "XF86AudioRaiseVolume" => "AudioVolumeUp",
            "XF86AudioMute" => "AudioVolumeMute",
            "XF86AudioPlay" => "MediaPlayPause",
            "XF86AudioStop" => "MediaStop",
            "XF86AudioNext" => "MediaTrackNext",
            "XF86AudioPrev" => "MediaTrackPrevious",
            _ => return None,
        }
        .to_owned(),
    )
}

/// Recording: format, rate and size, countdown, microphone and the toggles.
fn recording_card(prefs: &Rc<Prefs>) -> gtk::Box {
    let card = card(&prefs.tokens.borrow(), 4, None);
    select_row(
        prefs,
        &card,
        "recording.video_format",
        pairs(&preferences::RECORDING_FORMATS),
    );
    select_grid(
        prefs,
        &card,
        vec![
            (
                "recording.video_fps",
                options(preferences::RECORDING_FPS, |fps| {
                    preferences::fps_label(*fps)
                }),
            ),
            (
                "recording.video_max_resolution",
                pairs(&preferences::RESOLUTIONS),
            ),
        ],
    );
    select_row(
        prefs,
        &card,
        "recording.countdown_seconds",
        options(preferences::COUNTDOWN_SECONDS, |s| {
            preferences::countdown_label(*s)
        }),
    );
    // Off and the saved device until the shared engine's enumeration (cpal's
    // ALSA host, which PipeWire desktops serve through pipewire-alsa) returns
    // off the main thread, as shipping lists `list_recording_audio_devices`.
    let saved = prefs
        .get("recording.microphone_device_id")
        .as_str()
        .map(|id| {
            (
                id.to_owned(),
                id.strip_prefix("microphone:").unwrap_or(id).to_owned(),
            )
        });
    let microphones = move |devices: Vec<(String, String)>| {
        let mut list = vec![("off".to_owned(), preferences::MICROPHONE_OFF.to_owned())];
        list.extend(devices);
        if let Some(saved) = &saved
            && !list.iter().any(|(id, _)| *id == saved.0)
        {
            list.push(saved.clone());
        }
        list
    };
    let microphone = select(
        prefs,
        "recording.microphone_device_id",
        microphones(Vec::new()),
    );
    microphone.root.set_size_request(160, -1);
    let copy = preferences::row("recording.microphone_device_id");
    add(
        &card,
        &inline_row(copy.title, copy.description, &microphone.root),
    );
    let weak = Rc::downgrade(&microphone);
    glib::spawn_future_local(async move {
        let Ok(devices) = gio::spawn_blocking(|| {
            captures_recording_platform::microphone_devices()
                .into_iter()
                .map(|device| (device.id, device.name))
                .collect::<Vec<_>>()
        })
        .await
        else {
            return;
        };
        if let Some(choice) = weak.upgrade() {
            choice.set_options(microphones(devices));
        }
    });
    for key in preferences::RECORDING_TOGGLES {
        let path: &'static str = match key {
            "capture_system_audio" => "recording.capture_system_audio",
            "mono_audio" => "recording.mono_audio",
            "show_cursor" => "recording.show_cursor",
            "highlight_clicks" => "recording.highlight_clicks",
            _ => "recording.open_editor_after_recording",
        };
        setting_switch(prefs, &card, path, "");
    }
    card
}

/// GIF export: three selects side by side.
fn gif_card(prefs: &Rc<Prefs>) -> gtk::Box {
    let card = card(&prefs.tokens.borrow(), 5, None);
    select_grid(
        prefs,
        &card,
        vec![
            (
                "recording.gif_fps",
                options(preferences::GIF_FPS, |fps| preferences::fps_label(*fps)),
            ),
            (
                "recording.gif_max_width",
                options(preferences::GIF_MAX_WIDTHS, |w| {
                    preferences::width_label(*w)
                }),
            ),
            (
                "recording.gif_max_colors",
                options(preferences::GIF_PALETTE_COLORS, u16::to_string),
            ),
        ],
    );
    card
}

/// Updates: shipping's `UpdatePreferences`. Build identity beside the action
/// and its status, the install progress under them, then the open-captures
/// warning, the error, the captur.es fallback and the changelog switch.
/// Development builds show the disabled Check Now and "Updates unavailable".
fn updates_card(prefs: &Rc<Prefs>) -> gtk::Box {
    let tokens = prefs.tokens.borrow().clone();
    let s = |name: &str| tokens.number(name) as i32;
    let preview = crate::updates::current().is_some();
    let card = card(&tokens, 6, preview.then_some(updates::HEADER));
    // `.update-settings-row`: two equal columns, then the progress bar
    // spanning both (`grid-row: 2`, `--s-6` below).
    let outer = gtk::Box::new(gtk::Orientation::Vertical, s("s-6"));
    let row = gtk::Box::new(gtk::Orientation::Horizontal, s("s-6"));
    row.set_homogeneous(true);
    row.add_css_class("update-settings-row");
    let (identity, channel) = copy(
        preferences::UPDATES_VERSION,
        preferences::UPDATES_CHANNEL,
        false,
    );
    let version = identity
        .first_child()
        .and_downcast::<gtk::Label>()
        .expect("setting title");
    identity.set_valign(gtk::Align::Start);
    identity.set_margin_top(s("s-3"));
    row.append(&identity);
    // `.update-settings-actions`: right-aligned, `--s-3` apart.
    let actions = gtk::Box::new(gtk::Orientation::Vertical, s("s-3"));
    actions.set_halign(gtk::Align::End);
    // A label child, not `with_label`: GTK 4.14 drops a label-mode button's
    // accessible label.
    let button = gtk::Button::new();
    button.set_child(Some(&gtk::Label::new(Some(preferences::UPDATES_ACTION))));
    button.add_css_class("ui-btn");
    button.set_size_request(
        if preview {
            2 * s("s-12")
        } else {
            preferences::UPDATES_ACTION_WIDTH as i32
        },
        -1,
    );
    button.set_halign(gtk::Align::End);
    actions.append(&button);
    // `.update-settings-status` (`role="status"`): heading, then `small`.
    let status = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(s("s-1"))
        .accessible_role(gtk::AccessibleRole::Status)
        .build();
    status.update_property(&[gtk::accessible::Property::Label("Update status")]);
    let heading = description(preferences::UPDATES_TITLE, "update-settings-status");
    let detail = description("", "update-settings-status");
    for line in [&heading, &detail] {
        line.set_xalign(1.);
        line.set_justify(gtk::Justification::Right);
        status.append(line);
    }
    actions.append(&status);
    row.append(&actions);
    outer.append(&row);
    let progress = gtk::ProgressBar::new();
    progress.add_css_class("update-settings-progress");
    progress.set_pulse_step(0.05);
    progress.update_property(&[
        gtk::accessible::Property::Label("Installing update"),
        gtk::accessible::Property::ValueMin(0.),
        gtk::accessible::Property::ValueMax(100.),
    ]);
    progress.set_visible(false);
    outer.append(&progress);
    add(&card, &outer);
    let warning = description("", "update-settings-warning");
    warning.set_visible(false);
    add(&card, &warning);
    let error = description("", "update-settings-error");
    error.set_accessible_role(gtk::AccessibleRole::Alert);
    error.set_visible(false);
    add(&card, &error);
    // Shipping's Linux fallback: "download from captur.es" is a real button
    // inline after the first sentence; the Debian/AppImage guidance follows
    // on the next line, since an inline tail cannot wrap a long sentence.
    let fallback = gtk::Box::new(gtk::Orientation::Vertical, 0);
    fallback.append(&crate::notices::inline(
        crate::updates::FALLBACK_BEFORE,
        Some((
            crate::updates::FALLBACK_LINK,
            crate::updates::FALLBACK_LINK,
            Box::new(|| {
                if let Err(error) = gio::AppInfo::launch_default_for_uri(
                    crate::updates::DOWNLOAD_PAGE_URL,
                    gio::AppLaunchContext::NONE,
                ) {
                    eprintln!("Could not open captur.es: {error}");
                }
            }),
        )),
        ".",
        "update-download-fallback",
    ));
    fallback.append(&description(
        crate::updates::FALLBACK_AFTER,
        "update-download-fallback",
    ));
    fallback.add_css_class("update-download-fallback-row");
    fallback.set_visible(false);
    add(&card, &fallback);
    *prefs.updates.borrow_mut() = Some(updates::connect(updates::Widgets {
        version,
        channel,
        button,
        heading,
        detail,
        progress,
        warning,
        error,
        fallback: fallback.upcast(),
    }));
    setting_switch(prefs, &card, "show_update_changelog", "");
    card
}

/// About: Send feedback.
fn about_card(prefs: &Rc<Prefs>) -> gtk::Box {
    let card = card(&prefs.tokens.borrow(), 7, None);
    let (row, open) = utility_row(
        preferences::FEEDBACK_TITLE,
        preferences::FEEDBACK_DETAIL,
        preferences::FEEDBACK_ACTION,
    );
    let prefs = Rc::downgrade(prefs);
    open.connect_clicked(move |_| {
        if let Some(prefs) = prefs.upgrade() {
            (prefs.open_feedback)();
        }
    });
    add(&card, &row);
    card
}

/// General: Start Captures on login, queried and changed off the UI thread.
fn login_item_row(prefs: &Rc<Prefs>, history_root: Option<PathBuf>) -> gtk::Box {
    let (detail, available) = match &history_root {
        Some(_) => (preferences::LOGIN_ITEM_CHECKING, true),
        None => (preferences::login_item_unavailable(PLATFORM), false),
    };
    let tokens = prefs.tokens.borrow().clone();
    let (row, switch, detail_label) =
        switch_row(&tokens, preferences::LOGIN_ITEM_TITLE, detail, false);
    prefs.switches.borrow_mut().push(switch.clone());
    switch.set_sensitive(false);
    if !available {
        row.add_css_class("disabled");
        return row;
    }
    let root = history_root.expect("checked above");
    let settings_file = prefs.settings_file.clone();
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
