//! Shipping New Capture's floating command bar (`.recording-selector-panel`
//! in `RecordingSelector`): Close, the Screenshot/Record and
//! Region/Window/Full screen segments, the Aspect or Display select, the
//! primary action and the note under them. Real GTK widgets, so AT-SPI sees
//! the same buttons, toggles and comboboxes as shipping's DOM; copy and
//! states come from the shared `capture_menu`.

use std::rc::Rc;

use captures_app::capture_menu::{self, MenuMode, PrimaryState};
use gtk::prelude::*;

use crate::editor::controls::Choice;
use crate::widgets::Icon;

/// Shipping `REGION_ASPECT_PRESETS`.
pub const ASPECT_PRESETS: [(&str, &str, Option<f64>); 6] = [
    ("free", "Free", None),
    ("1:1", "1 : 1", Some(1.)),
    ("4:3", "4 : 3", Some(4. / 3.)),
    ("3:2", "3 : 2", Some(3. / 2.)),
    ("16:9", "16 : 9", Some(16. / 9.)),
    ("9:16", "9 : 16", Some(9. / 16.)),
];

/// Shipping's title on the Record segment when recording is unavailable.
pub const RECORDING_UNAVAILABLE: &str = "Screen recording is not available on this platform";
/// Shipping's title on the Window segment when no windows are listed.
pub const WINDOW_UNAVAILABLE: &str = "Window capture is not available in this desktop session";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Region,
    Window,
    Display,
}

pub struct Bar {
    pub root: gtk::Box,
    pub close: gtk::Button,
    pub targets: [gtk::ToggleButton; 3],
    aspect: Rc<Choice>,
    aspect_box: gtk::Box,
    display: Rc<Choice>,
    display_box: gtk::Box,
    several_displays: bool,
    pub primary: gtk::Button,
    primary_label: gtk::Label,
    confirm: gtk::Box,
    auto_start: gtk::Label,
    note: gtk::Box,
}

fn segment(icon: Option<&str>, label: &str) -> gtk::ToggleButton {
    // Shipping's `aria-pressed` buttons (a toggle button from GTK 4.10;
    // 4.8 reports a push button with the pressed state).
    let button = gtk::ToggleButton::new();
    button.add_css_class("capture-segment");
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    content.set_halign(gtk::Align::Center);
    match icon {
        Some(icon) => {
            let glyph = Icon::new(icon, 15).with_stroke(1.7);
            glyph.set_valign(gtk::Align::Center);
            content.append(&glyph);
        }
        None => {
            let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            dot.add_css_class("capture-record-dot");
            dot.set_valign(gtk::Align::Center);
            content.append(&dot);
        }
    }
    content.append(&gtk::Label::new(Some(label)));
    button.set_child(Some(&content));
    button.update_property(&[gtk::accessible::Property::Label(label)]);
    button
}

fn group(label: &str, buttons: &[&gtk::ToggleButton]) -> gtk::Box {
    let group = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .accessible_role(gtk::AccessibleRole::Group)
        .build();
    group.add_css_class("capture-switch");
    group.update_property(&[gtk::accessible::Property::Label(label)]);
    for button in buttons {
        group.append(*button);
    }
    for button in &buttons[1..] {
        button.set_group(Some(buttons[0]));
    }
    group
}

fn text(copy: &str, class: Option<&str>) -> gtk::Label {
    let label = gtk::Label::new(Some(copy));
    if let Some(class) = class {
        label.add_css_class(class);
    }
    label
}

impl Bar {
    /// `pick_aspect` and `pick_display` receive the chosen option id.
    pub fn new(
        displays: Vec<(String, String)>,
        display: &str,
        auto_start: bool,
        pick_aspect: impl Fn(String) + 'static,
        pick_display: impl Fn(String) + 'static,
    ) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("capture-bar");
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        top.add_css_class("capture-bar-top");
        root.append(&top);

        let close = gtk::Button::new();
        close.add_css_class("capture-bar-close");
        let glyph = Icon::new("close", 15).with_stroke(2.);
        glyph.set_halign(gtk::Align::Center);
        glyph.set_valign(gtk::Align::Center);
        close.set_child(Some(&glyph));
        close.update_property(&[gtk::accessible::Property::Label("Close capture controls")]);
        top.append(&close);

        let screenshot = segment(Some("capture"), "Screenshot");
        let record = segment(None, "Record");
        screenshot.set_active(true);
        // The GTK host has no recorder yet: shipping's unavailable state.
        record.set_sensitive(false);
        record.set_tooltip_text(Some(RECORDING_UNAVAILABLE));
        top.append(&group("Capture type", &[&screenshot, &record]));
        let divider = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        divider.add_css_class("capture-bar-divider");
        divider.set_valign(gtk::Align::Center);
        top.append(&divider);

        let targets = [
            segment(Some("target-region"), "Region"),
            segment(Some("target-window"), "Window"),
            segment(Some("target-display"), "Full screen"),
        ];
        targets[0].set_active(true);
        top.append(&group(
            "Capture target",
            &[&targets[0], &targets[1], &targets[2]],
        ));

        let aspect_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        aspect_box.add_css_class("capture-bar-aspect");
        aspect_box.append(&text("Aspect", Some("capture-bar-aspect-label")));
        let aspect = Choice::new(None, "Region aspect ratio", false, pick_aspect);
        aspect.set_glass();
        aspect.set_options(
            ASPECT_PRESETS
                .iter()
                .map(|(id, label, _)| ((*id).to_owned(), (*label).to_owned()))
                .collect(),
        );
        aspect.set("free");
        aspect.root.set_hexpand(true);
        aspect_box.append(&aspect.root);
        top.append(&aspect_box);

        let display_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        display_box.add_css_class("capture-bar-display");
        let several = displays.len() > 1;
        let picker = Choice::new(None, "Display", false, pick_display);
        picker.set_glass();
        picker.set_options(displays);
        picker.set(display);
        picker.set_sensitive(several);
        picker.root.set_hexpand(true);
        display_box.append(&picker.root);
        display_box.set_visible(false);
        top.append(&display_box);

        let primary = gtk::Button::new();
        primary.add_css_class("capture-bar-primary");
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        content.set_halign(gtk::Align::Center);
        let glyph = Icon::new("capture", 16).with_stroke(1.8);
        glyph.set_valign(gtk::Align::Center);
        content.append(&glyph);
        let primary_label = gtk::Label::new(None);
        content.append(&primary_label);
        primary.set_child(Some(&content));
        primary.set_hexpand(true);
        primary.set_halign(gtk::Align::End);
        top.append(&primary);

        // "These controls will show in screenshots · Press Enter to confirm".
        // Read as one sentence, as shipping's text node.
        let note = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .accessible_role(gtk::AccessibleRole::Note)
            .build();
        note.add_css_class("capture-bar-note");
        note.set_halign(gtk::Align::Center);
        let visibility = capture_menu::visibility_note(MenuMode::Screenshot, false, false);
        note.append(&text(visibility.lead.trim_end(), None));
        note.append(&text(visibility.emphasis, Some("emphasis")));
        note.append(&text(visibility.trail.trim_start(), None));
        note.append(&text(capture_menu::NOTE_SEPARATOR, None));
        let confirm = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        confirm.add_css_class("capture-bar-confirm");
        let (before, after) = capture_menu::CONFIRM_NOTE
            .split_once("Enter")
            .expect("the confirm note names Enter");
        confirm.append(&text(before.trim_end(), None));
        let key = text("Enter", Some("kbd"));
        key.set_valign(gtk::Align::Center);
        confirm.append(&key);
        confirm.append(&text(after.trim_start(), None));
        note.append(&confirm);
        let auto_start_label = text(capture_menu::AUTO_START_NOTE, None);
        note.append(&auto_start_label);
        root.append(&note);

        let bar = Self {
            root,
            close,
            targets,
            aspect,
            aspect_box,
            display: picker,
            display_box,
            several_displays: several,
            primary,
            primary_label,
            confirm,
            auto_start: auto_start_label,
            note,
        };
        bar.sync(Target::Region, false, auto_start, PrimaryState::default());
        bar
    }

    /// Shipping disables Window when the session lists no windows.
    pub fn set_window_available(&self, available: bool) {
        self.targets[1].set_sensitive(available);
        self.targets[1].set_tooltip_text((!available).then_some(WINDOW_UNAVAILABLE));
    }

    /// Show the controls for `target` and the primary action's state.
    pub fn sync(&self, target: Target, can_start: bool, auto_start: bool, state: PrimaryState) {
        self.aspect_box.set_visible(target == Target::Region);
        self.display_box.set_visible(target == Target::Display);
        for (button, of) in
            self.targets
                .iter()
                .zip([Target::Region, Target::Window, Target::Display])
        {
            if button.is_active() != (of == target) {
                button.set_active(of == target);
            }
        }
        let action = capture_menu::primary_action(MenuMode::Screenshot, auto_start, state);
        self.primary_label.set_label(action.label);
        self.primary.update_property(&[
            gtk::accessible::Property::Label(action.accessibility_label),
            gtk::accessible::Property::KeyShortcuts("Enter"),
        ]);
        self.primary.set_visible(!action.hidden);
        let busy = state.starting || state.switching_display;
        self.aspect.set_sensitive(!busy);
        self.display.set_sensitive(!busy && self.several_displays);
        self.primary.set_sensitive(can_start && !state.starting);
        self.confirm.set_visible(!auto_start);
        self.auto_start.set_visible(auto_start);
        let note = capture_menu::visibility_note(MenuMode::Screenshot, false, false);
        let confirm = capture_menu::confirm_note(auto_start);
        self.note
            .update_property(&[gtk::accessible::Property::Label(&format!(
                "{} {} {}",
                note.text(),
                capture_menu::NOTE_SEPARATOR,
                confirm.text
            ))]);
    }
}
