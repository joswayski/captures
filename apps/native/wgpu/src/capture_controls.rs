use captures_app::capture_menu::{
    self, Guidance, MenuMode, PreferenceTarget, PrimaryState, RecordingToggle,
};
use captures_app::motion::{Motion, Transition};
use captures_app::selection::{Bounds, Rect};
use captures_app::shortcuts::CaptureShortcut;
use captures_capture::{DisplayDescriptor, WindowDescriptor};
use captures_recording::{AudioDevice, MaxResolution};
use captures_recording_platform::RecordingCapabilities;
use captures_settings::RecordingSettings;
use eframe::egui::{self, Align2, Color32, RichText, Stroke, TextureHandle};

use crate::{
    motion::SlidingIndicator,
    selector::{self, Selector},
    tokens::Tokens,
    window_selector::{self, SelectionTarget, WindowSelector},
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TargetMode {
    #[default]
    Region,
    Window,
    Display,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ActionMode {
    #[default]
    Screenshot,
    Recording,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Target {
    Region(Rect),
    Window(usize),
    Display,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Capture(Target),
    StartRecording(Target),
    SwitchDisplay(String),
    /// Close the menu and open Preferences at this row, highlighted.
    OpenPreference(PreferenceTarget),
    Cancel,
}

pub struct View<'a> {
    pub panel_id: egui::Id,
    pub frozen: Option<&'a TextureHandle>,
    pub display: &'a DisplayDescriptor,
    pub displays: &'a [DisplayDescriptor],
    pub windows: &'a [WindowDescriptor],
    pub auto_start: bool,
    pub recording_available: bool,
    pub recording_unavailable_reason: Option<&'a str>,
    /// A start or display switch failed while the menu stayed open.
    pub error: Option<&'a str>,
}

pub struct CaptureControls {
    action_mode: ActionMode,
    mode: TargetMode,
    region: Selector,
    window: WindowSelector,
    recording: RecordingSelection,
    recording_capabilities: RecordingCapabilities,
    microphones: Vec<AudioDevice>,
    /// Devices enumerate on the recording worker after the menu opens.
    microphones_loading: bool,
    /// egui time the Record options row appeared, for its entrance.
    recording_options_since: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RecordingSelection {
    pub frames_per_second: u16,
    pub max_resolution: MaxResolution,
    pub countdown_seconds: u8,
    pub show_cursor: bool,
    pub highlight_clicks: bool,
    pub capture_system_audio: bool,
    pub microphone_device_id: Option<String>,
    pub mono_audio: bool,
}

impl Default for CaptureControls {
    fn default() -> Self {
        Self {
            action_mode: ActionMode::Screenshot,
            mode: TargetMode::Region,
            region: Selector::default(),
            window: WindowSelector::default(),
            recording: RecordingSelection {
                frames_per_second: 60,
                max_resolution: MaxResolution::Original,
                countdown_seconds: 3,
                show_cursor: true,
                highlight_clicks: false,
                capture_system_audio: false,
                microphone_device_id: None,
                mono_audio: false,
            },
            recording_capabilities: RecordingCapabilities::current(false),
            microphones: vec![],
            microphones_loading: false,
            recording_options_since: None,
        }
    }
}

impl CaptureControls {
    pub fn fixture() -> Self {
        Self {
            window: WindowSelector::fixture(),
            ..Self::default()
        }
    }

    pub fn recording_fixture() -> Self {
        Self {
            action_mode: ActionMode::Recording,
            window: WindowSelector::fixture(),
            ..Self::default()
        }
    }

    pub fn mode(&self) -> TargetMode {
        self.mode
    }

    pub fn configure_recording(
        &mut self,
        settings: &RecordingSettings,
        capabilities: RecordingCapabilities,
    ) {
        self.recording = RecordingSelection {
            frames_per_second: settings.video_fps,
            max_resolution: settings.video_max_resolution,
            countdown_seconds: settings.countdown_seconds,
            show_cursor: capabilities.cursor_control && settings.show_cursor,
            highlight_clicks: capabilities.click_highlights && settings.highlight_clicks,
            capture_system_audio: capabilities.system_audio && settings.capture_system_audio,
            microphone_device_id: capabilities
                .microphone
                .then(|| settings.microphone_device_id.clone())
                .flatten(),
            mono_audio: settings.mono_audio,
        };
        self.microphones_loading = capabilities.microphone;
        self.recording_capabilities = capabilities;
    }

    pub fn set_microphones(&mut self, microphones: Vec<AudioDevice>) {
        self.microphones = microphones;
        self.microphones_loading = false;
    }

    pub fn recording_selection(&self) -> RecordingSelection {
        self.recording.clone()
    }

    #[cfg(test)]
    fn region(&self) -> Option<Rect> {
        self.region.rect()
    }

    #[cfg(test)]
    fn window(&self) -> Option<SelectionTarget> {
        self.window.selected()
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn reset_for_display_change(&mut self) {
        self.region.clear_selection();
        self.window.reset();
    }

    pub fn select_recording_target(&mut self, target: TargetMode) {
        self.action_mode = ActionMode::Recording;
        self.mode = target;
    }

    pub fn apply_target_shortcut(&mut self, shortcut: CaptureShortcut) {
        self.mode = match shortcut {
            CaptureShortcut::Region | CaptureShortcut::RecordRegion => {
                self.window.clear_selection_and_hover();
                TargetMode::Region
            }
            CaptureShortcut::Window | CaptureShortcut::RecordWindow => {
                self.window.clear_hover();
                TargetMode::Window
            }
            CaptureShortcut::Display | CaptureShortcut::RecordDisplay => {
                self.window.clear_selection_and_hover();
                TargetMode::Display
            }
            CaptureShortcut::NewCapture => return,
        };
        self.action_mode = if shortcut.is_recording() {
            ActionMode::Recording
        } else {
            ActionMode::Screenshot
        };
    }

    pub fn exercise(
        &mut self,
        cycle: usize,
        bounds: Bounds,
        hit_test: impl Fn(captures_app::selection::Point) -> Option<usize>,
    ) {
        match cycle {
            0 => {
                self.mode = TargetMode::Region;
                self.region.exercise(0, bounds);
            }
            1 => {
                self.mode = TargetMode::Window;
                self.window.exercise(0, hit_test);
            }
            2 => self.mode = TargetMode::Display,
            3 => self.mode = TargetMode::Region,
            4 => self.mode = TargetMode::Window,
            5 => self.reset(),
            _ => {}
        }
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        tokens: &Tokens,
        view: View<'_>,
        hit_test: impl Fn(captures_app::selection::Point) -> Option<usize>,
    ) -> Option<Action> {
        let (overlay_width, overlay_height) = view.display.overlay_size();
        let bounds = Bounds {
            width: overlay_width,
            height: overlay_height,
        };
        let menu_mode = self.menu_mode();
        let mut action = match self.mode {
            TargetMode::Region => self
                .region
                .show_menu_surface(ui, tokens, view.frozen, view.auto_start, Some(bounds))
                .and_then(|action| match action {
                    selector::Action::Confirm => self.current_action(),
                    selector::Action::Cancel => Some(Action::Cancel),
                }),
            TargetMode::Window => {
                let target = self.window.show_menu_surface(
                    ui,
                    tokens,
                    &window_selector::View {
                        frozen: view.frozen,
                        display: view.display,
                        windows: view.windows,
                        auto_start: view.auto_start,
                    },
                    hit_test,
                );
                if target == Some(SelectionTarget::Display)
                    || self.window.selected() == Some(SelectionTarget::Display)
                {
                    self.mode = TargetMode::Display;
                }
                target.map(|target| self.action_for_target(window_target(target)))
            }
            TargetMode::Display => {
                let clicked = window_selector::show_display_surface(
                    ui,
                    tokens,
                    &window_selector::View {
                        frozen: view.frozen,
                        display: view.display,
                        windows: view.windows,
                        auto_start: view.auto_start,
                    },
                    &capture_menu::display_identity(
                        &view.display.name,
                        view.display.width,
                        view.display.height,
                        (self.action_mode == ActionMode::Recording)
                            .then_some(self.recording.frames_per_second),
                    ),
                );
                (clicked && view.auto_start).then(|| self.action_for_target(Target::Display))
            }
        };

        if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
            action = Some(Action::Cancel);
        } else if ui.input(|input| input.key_pressed(egui::Key::Enter))
            && let Some(target) = self.current_target()
        {
            action = Some(self.action_for_target(target));
        }

        let primary = capture_menu::primary_action(
            menu_mode,
            view.auto_start,
            PrimaryState {
                error: view.error.is_some(),
                ..PrimaryState::default()
            },
        );
        let content_rect = ui.ctx().content_rect();
        let panel_bounds = content_rect.shrink(16.);
        let panel = egui::Area::new(view.panel_id)
            .pivot(Align2::CENTER_BOTTOM)
            .default_pos(content_rect.center_bottom() - egui::vec2(0., 26.))
            .constrain_to(panel_bounds)
            .movable(true)
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                egui::Frame::new()
                    .fill(tokens.color("glass-strong"))
                    .stroke(Stroke::new(1., tokens.color("glass-border")))
                    .corner_radius(tokens.number("r-2xl") as u8)
                    .inner_margin(tokens.number("s-4") as i8)
                    .show(ui, |ui| {
                        // Match the shipping toolbar's stable outer width while
                        // retaining a sixteen-point margin on narrow displays.
                        let outer_width = (content_rect.width() - 32.).min(854.);
                        let frame_chrome = 2. * (tokens.number("s-4") + 1.);
                        ui.set_min_width(outer_width - frame_chrome);
                        tokens.glass_controls(ui);
                        if content_rect.width() <= 800. {
                            ui.spacing_mut().item_spacing.x = tokens.number("s-2");
                            ui.spacing_mut().button_padding.x = tokens.number("s-4");
                        }
                        ui.spacing_mut().item_spacing.y = tokens.number("s-3");
                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                if ui
                                    .button("×")
                                    .on_hover_text("Close capture controls (Esc)")
                                    .clicked()
                                {
                                    action = Some(Action::Cancel);
                                }
                                ui.separator();
                                // Shipping `.capture-action-switch` indicator.
                                let action_switch = SlidingIndicator::begin(
                                    ui,
                                    view.panel_id.with("action-indicator"),
                                    ui.min_rect().min,
                                );
                                let screenshot = segment(
                                    ui,
                                    tokens,
                                    self.action_mode == ActionMode::Screenshot,
                                    "Screenshot",
                                );
                                if screenshot.clicked() {
                                    self.action_mode = ActionMode::Screenshot;
                                }
                                let record = ui
                                    .add_enabled(
                                        view.recording_available,
                                        egui::Button::new(
                                            RichText::new("Record").color(tokens.color(
                                                if self.action_mode == ActionMode::Recording {
                                                    "glass-text"
                                                } else {
                                                    "glass-text-muted"
                                                },
                                            )),
                                        )
                                        .fill(Color32::TRANSPARENT),
                                    )
                                    .on_disabled_hover_text(view.recording_unavailable_reason.unwrap_or(
                                        "Screen recording is unavailable in this desktop session",
                                    ));
                                if record.clicked() {
                                    self.action_mode = ActionMode::Recording;
                                }
                                paint_indicator(
                                    ui,
                                    tokens,
                                    action_switch,
                                    if self.action_mode == ActionMode::Recording {
                                        record.rect
                                    } else {
                                        screenshot.rect
                                    },
                                );
                                ui.separator();
                                // Shipping `.recording-target-switch` indicator.
                                let target_switch = SlidingIndicator::begin(
                                    ui,
                                    view.panel_id.with("target-indicator"),
                                    ui.min_rect().min,
                                );
                                let mut selected_segment = None;
                                for (mode, label) in [
                                    (TargetMode::Region, "Region"),
                                    (TargetMode::Window, "Window"),
                                    (TargetMode::Display, "Full screen"),
                                ] {
                                    let response = segment(ui, tokens, self.mode == mode, label);
                                    if self.mode == mode {
                                        selected_segment = Some(response.rect);
                                    }
                                    if response.clicked() {
                                        let same = self.mode == mode;
                                        self.mode = mode;
                                        if mode == TargetMode::Display && view.auto_start {
                                            action =
                                                Some(self.action_for_target(Target::Display));
                                        } else if same
                                            && view.auto_start
                                            && let Some(target) = self.current_target()
                                        {
                                            action = Some(self.action_for_target(target));
                                        }
                                    }
                                }
                                if let Some(rect) = selected_segment {
                                    paint_indicator(ui, tokens, target_switch, rect);
                                }
                                if self.mode == TargetMode::Region {
                                    ui.separator();
                                    self.region.show_aspect_picker(ui, tokens, bounds);
                                } else if self.mode == TargetMode::Display {
                                    ui.separator();
                                    let mut display_id = view.display.id.clone();
                                    let mut selected = display_label(view.display).to_owned();
                                    if content_rect.width() <= 800. {
                                        selected = truncate_label(&selected, 14);
                                    }
                                    egui::ComboBox::from_id_salt("capture-controls-display")
                                        .selected_text(selected)
                                        .show_ui(ui, |ui| {
                                            for display in view.displays {
                                                ui.selectable_value(
                                                    &mut display_id,
                                                    display.id.clone(),
                                                    format!(
                                                        "{} — {}×{}{}",
                                                        display_label(display),
                                                        display.width,
                                                        display.height,
                                                        if display.is_primary {
                                                            " (Primary)"
                                                        } else {
                                                            ""
                                                        }
                                                    ),
                                                );
                                            }
                                        });
                                    if display_id != view.display.id {
                                        action = Some(Action::SwitchDisplay(display_id));
                                    }
                                }
                                if !primary.hidden {
                                    ui.separator();
                                    let target = self.current_target();
                                    if ui
                                        .add_enabled(
                                            target.is_some(),
                                            egui::Button::new(
                                                RichText::new(primary.label)
                                                    .color(tokens.color("theme-accent-ink")),
                                            )
                                            .fill(tokens.color("theme-accent"))
                                            .stroke(Stroke::NONE),
                                        )
                                        .on_hover_text(format!(
                                            "{} (Enter)",
                                            primary.accessibility_label
                                        ))
                                        .clicked()
                                        && let Some(target) = target
                                    {
                                        action = Some(self.action_for_target(target));
                                    }
                                }
                            });
                            if self.action_mode == ActionMode::Recording {
                                ui.separator();
                                // Shipping `recording-options-arrive` on the Record row.
                                let now = ui.input(|input| input.time);
                                let since = *self.recording_options_since.get_or_insert(now);
                                let arrive = tokens.motion(Motion::CaptureMenuOptionsArrive);
                                let reduced = crate::motion::reduced(ui.ctx());
                                let elapsed = (now - since) * 1000.;
                                if arrive.running(elapsed, reduced) {
                                    ui.ctx().request_repaint();
                                }
                                let rect = ui.available_rect_before_wrap();
                                crate::motion::with_pose(
                                    ui,
                                    arrive.pose_at(elapsed, reduced),
                                    rect,
                                    |ui| self.show_recording_options(ui, tokens),
                                );
                            } else {
                                self.recording_options_since = None;
                            }
                            if let Some(target) = self.show_note(ui, tokens, menu_mode, view.auto_start)
                            {
                                action = Some(Action::OpenPreference(target));
                            }
                            if let Some(reason) = view.recording_unavailable_reason {
                                ui.label(
                                    RichText::new(reason)
                                        .small()
                                        .color(tokens.color("theme-signal")),
                                );
                            }
                            if let Some(error) = view.error {
                                ui.label(
                                    RichText::new(error)
                                        .small()
                                        .color(tokens.color("danger-text")),
                                );
                            }
                        });
                    });
            });
        if panel.response.contains_pointer() && ui.input(|input| input.pointer.any_pressed()) {
            // A press on foreground controls must not begin a region drag below them.
            if self.mode == TargetMode::Region {
                self.region.cancel_drag();
            }
        }
        action
    }

    fn menu_mode(&self) -> MenuMode {
        match self.action_mode {
            ActionMode::Screenshot => MenuMode::Screenshot,
            ActionMode::Recording => MenuMode::Recording,
        }
    }

    /// Shipping `capture-selector-note`: the capability-driven visibility note
    /// (linked to its Preferences row when this platform can exclude the
    /// controls), then the Enter hint or the linked auto-capture notice.
    fn show_note(
        &self,
        ui: &mut egui::Ui,
        tokens: &Tokens,
        mode: MenuMode,
        auto_start: bool,
    ) -> Option<PreferenceTarget> {
        let note = capture_menu::visibility_note(
            mode,
            self.recording_capabilities.can_exclude_controls,
            self.recording_capabilities.controls_excluded,
        );
        let confirm = capture_menu::confirm_note(auto_start);
        let hint = note.hint.unwrap_or_default();
        let pieces = [
            NotePiece {
                parts: vec![
                    (note.lead, false),
                    (note.emphasis, true),
                    (note.trail.as_str(), false),
                    (hint, false),
                ],
                target: note.target,
            },
            NotePiece {
                parts: vec![(capture_menu::NOTE_SEPARATOR, false)],
                target: None,
            },
            NotePiece {
                parts: vec![(confirm.text, false)],
                target: confirm.target,
            },
        ];
        let gap = tokens.number("s-2");
        let width = pieces
            .iter()
            .map(|piece| piece.size(ui, tokens).x)
            .sum::<f32>()
            + gap * (pieces.len() - 1) as f32;
        let mut opened = None;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            ui.add_space(((ui.available_width() - width) / 2.).max(0.));
            for piece in &pieces {
                if piece.show(ui, tokens).clicked() {
                    opened = piece.target;
                }
            }
        });
        opened
    }

    fn current_target(&self) -> Option<Target> {
        match self.mode {
            TargetMode::Region => self
                .region
                .can_confirm()
                .then(|| self.region.rect())
                .flatten()
                .map(Target::Region),
            TargetMode::Window => self.window.selected().map(window_target),
            TargetMode::Display => Some(Target::Display),
        }
    }

    fn current_action(&self) -> Option<Action> {
        self.current_target()
            .map(|target| self.action_for_target(target))
    }

    fn action_for_target(&self, target: Target) -> Action {
        match self.action_mode {
            ActionMode::Screenshot => Action::Capture(target),
            ActionMode::Recording => Action::StartRecording(target),
        }
    }

    /// Shipping `recording-options-row`: labelled FPS / Max resolution selects,
    /// Show cursor / Show clicks / Desktop audio switches and the microphone.
    fn show_recording_options(&mut self, ui: &mut egui::Ui, tokens: &Tokens) {
        let capabilities = self.recording_capabilities.clone();
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = tokens.number("s-4");
            field(ui, tokens, capture_menu::FIELD_FPS, 76., |ui| {
                egui::ComboBox::from_id_salt("recording-fps")
                    .width(ui.available_width())
                    .selected_text(self.recording.frames_per_second.to_string())
                    .show_ui(ui, |ui| {
                        for fps in capture_menu::FPS_OPTIONS {
                            ui.selectable_value(
                                &mut self.recording.frames_per_second,
                                fps,
                                fps.to_string(),
                            );
                        }
                    })
                    .response
                    .on_hover_text(capture_menu::FPS_ACCESSIBILITY_LABEL);
            });
            field(ui, tokens, capture_menu::FIELD_MAX_RESOLUTION, 132., |ui| {
                egui::ComboBox::from_id_salt("recording-resolution")
                    .width(ui.available_width())
                    .selected_text(resolution_label(self.recording.max_resolution))
                    .show_ui(ui, |ui| {
                        for resolution in [
                            MaxResolution::Original,
                            MaxResolution::P1080,
                            MaxResolution::P720,
                        ] {
                            ui.selectable_value(
                                &mut self.recording.max_resolution,
                                resolution,
                                resolution_label(resolution),
                            );
                        }
                    })
                    .response
                    .on_hover_text(capture_menu::MAX_RESOLUTION_ACCESSIBILITY_LABEL);
            });
            for (toggle, width, available) in [
                (
                    RecordingToggle::ShowCursor,
                    92.,
                    capabilities.cursor_control,
                ),
                (
                    RecordingToggle::ShowClicks,
                    92.,
                    capabilities.click_highlights,
                ),
                (
                    RecordingToggle::DesktopAudio,
                    118.,
                    capabilities.system_audio,
                ),
            ] {
                field(ui, tokens, toggle.label(), width, |ui| {
                    let value = match toggle {
                        RecordingToggle::ShowCursor => &mut self.recording.show_cursor,
                        RecordingToggle::ShowClicks => &mut self.recording.highlight_clicks,
                        RecordingToggle::DesktopAudio => &mut self.recording.capture_system_audio,
                    };
                    if recording_switch(ui, tokens, toggle, value, available) {
                        (self.recording.show_cursor, self.recording.highlight_clicks) =
                            capture_menu::couple_pointer_options(
                                toggle,
                                self.recording.show_cursor,
                                self.recording.highlight_clicks,
                            );
                    }
                });
            }
            let width = ui.available_width().clamp(120., 240.);
            field(ui, tokens, capture_menu::FIELD_MICROPHONE, width, |ui| {
                self.show_microphone_select(ui, capabilities.microphone);
            });
        });
    }

    fn show_microphone_select(&mut self, ui: &mut egui::Ui, available: bool) {
        let devices = self
            .microphones
            .iter()
            .map(|device| (device.id.as_str(), device.name.as_str()))
            .collect::<Vec<_>>();
        let selected = self.recording.microphone_device_id.as_deref();
        let entries = capture_menu::microphone_entries(
            available,
            self.microphones_loading,
            selected,
            &devices,
        );
        let label = capture_menu::microphone_selected_label(
            available,
            self.microphones_loading,
            selected,
            &devices,
        );
        let mut choice = self.recording.microphone_device_id.clone();
        let width = ui.available_width();
        ui.add_enabled_ui(available && !self.microphones_loading, |ui| {
            egui::ComboBox::from_id_salt("recording-microphone")
                .width(width)
                .selected_text(truncate_label(&label, 22))
                .show_ui(ui, |ui| {
                    for entry in &entries {
                        ui.add_enabled_ui(entry.enabled, |ui| {
                            ui.selectable_value(&mut choice, entry.id.clone(), &entry.label);
                        });
                    }
                })
                .response
                .on_hover_text(capture_menu::FIELD_MICROPHONE);
        });
        self.recording.microphone_device_id = choice;
    }
}

/// One shipping `recording-field`: an uppercase caption over its control.
fn field(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    label: &str,
    width: f32,
    add: impl FnOnce(&mut egui::Ui),
) {
    ui.allocate_ui_with_layout(
        egui::vec2(width, 0.),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing.y = tokens.number("s-3");
            // Shipping `.recording-field` renders its caption uppercase.
            ui.label(
                RichText::new(label.to_uppercase())
                    .size(tokens.number("text-2xs"))
                    .color(tokens.color("glass-text-subtle")),
            );
            add(ui);
        },
    );
}

/// Shipping `recording-toggle`: a 30×18 switch and its On/Off/Unavailable text,
/// with the unavailable reason as a tooltip. Returns true when toggled.
fn recording_switch(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    toggle: RecordingToggle,
    value: &mut bool,
    available: bool,
) -> bool {
    let on = available && *value;
    let status = capture_menu::toggle_status(available, *value);
    let font = egui::FontId::proportional(tokens.number("text-xs"));
    let text_width = ui
        .painter()
        .layout_no_wrap(
            status.into(),
            font.clone(),
            tokens.color("glass-text-muted"),
        )
        .size()
        .x;
    let track = egui::vec2(30., 18.);
    let gap = tokens.number("s-3");
    let (rect, mut response) = ui.allocate_exact_size(
        egui::vec2(track.x + gap + text_width, ui.spacing().interact_size.y),
        if available {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        },
    );
    let changed = available && response.clicked();
    if changed {
        *value = !*value;
        response.mark_changed();
    }
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::Checkbox,
            available,
            available && *value,
            toggle.accessibility_label(),
        )
    });
    let response = if available {
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        response.on_hover_text(toggle.unavailable_reason())
    };
    let on = if changed { available && *value } else { on };
    let painter = ui.painter();
    let track_rect = egui::Rect::from_min_size(
        egui::pos2(rect.left(), rect.center().y - track.y / 2.),
        track,
    );
    painter.rect(
        track_rect,
        track.y / 2.,
        if on {
            tokens.color("theme-accent")
        } else {
            egui::Color32::from_black_alpha(89)
        },
        if on {
            Stroke::NONE
        } else {
            Stroke::new(1., tokens.color("glass-border-strong"))
        },
        egui::StrokeKind::Inside,
    );
    painter.circle_filled(
        egui::pos2(
            track_rect.left() + 8. + if on { 12. } else { 0. },
            track_rect.center().y,
        ),
        6.,
        tokens.color(if on {
            "theme-accent-ink"
        } else {
            "glass-text-subtle"
        }),
    );
    if response.has_focus() {
        painter.rect_stroke(
            track_rect.expand(2.),
            track.y / 2. + 2.,
            Stroke::new(1., tokens.color("theme-accent")),
            egui::StrokeKind::Outside,
        );
    }
    painter.text(
        egui::pos2(track_rect.right() + gap, rect.center().y),
        Align2::LEFT_CENTER,
        status,
        font,
        tokens.color(if available && response.hovered() {
            "glass-text"
        } else {
            "glass-text-muted"
        }),
    );
    changed
}

fn resolution_label(resolution: MaxResolution) -> &'static str {
    let key = match resolution {
        MaxResolution::Original => "original",
        MaxResolution::P1080 => "p1080",
        MaxResolution::P720 => "p720",
    };
    capture_menu::RESOLUTION_OPTIONS
        .iter()
        .find(|(value, _)| *value == key)
        .map_or("Original", |(_, label)| label)
}

/// A run of note text; a `target` makes it a Preferences link with the
/// shipping external-link glyph, hover wash and accent text.
struct NotePiece<'a> {
    parts: Vec<(&'a str, bool)>,
    target: Option<PreferenceTarget>,
}

impl NotePiece<'_> {
    const PADDING: egui::Vec2 = egui::vec2(5., 2.);
    const ICON: f32 = 10.;

    fn galley(
        &self,
        ui: &egui::Ui,
        tokens: &Tokens,
        hovered: bool,
    ) -> std::sync::Arc<egui::Galley> {
        let mut job = egui::text::LayoutJob::default();
        let font = egui::FontId::proportional(tokens.number("text-xs"));
        for (text, strong) in &self.parts {
            let color = if hovered {
                tokens.color("theme-accent-text-strong")
            } else if *strong {
                tokens.color("glass-text")
            } else {
                tokens.color("glass-text-subtle")
            };
            job.append(text, 0., egui::TextFormat::simple(font.clone(), color));
        }
        ui.painter().layout_job(job)
    }

    fn size(&self, ui: &egui::Ui, tokens: &Tokens) -> egui::Vec2 {
        let text = self.galley(ui, tokens, false).size();
        if self.target.is_some() {
            text + 2. * Self::PADDING + egui::vec2(Self::ICON + 5., 0.)
        } else {
            text
        }
    }

    fn show(&self, ui: &mut egui::Ui, tokens: &Tokens) -> egui::Response {
        let size = self.size(ui, tokens);
        let Some(_) = self.target else {
            let galley = self.galley(ui, tokens, false);
            let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
            ui.painter()
                .galley(rect.min, galley, tokens.color("glass-text-subtle"));
            return response;
        };
        let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
        let text = self.parts.iter().map(|(text, _)| *text).collect::<String>();
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Link, true, &text));
        let active = response.hovered() || response.has_focus();
        let painter = ui.painter();
        if active {
            painter.rect_filled(rect, tokens.number("r-sm"), tokens.color("glass-hover"));
        }
        if response.has_focus() {
            painter.rect_stroke(
                rect.expand(2.),
                tokens.number("r-sm"),
                Stroke::new(2., tokens.color("theme-accent")),
                egui::StrokeKind::Outside,
            );
        }
        let galley = self.galley(ui, tokens, active);
        let origin = rect.min + Self::PADDING;
        let icon = egui::Rect::from_min_size(
            egui::pos2(
                origin.x + galley.size().x + 5.,
                rect.center().y - Self::ICON / 2.,
            ),
            egui::vec2(Self::ICON, Self::ICON),
        );
        painter.galley(origin, galley, tokens.color("glass-text-subtle"));
        // Shipping `ExternalPreferenceIcon`: an open box with an outward arrow.
        let color = if active {
            tokens.color("theme-accent-text-strong")
        } else {
            tokens.color("glass-text-subtle").gamma_multiply(0.72)
        };
        let stroke = Stroke::new(1., color);
        let at = |x: f32, y: f32| icon.min + egui::vec2(x, y) * (Self::ICON / 16.);
        painter.line(
            vec![
                at(6.5, 3.),
                at(3., 3.),
                at(3., 13.),
                at(13., 13.),
                at(13., 9.5),
            ],
            stroke,
        );
        painter.line(vec![at(9., 3.), at(13., 3.), at(13., 7.)], stroke);
        painter.line_segment([at(8.5, 7.5), at(13., 3.)], stroke);
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    }
}

/// Shipping `CaptureGuidance` chip, 16% from the top of the overlay. It hides
/// while `hidden` (a region drag) and ducks out of the way when the pointer
/// comes within 28 points, restoring only past a 12-point leave slack.
pub(crate) fn paint_guidance(
    ui: &egui::Ui,
    tokens: &Tokens,
    surface: egui::Rect,
    guidance: Guidance,
    hidden: bool,
) {
    let painter = ui.painter();
    let title = painter.layout_no_wrap(
        guidance.title.into(),
        egui::FontId::proportional(tokens.number("text-md")),
        tokens.color("glass-text"),
    );
    let hint = painter.layout_no_wrap(
        guidance.hint.into(),
        egui::FontId::proportional(tokens.number("text-xs")),
        tokens.color("glass-text-muted"),
    );
    let padding = egui::vec2(tokens.number("s-6"), tokens.number("s-4"));
    let size = egui::vec2(
        title.size().x.max(hint.size().x),
        title.size().y + 2. + hint.size().y,
    ) + 2. * padding;
    let chip = egui::Rect::from_min_size(
        egui::pos2(
            surface.center().x - size.x / 2.,
            surface.top() + surface.height() * 0.16,
        ),
        size,
    );
    let id = egui::Id::unique("capture-guidance-pointer");
    let was_over = ui.data(|data| data.get_temp::<bool>(id)).unwrap_or(false);
    let over = ui
        .input(|input| input.pointer.latest_pos())
        .filter(|_| ui.input(|input| input.pointer.has_pointer()))
        .is_some_and(|pointer| {
            capture_menu::pointer_over_guidance(
                pointer.x.into(),
                pointer.y.into(),
                chip.left().into(),
                chip.top().into(),
                chip.right().into(),
                chip.bottom().into(),
                was_over,
            )
        });
    if over != was_over {
        ui.data_mut(|data| data.insert_temp(id, over));
    }
    let opacity = ui.ctx().animate_bool_with_time(
        egui::Id::unique("capture-guidance-opacity"),
        !hidden && !over,
        tokens.number("dur-3") / 1000.,
    );
    if opacity <= 0. {
        return;
    }
    let mut painter = painter.clone();
    painter.multiply_opacity(opacity);
    painter.rect(
        chip,
        tokens.number("r-xl"),
        tokens.color("glass-strong"),
        Stroke::new(1., tokens.color("glass-border-strong")),
        egui::StrokeKind::Inside,
    );
    painter.galley(
        egui::pos2(
            chip.center().x - title.size().x / 2.,
            chip.top() + padding.y,
        ),
        title,
        egui::Color32::WHITE,
    );
    painter.galley(
        egui::pos2(
            chip.center().x - hint.size().x / 2.,
            chip.bottom() - padding.y - hint.size().y,
        ),
        hint,
        egui::Color32::WHITE,
    );
}

fn window_target(target: SelectionTarget) -> Target {
    match target {
        SelectionTarget::Display => Target::Display,
        SelectionTarget::Window(index) => Target::Window(index),
    }
}

/// One segment; its raised fill is the switch's [`SlidingIndicator`].
fn segment(ui: &mut egui::Ui, tokens: &Tokens, selected: bool, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(label).color(tokens.color(if selected {
            "glass-text"
        } else {
            "glass-text-muted"
        })))
        .fill(Color32::TRANSPARENT)
        .stroke(Stroke::NONE),
    )
}

/// The selected segment's `glass-raised` pill, sliding over `--dur-4`.
fn paint_indicator(
    ui: &egui::Ui,
    tokens: &Tokens,
    indicator: SlidingIndicator,
    selected: egui::Rect,
) {
    let radius = ui.visuals().widgets.inactive.corner_radius;
    let fill = tokens.color("glass-raised");
    let stroke = Stroke::new(1., tokens.color("glass-border"));
    let tween = tokens.transition(Transition::SegmentedIndicator);
    indicator.finish(ui, selected, &tween, |rect| {
        egui::Shape::Rect(egui::epaint::RectShape::new(
            rect,
            radius,
            fill,
            stroke,
            egui::StrokeKind::Inside,
        ))
    });
}

/// The OS display name, with the shipping `session.display.name || "Display"` fallback.
fn display_label(display: &DisplayDescriptor) -> &str {
    let name = display.name.trim();
    if name.is_empty() { "Display" } else { name }
}

fn truncate_label(label: &str, maximum_characters: usize) -> String {
    if label.chars().count() <= maximum_characters {
        return label.into();
    }

    let mut truncated = label
        .chars()
        .take(maximum_characters.saturating_sub(1))
        .collect::<String>();
    truncated.push('…');
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;

    fn display() -> DisplayDescriptor {
        DisplayDescriptor {
            id: "display".into(),
            name: "Fixture display".into(),
            x: 0,
            y: 0,
            width: 1000,
            height: 720,
            scale_factor: 1.,
            is_primary: true,
        }
    }

    fn run_frame(
        ctx: &egui::Context,
        controls: &mut CaptureControls,
        size: egui::Vec2,
        events: Vec<egui::Event>,
        panel_id: egui::Id,
    ) -> Option<Action> {
        run_frame_with_auto_start(ctx, controls, size, events, panel_id, false)
    }

    fn run_frame_with_auto_start(
        ctx: &egui::Context,
        controls: &mut CaptureControls,
        size: egui::Vec2,
        events: Vec<egui::Event>,
        panel_id: egui::Id,
        auto_start: bool,
    ) -> Option<Action> {
        render(ctx, controls, size, events, panel_id, auto_start, None).0
    }

    /// One pass; also returns every painted text run with its screen rect.
    fn render(
        ctx: &egui::Context,
        controls: &mut CaptureControls,
        size: egui::Vec2,
        events: Vec<egui::Event>,
        panel_id: egui::Id,
        auto_start: bool,
        error: Option<&str>,
    ) -> (Option<Action>, Vec<(String, egui::Rect)>) {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
        let display = display();
        let displays = [
            display.clone(),
            DisplayDescriptor {
                id: "display-2".into(),
                name: "Secondary".into(),
                x: display.width as i32,
                is_primary: false,
                ..display.clone()
            },
        ];
        let tokens = crate::tokens::load()["dark-mustard"].clone();
        ctx.begin_pass(egui::RawInput {
            screen_rect: Some(screen),
            events,
            ..Default::default()
        });
        let mut ui = egui::Ui::new(
            ctx.clone(),
            egui::Id::unique("capture-controls-input-test"),
            egui::UiBuilder::new().max_rect(screen),
        );
        let action = controls.show(
            &mut ui,
            &tokens,
            View {
                panel_id,
                frozen: None,
                display: &display,
                displays: &displays,
                windows: &[],
                auto_start,
                recording_available: true,
                recording_unavailable_reason: None,
                error,
            },
            |_| None,
        );
        let mut output = ctx.end_pass();
        output.textures_delta.clear();
        fn collect(shape: &egui::Shape, texts: &mut Vec<(String, egui::Rect)>) {
            match shape {
                egui::Shape::Text(text) => {
                    texts.push((text.galley.text().to_owned(), text.visual_bounding_rect()));
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        collect(shape, texts);
                    }
                }
                _ => {}
            }
        }
        let mut texts = Vec::new();
        for clipped in &output.shapes {
            collect(&clipped.shape, &mut texts);
        }
        (action, texts)
    }

    fn painted(texts: &[(String, egui::Rect)], text: &str) -> Option<egui::Rect> {
        texts
            .iter()
            .find(|(painted, _)| painted == text)
            .map(|(_, rect)| *rect)
    }

    /// Settle the Area layout and guidance animation, then return painted text.
    fn settle(
        controls: &mut CaptureControls,
        auto_start: bool,
        error: Option<&str>,
    ) -> (egui::Context, egui::Id, Vec<(String, egui::Rect)>) {
        let ctx = egui::Context::default();
        // Settle the Record row's entrance (40 ms delay, then instant); its
        // timing has its own test.
        crate::motion::set_reduced(&ctx, true);
        let panel_id = egui::Id::unique("capture-controls-settle");
        let size = egui::vec2(1280., 900.);
        for _ in 0..4 {
            render(&ctx, controls, size, vec![], panel_id, auto_start, error);
        }
        let texts = render(&ctx, controls, size, vec![], panel_id, auto_start, error).1;
        (ctx, panel_id, texts)
    }

    fn run_input(controls: &mut CaptureControls, events: Vec<egui::Event>) -> Option<Action> {
        run_frame(
            &egui::Context::default(),
            controls,
            egui::vec2(1000., 720.),
            events,
            egui::Id::unique("capture-controls-input-toolbar"),
        )
    }

    fn key(key: egui::Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    fn pointer_button(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }

    fn click(
        ctx: &egui::Context,
        controls: &mut CaptureControls,
        position: egui::Pos2,
        panel_id: egui::Id,
    ) {
        run_frame(
            ctx,
            controls,
            egui::vec2(1000., 720.),
            vec![
                egui::Event::PointerMoved(position),
                pointer_button(position, true),
            ],
            panel_id,
        );
        run_frame(
            ctx,
            controls,
            egui::vec2(1000., 720.),
            vec![pointer_button(position, false)],
            panel_id,
        );
    }

    #[test]
    fn target_switching_retains_settled_region_and_window_choices() {
        let mut controls = CaptureControls::default();
        controls.region.exercise(
            0,
            Bounds {
                width: 1000.,
                height: 720.,
            },
        );
        controls.window.exercise(0, |_| Some(2));
        controls.mode = TargetMode::Window;
        assert_eq!(controls.current_target(), Some(Target::Window(2)));
        controls.mode = TargetMode::Region;
        assert!(matches!(controls.current_target(), Some(Target::Region(_))));
        controls.mode = TargetMode::Display;
        assert_eq!(controls.current_target(), Some(Target::Display));
    }

    #[test]
    fn keyboard_target_shortcuts_apply_shipping_selection_policy_without_auto_start() {
        let bounds = Bounds {
            width: 1000.,
            height: 720.,
        };
        let mut controls = CaptureControls::default();
        controls.region.exercise(0, bounds);
        let settled_region = controls.region();
        controls.window.exercise(0, |_| Some(2));

        controls.apply_target_shortcut(CaptureShortcut::Window);
        assert_eq!(controls.mode(), TargetMode::Window);
        assert_eq!(controls.window(), Some(SelectionTarget::Window(2)));
        assert_eq!(controls.window.hovered(), None);
        assert_eq!(controls.region(), settled_region);

        controls.apply_target_shortcut(CaptureShortcut::Display);
        assert_eq!(controls.mode(), TargetMode::Display);
        assert_eq!(controls.window(), None);
        assert_eq!(controls.window.hovered(), None);
        assert_eq!(controls.region(), settled_region);
        assert_eq!(
            run_frame_with_auto_start(
                &egui::Context::default(),
                &mut controls,
                egui::vec2(1000., 720.),
                vec![],
                egui::Id::unique("capture-controls-shortcut-auto-start"),
                true,
            ),
            None,
            "keyboard target changes must not arm pointer auto-start",
        );

        controls.window.exercise(0, |_| Some(1));
        controls.apply_target_shortcut(CaptureShortcut::Region);
        assert_eq!(controls.mode(), TargetMode::Region);
        assert_eq!(controls.window(), None);
        assert_eq!(controls.window.hovered(), None);
        assert_eq!(controls.region(), settled_region);
    }

    #[test]
    fn recording_shortcuts_switch_mode_preserve_region_and_require_confirmation() {
        let mut controls = CaptureControls::default();
        controls.region.exercise(
            0,
            Bounds {
                width: 1000.,
                height: 720.,
            },
        );
        let region = controls.region();
        for (shortcut, target) in [
            (CaptureShortcut::RecordWindow, TargetMode::Window),
            (CaptureShortcut::RecordDisplay, TargetMode::Display),
            (CaptureShortcut::RecordRegion, TargetMode::Region),
        ] {
            controls.window.exercise(0, |_| Some(2));
            controls.apply_target_shortcut(shortcut);
            assert_eq!(controls.mode(), target);
            assert_eq!(controls.action_mode, ActionMode::Recording);
            assert_eq!(controls.region(), region);
            assert_eq!(
                controls.window(),
                (target == TargetMode::Window).then_some(SelectionTarget::Window(2))
            );
            assert_eq!(controls.window.hovered(), None);
            assert_eq!(
                run_frame_with_auto_start(
                    &egui::Context::default(),
                    &mut controls,
                    egui::vec2(1000., 720.),
                    vec![],
                    egui::Id::unique("record-shortcut"),
                    true
                ),
                None
            );
        }
        assert!(matches!(
            run_input(&mut controls, vec![key(egui::Key::Enter)]),
            Some(Action::StartRecording(Target::Region(_)))
        ));
        controls.apply_target_shortcut(CaptureShortcut::Display);
        assert_eq!(controls.action_mode, ActionMode::Screenshot);
        assert_eq!(
            run_input(&mut controls, vec![key(egui::Key::Enter)]),
            Some(Action::Capture(Target::Display))
        );
    }

    #[test]
    fn reset_clears_choices_and_returns_to_empty_region() {
        let mut controls = CaptureControls::default();
        controls.region.exercise(
            0,
            Bounds {
                width: 1000.,
                height: 720.,
            },
        );
        controls.window.exercise(0, |_| Some(1));
        controls.reset();
        assert_eq!(controls.mode(), TargetMode::Region);
        assert!(controls.region().is_none());
        assert!(controls.window().is_none());
        assert!(controls.current_target().is_none());
    }

    #[test]
    fn display_change_retains_mode_but_clears_display_local_choices() {
        let mut controls = CaptureControls::default();
        controls.region.exercise(
            0,
            Bounds {
                width: 1000.,
                height: 720.,
            },
        );
        controls.window.exercise(0, |_| Some(1));
        controls.mode = TargetMode::Window;
        controls.reset_for_display_change();
        assert_eq!(controls.mode(), TargetMode::Window);
        assert!(controls.region().is_none());
        assert!(controls.window().is_none());
    }

    #[test]
    fn empty_desktop_click_changes_window_segment_to_full_screen() {
        let ctx = egui::Context::default();
        let panel_id = egui::Id::unique("capture-controls-desktop-click");
        let mut controls = CaptureControls {
            mode: TargetMode::Window,
            ..Default::default()
        };
        run_frame(
            &ctx,
            &mut controls,
            egui::vec2(1000., 720.),
            vec![],
            panel_id,
        );
        click(&ctx, &mut controls, egui::pos2(100., 100.), panel_id);
        assert_eq!(controls.mode(), TargetMode::Display);
        assert_eq!(controls.current_target(), Some(Target::Display));
    }

    #[test]
    fn escape_cancels_but_enter_requires_a_valid_target() {
        let mut controls = CaptureControls::default();
        assert_eq!(run_input(&mut controls, vec![key(egui::Key::Enter)]), None);
        assert_eq!(
            run_input(&mut controls, vec![key(egui::Key::Escape)]),
            Some(Action::Cancel)
        );

        controls.region.exercise(
            0,
            Bounds {
                width: 1000.,
                height: 720.,
            },
        );
        assert!(matches!(
            run_input(&mut controls, vec![key(egui::Key::Enter)]),
            Some(Action::Capture(Target::Region(_)))
        ));
    }

    #[test]
    fn narrow_toolbar_stays_inside_sixteen_point_monitor_margins() {
        let ctx = egui::Context::default();
        let panel_id = egui::Id::unique("capture-controls-narrow-toolbar");
        let mut controls = CaptureControls {
            mode: TargetMode::Display,
            ..Default::default()
        };
        for _ in 0..2 {
            run_frame(
                &ctx,
                &mut controls,
                egui::vec2(768., 720.),
                vec![],
                panel_id,
            );
        }
        let rect = ctx
            .memory(|memory| memory.area_rect(panel_id))
            .expect("capture toolbar area");
        assert!(rect.left() >= 16., "left edge was {}", rect.left());
        assert!(rect.right() <= 752., "right edge was {}", rect.right());
        assert!(rect.bottom() <= 704., "bottom edge was {}", rect.bottom());
    }

    #[test]
    fn wide_toolbar_keeps_shipping_width_and_center_across_targets() {
        let ctx = egui::Context::default();
        let panel_id = egui::Id::unique("capture-controls-wide-toolbar");
        let size = egui::vec2(1280., 900.);
        let mut controls = CaptureControls::default();
        let mut rects = Vec::new();
        for mode in [TargetMode::Region, TargetMode::Window, TargetMode::Display] {
            controls.mode = mode;
            for _ in 0..2 {
                run_frame(&ctx, &mut controls, size, vec![], panel_id);
            }
            rects.push(ctx.memory(|memory| memory.area_rect(panel_id)).unwrap());
        }

        for rect in &rects {
            assert!((rect.width() - 854.).abs() <= 1., "width was {rect:?}");
            assert!((rect.center().x - 640.).abs() <= 1., "center was {rect:?}");
            assert!((rect.bottom() - 874.).abs() <= 1., "bottom was {rect:?}");
        }
    }

    #[test]
    fn toolbar_background_drag_moves_and_clamps_without_starting_region() {
        let ctx = egui::Context::default();
        let panel_id = egui::Id::unique("capture-controls-movable-toolbar");
        let size = egui::vec2(1000., 720.);
        let mut controls = CaptureControls::default();
        for _ in 0..2 {
            run_frame(&ctx, &mut controls, size, vec![], panel_id);
        }
        let original = ctx.memory(|memory| memory.area_rect(panel_id)).unwrap();
        let start = egui::pos2(original.center().x, original.bottom() - 10.);
        run_frame(
            &ctx,
            &mut controls,
            size,
            vec![
                egui::Event::PointerMoved(start),
                pointer_button(start, true),
            ],
            panel_id,
        );
        let destination = egui::pos2(4., 4.);
        run_frame(
            &ctx,
            &mut controls,
            size,
            vec![egui::Event::PointerMoved(destination)],
            panel_id,
        );
        run_frame(
            &ctx,
            &mut controls,
            size,
            vec![pointer_button(destination, false)],
            panel_id,
        );

        let moved = ctx.memory(|memory| memory.area_rect(panel_id)).unwrap();
        assert_ne!(moved, original);
        assert!(moved.left() >= 16., "left edge was {}", moved.left());
        assert!(moved.top() >= 16., "top edge was {}", moved.top());
        assert!(controls.region().is_none());
    }

    #[test]
    fn narrow_display_label_is_unicode_safe() {
        assert_eq!(truncate_label("Built-in display", 8), "Built-i…");
        assert_eq!(truncate_label("主ディスプレイ", 8), "主ディスプレイ");
    }

    #[test]
    fn display_label_uses_os_name_then_shipping_fallback() {
        let mut named = display();
        named.name = "  Built-in Retina Display ".into();
        assert_eq!(display_label(&named), "Built-in Retina Display");
        named.name = " ".into();
        assert_eq!(display_label(&named), "Display");
    }

    #[test]
    fn primary_button_follows_shipping_labels_and_auto_start_hide_rule() {
        let mut controls = CaptureControls::default();
        let (_, _, texts) = settle(&mut controls, false, None);
        assert!(painted(&texts, "Capture").is_some());
        let (_, _, texts) = settle(&mut controls, true, None);
        assert!(
            painted(&texts, "Capture").is_none(),
            "auto-start hides the button"
        );
        let (_, _, texts) = settle(&mut controls, true, Some("Display changed"));
        assert!(painted(&texts, "Retry capture").is_some());
        assert!(painted(&texts, "Display changed").is_some());
        controls.action_mode = ActionMode::Recording;
        let (_, _, texts) = settle(&mut controls, false, None);
        assert!(painted(&texts, "Start recording").is_some());
    }

    #[test]
    fn visibility_note_follows_capabilities_and_links_to_preferences() {
        let mut controls = CaptureControls::default();
        let (ctx, panel_id, texts) = settle(&mut controls, true, None);
        let expected = if cfg!(target_os = "linux") {
            "These controls will show in screenshots"
        } else {
            "These controls won’t show in screenshots"
        };
        assert!(painted(&texts, expected).is_some(), "{texts:?}");
        let auto_start = painted(&texts, capture_menu::AUTO_START_NOTE).expect("auto-start link");
        let size = egui::vec2(1280., 900.);
        let position = auto_start.center();
        render(
            &ctx,
            &mut controls,
            size,
            vec![
                egui::Event::PointerMoved(position),
                pointer_button(position, true),
            ],
            panel_id,
            true,
            None,
        );
        let (action, _) = render(
            &ctx,
            &mut controls,
            size,
            vec![pointer_button(position, false)],
            panel_id,
            true,
            None,
        );
        assert_eq!(
            action,
            Some(Action::OpenPreference(
                PreferenceTarget::AutoStartOnSelection
            ))
        );
        let (_, _, texts) = settle(&mut controls, false, None);
        assert!(painted(&texts, "Press Enter to confirm").is_some());
    }

    #[test]
    fn record_options_row_arrives_after_the_shipping_delay_and_never_under_reduced_motion() {
        let mut controls = CaptureControls::recording_fixture();
        let ctx = egui::Context::default();
        let panel_id = egui::Id::unique("capture-controls-arrival");
        let size = egui::vec2(1280., 900.);
        // The first frames fall inside the 40 ms delay: the row is invisible.
        let texts = render(&ctx, &mut controls, size, vec![], panel_id, false, None).1;
        assert!(painted(&texts, "FPS").is_none(), "{texts:?}");
        for _ in 0..30 {
            render(&ctx, &mut controls, size, vec![], panel_id, false, None);
        }
        let texts = render(&ctx, &mut controls, size, vec![], panel_id, false, None).1;
        assert!(painted(&texts, "FPS").is_some(), "{texts:?}");

        // Reduced motion keeps the delay, then lands at rest with no frames between.
        let mut controls = CaptureControls::recording_fixture();
        let ctx = egui::Context::default();
        crate::motion::set_reduced(&ctx, true);
        let texts = render(&ctx, &mut controls, size, vec![], panel_id, false, None).1;
        assert!(painted(&texts, "FPS").is_none(), "{texts:?}");
        for _ in 0..3 {
            render(&ctx, &mut controls, size, vec![], panel_id, false, None);
        }
        let texts = render(&ctx, &mut controls, size, vec![], panel_id, false, None).1;
        assert!(painted(&texts, "FPS").is_some(), "{texts:?}");
    }

    #[test]
    fn recording_row_uses_labelled_fields_switch_states_and_microphone_loading() {
        let mut controls = CaptureControls::recording_fixture();
        let mut capabilities = RecordingCapabilities::current(false);
        capabilities.cursor_control = true;
        capabilities.click_highlights = false;
        capabilities.microphone = true;
        controls.configure_recording(&RecordingSettings::default(), capabilities);
        let (_, _, texts) = settle(&mut controls, false, None);
        for label in [
            "FPS",
            "MAX RESOLUTION",
            "SHOW CURSOR",
            "SHOW CLICKS",
            "DESKTOP AUDIO",
            "MICROPHONE",
            "Unavailable",
            "Off",
        ] {
            assert!(
                painted(&texts, label).is_some(),
                "{label} missing: {texts:?}"
            );
        }
        assert!(painted(&texts, "Loading microphones…").is_none());
        controls.recording.microphone_device_id = Some("missing".into());
        let (_, _, texts) = settle(&mut controls, false, None);
        assert!(
            painted(&texts, "Loading microphone…").is_some(),
            "{texts:?}"
        );
        controls.set_microphones(vec![]);
        let (_, _, texts) = settle(&mut controls, false, None);
        assert!(
            painted(&texts, "Selected microphone").is_some(),
            "{texts:?}"
        );
    }

    #[test]
    fn full_screen_shows_display_identity_with_record_fps() {
        let mut controls = CaptureControls {
            mode: TargetMode::Display,
            ..Default::default()
        };
        let (_, _, texts) = settle(&mut controls, false, None);
        assert!(painted(&texts, "Fixture display").is_some());
        assert!(painted(&texts, "1000 × 720").is_some());
        assert!(painted(&texts, "Click to capture this display").is_none());
        controls.action_mode = ActionMode::Recording;
        let (_, _, texts) = settle(&mut controls, false, None);
        assert!(
            painted(&texts, "1000 × 720 · 60 FPS").is_some(),
            "{texts:?}"
        );
    }

    #[test]
    fn menu_guidance_uses_shipping_copy_and_hides_after_window_selection() {
        let mut controls = CaptureControls::default();
        let (_, _, texts) = settle(&mut controls, false, None);
        let chip = painted(&texts, "Drag to select a region").expect("region guidance");
        assert!(painted(&texts, "Shift for square · Esc to cancel").is_some());
        assert!(
            (chip.top() - 900. * 0.16).abs() < 24.,
            "chip at 16%: {chip:?}"
        );
        controls.mode = TargetMode::Window;
        let (_, _, texts) = settle(&mut controls, false, None);
        assert!(painted(&texts, "Select a window to continue").is_some());
        controls.window.exercise(0, |_| Some(0));
        let (_, _, texts) = settle(&mut controls, false, None);
        assert!(painted(&texts, "Select a window to continue").is_none());
    }
}
