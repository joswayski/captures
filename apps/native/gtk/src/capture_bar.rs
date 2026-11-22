//! Shipping New Capture's floating command bar (`.recording-selector-panel`
//! in `RecordingSelector`): Close, the Screenshot/Record and
//! Region/Window/Full screen segments, the Aspect or Display select, the
//! primary action and the note under them. Real GTK widgets, so AT-SPI sees
//! the same buttons, toggles and comboboxes as shipping's DOM; copy and
//! states come from the shared `capture_menu`.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use captures_app::capture_menu::{self, MenuMode, PreferenceTarget, PrimaryState, RecordingToggle};
use captures_app::motion::Transition;
use gtk::{glib, graphene, gsk, prelude::*};

use crate::editor::controls::Choice;
use crate::tokens::Tokens;
use crate::widgets::{Icon, SnapshotArea};

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
    pub modes: [gtk::ToggleButton; 2],
    pub recording: Rc<RecordingFields>,
    aspect: Rc<Choice>,
    aspect_box: gtk::Box,
    display: Rc<Choice>,
    display_box: gtk::Box,
    several_displays: bool,
    pub primary: gtk::Button,
    primary_label: gtk::Label,
    primary_icon: Icon,
    primary_dot: gtk::Box,
    visibility: gtk::Label,
    confirm: gtk::Box,
    /// Shipping's `CapturePreferenceLink` around the auto-capture note.
    pub preferences_link: gtk::Button,
    link_target: Cell<Option<PreferenceTarget>>,
    note: gtk::Box,
}

/// Shipping `.recording-selector-panel button { cursor: pointer }`: the bar
/// sits over the selector's crosshair, so its controls show a pointing hand
/// and its blank glass the default arrow.
fn point_at_buttons(widget: &gtk::Widget) {
    if widget.is::<gtk::Button>() {
        widget.set_cursor_from_name(Some("pointer"));
    }
    let mut child = widget.first_child();
    while let Some(current) = child {
        point_at_buttons(&current);
        child = current.next_sibling();
    }
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

fn group(label: &str, buttons: &[&gtk::ToggleButton], tokens: &Rc<Tokens>) -> gtk::Overlay {
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
    // The thumb is drawn under the buttons, which still size the control.
    let indicator = segmented_indicator(buttons, tokens);
    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&indicator));
    overlay.add_overlay(&group);
    overlay.set_measure_overlay(&group, true);
    overlay
}

/// Shipping `.capture-segmented-indicator` on media: a raised glass thumb
/// behind the pressed segment. It lands under the first segment without
/// motion (shipping adds `.ready` after measuring), then slides to each
/// newly pressed one over `SegmentedIndicator`, at once with animations off.
fn segmented_indicator(buttons: &[&gtk::ToggleButton], tokens: &Rc<Tokens>) -> SnapshotArea {
    #[derive(Default)]
    struct Slide {
        /// The rectangle last drawn, and where the current slide began.
        shown: Cell<Option<[f32; 4]>>,
        from: Cell<Option<[f32; 4]>>,
        since_us: Cell<i64>,
    }
    let slide = Rc::new(Slide::default());
    let tween = Transition::SegmentedIndicator.resolve(&**tokens);
    let buttons: Vec<gtk::ToggleButton> = buttons.iter().map(|button| (*button).clone()).collect();
    let area = SnapshotArea::new(|_, _, _| {});
    let weak = area.downgrade();
    let (draw_slide, draw_buttons, tokens) = (slide.clone(), buttons.clone(), tokens.clone());
    area.set_draw(move |snapshot, _, _| {
        let Some(area) = weak.upgrade() else { return };
        let Some(bounds) = draw_buttons
            .iter()
            .find(|button| button.is_active())
            .and_then(|button| button.compute_bounds(&area))
        else {
            draw_slide.shown.set(None);
            return;
        };
        let target = [bounds.x(), bounds.y(), bounds.width(), bounds.height()];
        let reduced = !area.settings().is_gtk_enable_animations();
        let rect = match (draw_slide.from.get(), tween) {
            (Some(from), Some(tween)) => {
                let elapsed = (glib::monotonic_time() - draw_slide.since_us.get()) as f64 / 1000.;
                let t = tween.progress(elapsed, reduced) as f32;
                std::array::from_fn(|i| from[i] + (target[i] - from[i]) * t)
            }
            _ => target,
        };
        draw_slide.shown.set(Some(rect));
        let [x, y, width, height] = rect;
        let bounds = graphene::Rect::new(x, y, width, height);
        let thumb = gsk::RoundedRect::from_rect(bounds, tokens.number("r-sm"));
        for (dx, dy, blur, spread, color) in tokens.shadow("shadow-sm") {
            snapshot.append_outset_shadow(&thumb, &color, dx, dy, spread, blur);
        }
        snapshot.push_rounded_clip(&thumb);
        snapshot.append_color(&tokens.rgba("glass-raised"), &bounds);
        snapshot.pop();
        // `inset 0 0 0 1px var(--glass-border)`.
        snapshot.append_border(&thumb, &[1.; 4], &[tokens.rgba("glass-border"); 4]);
    });
    for button in &buttons {
        let (slide, area) = (slide.clone(), area.clone());
        button.connect_toggled(move |button| {
            if !button.is_active() {
                return;
            }
            slide.from.set(slide.shown.get());
            slide.since_us.set(glib::monotonic_time());
            area.queue_draw();
            let Some(tween) = tween else { return };
            let (slide, end) = (slide.clone(), tween.duration_ms);
            area.add_tick_callback(move |area, _| {
                area.queue_draw();
                let elapsed = (glib::monotonic_time() - slide.since_us.get()) as f64 / 1000.;
                if elapsed < end {
                    glib::ControlFlow::Continue
                } else {
                    glib::ControlFlow::Break
                }
            });
        });
    }
    area
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
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        displays: Vec<(String, String)>,
        display: &str,
        auto_start: bool,
        recording_settings: captures_settings::RecordingSettings,
        tokens: Rc<crate::tokens::Tokens>,
        pick_aspect: impl Fn(String) + 'static,
        pick_display: impl Fn(String) + 'static,
        options_changed: impl Fn() + 'static,
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
        top.append(&group("Capture type", &[&screenshot, &record], &tokens));
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
            &tokens,
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
        // Shipping's select keeps its content width inside the 150 px picker.
        aspect.root.set_halign(gtk::Align::Start);
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
        let primary_dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        primary_dot.add_css_class("capture-record-dot");
        primary_dot.set_valign(gtk::Align::Center);
        // Shipping's `::after` ping over the dot while Record can start.
        let ping = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        ping.add_css_class("capture-record-ping");
        ping.set_hexpand(true);
        primary_dot.append(&ping);
        content.append(&primary_dot);
        let primary_label = gtk::Label::new(None);
        content.append(&primary_label);
        primary.set_child(Some(&content));
        primary.set_hexpand(true);
        primary.set_halign(gtk::Align::End);
        top.append(&primary);
        let recording = RecordingFields::new(recording_settings, tokens, options_changed);
        root.append(&recording.root);

        // "These controls will show in screenshots · Press Enter to confirm".
        // Read as one sentence, as shipping's text node.
        let note = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .accessible_role(gtk::AccessibleRole::Note)
            .build();
        note.add_css_class("capture-bar-note");
        note.set_halign(gtk::Align::Center);
        let visibility = gtk::Label::new(None);
        note.append(&visibility);
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
        let preferences_link = gtk::Button::new();
        preferences_link.add_css_class("capture-bar-preferences-link");
        let link = gtk::Box::new(gtk::Orientation::Horizontal, 5);
        link.append(&text(capture_menu::AUTO_START_NOTE, None));
        let icon = Icon::new("external-link", 10).with_stroke(1.6);
        icon.add_css_class("capture-bar-preferences-icon");
        icon.set_valign(gtk::Align::Center);
        link.append(&icon);
        preferences_link.set_child(Some(&link));
        preferences_link.update_property(&[gtk::accessible::Property::Label(
            capture_menu::AUTO_START_NOTE,
        )]);
        note.append(&preferences_link);
        root.append(&note);
        point_at_buttons(root.upcast_ref());

        let bar = Self {
            root,
            close,
            targets,
            modes: [screenshot, record],
            recording,
            aspect,
            aspect_box,
            display: picker,
            display_box,
            several_displays: several,
            primary,
            primary_label,
            primary_icon: glyph,
            primary_dot,
            visibility,
            confirm,
            preferences_link,
            link_target: Cell::new(None),
            note,
        };
        bar.sync(
            Target::Region,
            MenuMode::Screenshot,
            false,
            auto_start,
            PrimaryState::default(),
        );
        bar
    }

    /// The Preferences row the note's link opens, while it shows one.
    pub fn link_target(&self) -> Option<PreferenceTarget> {
        self.link_target.get()
    }

    pub fn set_recording_available(&self, available: bool) {
        self.modes[1].set_sensitive(available);
        self.modes[1].set_tooltip_text((!available).then_some(RECORDING_UNAVAILABLE));
    }

    /// Shipping disables Window when the session lists no windows.
    pub fn set_window_available(&self, available: bool) {
        self.targets[1].set_sensitive(available);
        self.targets[1].set_tooltip_text((!available).then_some(WINDOW_UNAVAILABLE));
    }

    /// Show the controls for `target` and the primary action's state.
    pub fn sync(
        &self,
        target: Target,
        mode: MenuMode,
        can_start: bool,
        auto_start: bool,
        state: PrimaryState,
    ) {
        self.modes[if mode == MenuMode::Recording { 1 } else { 0 }].set_active(true);
        // `recording-options-arrive` plays each time the row appears.
        let options = mode == MenuMode::Recording;
        if options != self.recording.root.is_visible() {
            if options {
                self.recording.root.add_css_class("arrive");
            } else {
                self.recording.root.remove_css_class("arrive");
            }
        }
        self.recording.root.set_visible(options);
        if mode == MenuMode::Recording {
            self.recording.load_microphones();
        }
        self.primary_icon.set_visible(mode == MenuMode::Screenshot);
        self.primary_dot.set_visible(mode == MenuMode::Recording);
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
        let action = capture_menu::primary_action(mode, auto_start, state);
        self.primary_label.set_label(action.label);
        self.primary.update_property(&[
            gtk::accessible::Property::Label(action.accessibility_label),
            gtk::accessible::Property::KeyShortcuts("Enter"),
        ]);
        self.primary.set_visible(!action.hidden);
        let busy = state.starting || state.switching_display;
        self.recording.root.set_sensitive(!busy);
        self.aspect.set_sensitive(!busy);
        self.display.set_sensitive(!busy && self.several_displays);
        self.primary.set_sensitive(can_start && !state.starting);
        self.confirm.set_visible(!auto_start);
        self.preferences_link.set_visible(auto_start);
        let note = capture_menu::visibility_note(mode, false, false);
        self.visibility.set_markup(&format!(
            "{}<b>{}</b>{}{}",
            note.lead,
            note.emphasis,
            note.trail,
            note.hint.unwrap_or_default()
        ));
        let confirm = capture_menu::confirm_note(auto_start);
        self.link_target.set(confirm.target);
        self.note
            .update_property(&[gtk::accessible::Property::Label(&format!(
                "{} {} {}",
                note.text(),
                capture_menu::NOTE_SEPARATOR,
                confirm.text
            ))]);
    }
}

/// Per-capture options, deliberately separate from persisted Preferences.
pub struct RecordingFields {
    root: gtk::Box,
    pub settings: RefCell<captures_settings::RecordingSettings>,
    toggles: Vec<(RecordingToggle, crate::switch::TokenSwitch, gtk::Label)>,
    _choices: [Rc<Choice>; 2],
    microphone: Rc<Choice>,
    microphones_loaded: Cell<bool>,
}

impl RecordingFields {
    fn new(
        settings: captures_settings::RecordingSettings,
        tokens: Rc<crate::tokens::Tokens>,
        changed: impl Fn() + 'static,
    ) -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        root.add_css_class("capture-recording-options");
        let capabilities = captures_recording_platform::RecordingCapabilities::current(false);
        let mut settings = settings;
        settings.show_cursor &= capabilities.cursor_control;
        settings.highlight_clicks &= capabilities.click_highlights && settings.show_cursor;
        settings.capture_system_audio &= capabilities.system_audio;
        if !capabilities.microphone {
            settings.microphone_device_id = None;
        }
        let link: Rc<RefCell<std::rc::Weak<Self>>> = Rc::default();
        let fps_link = link.clone();
        let fps = Choice::new(
            None,
            capture_menu::FPS_ACCESSIBILITY_LABEL,
            false,
            move |value| {
                if let Some(fields) = fps_link.borrow().upgrade() {
                    fields.settings.borrow_mut().video_fps = value.parse().expect("FPS option");
                    changed();
                }
            },
        );
        fps.set_glass();
        fps.set_options(
            capture_menu::FPS_OPTIONS
                .iter()
                .map(|fps| (fps.to_string(), fps.to_string()))
                .collect(),
        );
        fps.set(&settings.video_fps.to_string());
        root.append(&Self::field(capture_menu::FIELD_FPS, &fps.root, 76));
        let resolution_link = link.clone();
        let resolution = Choice::new(
            None,
            capture_menu::MAX_RESOLUTION_ACCESSIBILITY_LABEL,
            false,
            move |value| {
                if let Some(fields) = resolution_link.borrow().upgrade() {
                    fields.settings.borrow_mut().video_max_resolution = match value.as_str() {
                        "p1080" => captures_recording::MaxResolution::P1080,
                        "p720" => captures_recording::MaxResolution::P720,
                        _ => captures_recording::MaxResolution::Original,
                    };
                }
            },
        );
        resolution.set_glass();
        resolution.set_options(
            capture_menu::RESOLUTION_OPTIONS
                .iter()
                .map(|(id, label)| ((*id).into(), (*label).into()))
                .collect(),
        );
        resolution.set(match settings.video_max_resolution {
            captures_recording::MaxResolution::Original => "original",
            captures_recording::MaxResolution::P1080 => "p1080",
            captures_recording::MaxResolution::P720 => "p720",
        });
        root.append(&Self::field(
            capture_menu::FIELD_MAX_RESOLUTION,
            &resolution.root,
            132,
        ));
        let toggles = RecordingToggle::ALL
            .into_iter()
            .map(|toggle| {
                let available = match toggle {
                    RecordingToggle::ShowCursor => capabilities.cursor_control,
                    RecordingToggle::ShowClicks => capabilities.click_highlights,
                    RecordingToggle::DesktopAudio => capabilities.system_audio,
                };
                let switch = crate::switch::TokenSwitch::new(tokens.clone(), false);
                switch.set_glass();
                switch.set_sensitive(available);
                switch.update_property(&[gtk::accessible::Property::Label(
                    toggle.accessibility_label(),
                )]);
                let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                row.add_css_class("capture-recording-toggle");
                row.append(&switch);
                let status = gtk::Label::new(None);
                row.append(&status);
                row.set_tooltip_text((!available).then_some(toggle.unavailable_reason()));
                let weak = link.clone();
                // The label and track share one pointer target, like shipping.
                let click = gtk::GestureClick::new();
                let sw = switch.downgrade();
                click.connect_released(move |_, _, _, _| {
                    if let Some(sw) = sw.upgrade() {
                        sw.grab_focus();
                        sw.activate_toggle();
                    }
                });
                row.add_controller(click);
                switch.connect_toggled(move |on| {
                    if let Some(fields) = weak.borrow().upgrade() {
                        {
                            let mut settings = fields.settings.borrow_mut();
                            match toggle {
                                RecordingToggle::ShowCursor => settings.show_cursor = on,
                                RecordingToggle::ShowClicks => settings.highlight_clicks = on,
                                RecordingToggle::DesktopAudio => settings.capture_system_audio = on,
                            }
                            (settings.show_cursor, settings.highlight_clicks) =
                                capture_menu::couple_pointer_options(
                                    toggle,
                                    settings.show_cursor,
                                    settings.highlight_clicks,
                                );
                        }
                        fields.sync();
                    }
                });
                root.append(&Self::field(
                    toggle.label(),
                    &row,
                    if toggle == RecordingToggle::DesktopAudio {
                        118
                    } else {
                        92
                    },
                ));
                (toggle, switch, status)
            })
            .collect();
        let mic_link = link.clone();
        let microphone = Choice::new(None, capture_menu::FIELD_MICROPHONE, false, move |value| {
            if let Some(fields) = mic_link.borrow().upgrade() {
                fields.settings.borrow_mut().microphone_device_id =
                    (!value.is_empty()).then_some(value);
            }
        });
        microphone.set_glass();
        microphone.set_options(vec![(
            String::new(),
            capture_menu::MICROPHONES_LOADING.into(),
        )]);
        microphone.set("");
        microphone.set_sensitive(false);
        // Shipping's `minmax(180px, 240px)` column: the bar always has room.
        let microphone_field = Self::field(capture_menu::FIELD_MICROPHONE, &microphone.root, 240);
        root.append(&microphone_field);
        let fields = Rc::new(Self {
            root,
            settings: RefCell::new(settings),
            toggles,
            _choices: [fps, resolution],
            microphone,
            microphones_loaded: Cell::new(false),
        });
        *link.borrow_mut() = Rc::downgrade(&fields);
        fields.sync();
        fields
    }

    fn load_microphones(self: &Rc<Self>) {
        if self.microphones_loaded.replace(true) {
            return;
        }
        let available =
            captures_recording_platform::RecordingCapabilities::current(false).microphone;
        let weak = Rc::downgrade(self);
        gtk::glib::spawn_future_local(async move {
            let devices = gtk::gio::spawn_blocking(captures_recording_platform::microphone_devices)
                .await
                .unwrap_or_default();
            let Some(fields) = weak.upgrade() else { return };
            let microphone = &fields.microphone;
            let settings = fields.settings.borrow();
            let devices: Vec<_> = devices
                .iter()
                .map(|device| (device.id.as_str(), device.name.as_str()))
                .collect();
            microphone.set_options(
                capture_menu::microphone_entries(
                    available,
                    false,
                    settings.microphone_device_id.as_deref(),
                    &devices,
                )
                .into_iter()
                .map(|entry| (entry.id.unwrap_or_default(), entry.label))
                .collect(),
            );
            microphone.set(settings.microphone_device_id.as_deref().unwrap_or_default());
            microphone.set_sensitive(available);
        });
    }

    fn field(label: &str, control: &impl IsA<gtk::Widget>, width: i32) -> gtk::Box {
        let field = gtk::Box::new(gtk::Orientation::Vertical, 6);
        field.set_hexpand(false);
        field.set_size_request(width, -1);
        let caption = gtk::Label::new(Some(&label.to_uppercase()));
        caption.add_css_class("capture-recording-caption");
        caption.set_xalign(0.);
        field.append(&caption);
        field.append(control);
        field
    }

    fn sync(&self) {
        let settings = self.settings.borrow();
        for (toggle, switch, status) in &self.toggles {
            let on = match toggle {
                RecordingToggle::ShowCursor => settings.show_cursor,
                RecordingToggle::ShowClicks => settings.highlight_clicks,
                RecordingToggle::DesktopAudio => settings.capture_system_audio,
            };
            if on != switch.is_on() {
                switch.activate_toggle_quietly(on);
            }
            status.set_label(capture_menu::toggle_status(switch.is_sensitive(), on));
        }
    }
}
