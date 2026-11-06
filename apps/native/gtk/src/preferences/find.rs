//! Preferences Find, shipping `usePreferencesFind` and `PreferencesFindBar`
//! (`App.tsx`, `lib/preferencesFind.ts`): Ctrl+F opens a second header row,
//! matching card headers and rows get the selected fill, the current match
//! the accent ring, and it scrolls to the centre of the page.
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use captures_app::preferences;
use gtk::prelude::*;
use gtk::{gdk, glib};

use crate::widgets::Icon;

/// Shipping `PREFERENCE_FIND_SELECTOR`, as CSS classes.
const TARGETS: [&str; 6] = [
    "settings-card-header",
    "setting-row",
    "check-row",
    "settings-utility-row",
    "shortcut-row",
    "custom-theme-editor",
];
const MATCH: &str = "preference-find-match";
const CURRENT: &str = "preference-find-current";
/// `scrollIntoView({ behavior: "smooth" })`, roughly Chromium's duration.
const SCROLL_DURATION: Duration = Duration::from_millis(260);

pub struct Find {
    pub bar: gtk::Box,
    entry: gtk::Entry,
    count: gtk::Label,
    steps: [gtk::Button; 2],
    sections: gtk::Box,
    scroller: gtk::ScrolledWindow,
    matches: RefCell<Vec<gtk::Widget>>,
    current: Cell<usize>,
    /// Bumped per scroll so a newer one cancels an animation in flight.
    scroll_generation: Rc<Cell<u64>>,
}

pub fn build(
    window: &gtk::ApplicationWindow,
    sections: &gtk::Box,
    scroller: &gtk::ScrolledWindow,
    spacing: i32,
) -> Rc<Find> {
    let bar = glib::Object::builder::<gtk::Box>()
        .property("accessible-role", gtk::AccessibleRole::Search)
        .property("orientation", gtk::Orientation::Horizontal)
        .property("spacing", spacing)
        .build();
    bar.add_css_class("preferences-find");
    bar.set_visible(false);
    let entry = gtk::Entry::new();
    entry.add_css_class("preferences-find-input");
    entry.set_hexpand(true);
    entry.set_placeholder_text(Some(preferences::FIND_PLACEHOLDER));
    entry.update_property(&[gtk::accessible::Property::Label(
        preferences::FIND_PLACEHOLDER,
    )]);
    bar.append(&entry);
    let count = gtk::Label::new(None);
    count.add_css_class("preferences-find-count");
    count.set_xalign(0.);
    // `min-width: 5.5rem` at the 13 px root size.
    count.set_size_request(72, -1);
    count.set_accessible_role(gtk::AccessibleRole::Status);
    bar.append(&count);
    let button = |icon: &str, label: &str, class: &str| {
        let button = gtk::Button::new();
        button.add_css_class(class);
        let glyph = Icon::new(icon, 14).with_stroke(1.7);
        glyph.set_halign(gtk::Align::Center);
        glyph.set_valign(gtk::Align::Center);
        button.set_child(Some(&glyph));
        button.set_valign(gtk::Align::Center);
        button.update_property(&[gtk::accessible::Property::Label(label)]);
        bar.append(&button);
        button
    };
    let previous = button(
        "chevron-up",
        preferences::FIND_PREVIOUS,
        "preferences-find-step",
    );
    let next = button(
        "chevron-down",
        preferences::FIND_NEXT,
        "preferences-find-step",
    );
    let close = button("close", preferences::FIND_CLOSE, "preferences-find-close");

    let find = Rc::new(Find {
        bar,
        entry,
        count,
        steps: [previous.clone(), next.clone()],
        sections: sections.clone(),
        scroller: scroller.clone(),
        matches: RefCell::default(),
        current: Cell::new(0),
        scroll_generation: Rc::default(),
    });
    find.refresh(false);

    let weak = Rc::downgrade(&find);
    find.entry.connect_changed(move |_| {
        if let Some(find) = weak.upgrade() {
            find.current.set(0);
            find.refresh(true);
        }
    });
    let weak = Rc::downgrade(&find);
    find.entry.connect_activate(move |_| {
        if let Some(find) = weak.upgrade() {
            find.step(1);
        }
    });
    // Shift+Enter in the field steps back (`activate` carries no modifiers).
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let weak = Rc::downgrade(&find);
    keys.connect_key_pressed(move |_, key, _, state| {
        let enter = matches!(key, gdk::Key::Return | gdk::Key::KP_Enter);
        if enter
            && state.contains(gdk::ModifierType::SHIFT_MASK)
            && let Some(find) = weak.upgrade()
        {
            find.step(-1);
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    find.entry.add_controller(keys);
    for (button, delta) in [(&previous, -1), (&next, 1)] {
        let weak = Rc::downgrade(&find);
        button.connect_clicked(move |_| {
            if let Some(find) = weak.upgrade() {
                find.step(delta);
            }
        });
    }
    let weak = Rc::downgrade(&find);
    close.connect_clicked(move |_| {
        if let Some(find) = weak.upgrade() {
            find.close();
        }
    });

    // Bubble phase: a shortcut recorder that is recording stops its keys
    // first, as shipping skips find while `recordingShortcut`.
    let keys = gtk::EventControllerKey::new();
    let weak = Rc::downgrade(&find);
    keys.connect_key_pressed(move |_, key, _, state| {
        let Some(find) = weak.upgrade() else {
            return glib::Propagation::Proceed;
        };
        let mods = state
            & (gdk::ModifierType::CONTROL_MASK
                | gdk::ModifierType::SHIFT_MASK
                | gdk::ModifierType::ALT_MASK
                | gdk::ModifierType::SUPER_MASK
                | gdk::ModifierType::META_MASK);
        let ctrl = gdk::ModifierType::CONTROL_MASK;
        let shift = gdk::ModifierType::SHIFT_MASK;
        let letter = key.to_lower();
        if letter == gdk::Key::f && mods == ctrl {
            find.open();
            return glib::Propagation::Stop;
        }
        if !find.bar.get_visible() {
            return glib::Propagation::Proceed;
        }
        let delta = if key == gdk::Key::Escape && (mods - shift).is_empty() {
            find.close();
            return glib::Propagation::Stop;
        } else if (letter == gdk::Key::g && mods == ctrl)
            || (key == gdk::Key::F3 && mods.is_empty())
        {
            1
        } else if (letter == gdk::Key::g && mods == ctrl | shift)
            || (key == gdk::Key::F3 && mods == shift)
        {
            -1
        } else {
            return glib::Propagation::Proceed;
        };
        find.step(delta);
        glib::Propagation::Stop
    });
    window.add_controller(keys);
    find
}

impl Find {
    /// Ctrl+F: show the bar and select the query (shipping `revealFind`).
    pub fn open(&self) {
        self.bar.set_visible(true);
        self.entry.grab_focus();
        self.entry.select_region(0, -1);
        self.refresh(true);
    }

    /// `--find`: open with a typed query.
    pub fn open_with(self: &Rc<Self>, query: &str) {
        self.bar.set_visible(true);
        self.entry.set_text(query);
        self.entry.grab_focus();
        self.entry.set_position(-1);
        // The page has no layout yet: scroll once the second frame has one.
        let weak = Rc::downgrade(self);
        let frames = Cell::new(0);
        self.scroller.add_tick_callback(move |_, _| {
            frames.set(frames.get() + 1);
            if frames.get() < 2 {
                return glib::ControlFlow::Continue;
            }
            if let Some(find) = weak.upgrade() {
                find.refresh(true);
            }
            glib::ControlFlow::Break
        });
    }

    /// Escape or Close hides the bar and its highlights but keeps the query.
    fn close(&self) {
        self.bar.set_visible(false);
        self.set_matches(Vec::new());
        self.scroll_generation.set(self.scroll_generation.get() + 1);
    }

    fn step(&self, delta: isize) {
        let count = self.matches.borrow().len();
        if count == 0 {
            return;
        }
        let current = (self.current.get() as isize + delta).rem_euclid(count as isize);
        self.current.set(current as usize);
        self.refresh(true);
    }

    /// Re-match the page (rows can appear or hide), mark the matches, update
    /// the count, and optionally bring the current match to the centre.
    fn refresh(&self, scroll: bool) {
        let query = self.entry.text();
        let matches = if self.bar.get_visible() {
            let mut found = Vec::new();
            collect(self.sections.upcast_ref(), &query, &mut found);
            found
        } else {
            Vec::new()
        };
        let count = matches.len();
        let index = self.current.get().min(count.saturating_sub(1));
        self.current.set(index);
        self.set_matches(matches);
        self.count
            .set_label(&preferences::find_count_label(&query, count, index));
        for step in &self.steps {
            step.set_sensitive(count > 0);
        }
        if scroll && let Some(current) = self.matches.borrow().get(index) {
            self.scroll_to(current);
        }
    }

    fn set_matches(&self, matches: Vec<gtk::Widget>) {
        for widget in self.matches.borrow().iter() {
            widget.remove_css_class(MATCH);
            widget.remove_css_class(CURRENT);
        }
        for (index, widget) in matches.iter().enumerate() {
            widget.add_css_class(MATCH);
            if index == self.current.get() {
                widget.add_css_class(CURRENT);
            }
        }
        *self.matches.borrow_mut() = matches;
    }

    /// `scrollIntoView({ block: "center" })`, smooth unless animations are
    /// off (GTK's reduced-motion setting).
    fn scroll_to(&self, target: &gtk::Widget) {
        let viewport = self.scroller.child().expect("viewport");
        let content = viewport.first_child().unwrap_or(viewport);
        let Some(bounds) = target.compute_bounds(&content) else {
            return;
        };
        let adjustment = self.scroller.vadjustment();
        let goal = (f64::from(bounds.y()) + f64::from(bounds.height()) / 2.
            - adjustment.page_size() / 2.)
            .clamp(
                adjustment.lower(),
                (adjustment.upper() - adjustment.page_size()).max(adjustment.lower()),
            );
        let generation = self.scroll_generation.get() + 1;
        self.scroll_generation.set(generation);
        let animate =
            self.scroller.settings().is_gtk_enable_animations() && self.scroller.is_mapped();
        if !animate {
            adjustment.set_value(goal);
            return;
        }
        let from = adjustment.value();
        let generation_cell = self.scroll_generation.clone();
        let start = Cell::new(None);
        self.scroller.add_tick_callback(move |_, clock| {
            if generation_cell.get() != generation {
                return glib::ControlFlow::Break;
            }
            let now = clock.frame_time();
            let begun = start.get().unwrap_or(now);
            start.set(Some(begun));
            let t = ((now - begun) as f64 / SCROLL_DURATION.as_micros() as f64).min(1.);
            let eased = 1. - (1. - t).powi(3);
            adjustment.set_value(from + (goal - from) * eased);
            if t < 1. {
                glib::ControlFlow::Continue
            } else {
                glib::ControlFlow::Break
            }
        });
    }
}

/// Target widgets under `widget` whose text matches, in page order. Hidden
/// rows and popover option lists are not part of the page.
fn collect(widget: &gtk::Widget, query: &str, found: &mut Vec<gtk::Widget>) {
    if !widget.get_visible() || widget.is::<gtk::Popover>() {
        return;
    }
    if TARGETS.iter().any(|class| widget.has_css_class(class)) {
        let mut text = String::new();
        text_content(widget, &mut text);
        if preferences::find_matches(&text, query) {
            found.push(widget.clone());
        }
    }
    let mut child = widget.first_child();
    while let Some(widget) = child {
        collect(&widget, query, found);
        child = widget.next_sibling();
    }
}

/// The widget's visible label text, like the DOM's `textContent`.
fn text_content(widget: &gtk::Widget, out: &mut String) {
    if !widget.get_visible() || widget.is::<gtk::Popover>() {
        return;
    }
    if let Some(label) = widget.downcast_ref::<gtk::Label>() {
        out.push(' ');
        out.push_str(&label.text());
    }
    let mut child = widget.first_child();
    while let Some(widget) = child {
        text_content(&widget, out);
        child = widget.next_sibling();
    }
}
