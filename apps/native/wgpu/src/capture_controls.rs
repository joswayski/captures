use captures_app::selection::{Bounds, Rect};
use captures_capture::{DisplayDescriptor, WindowDescriptor};
use eframe::egui::{self, Align2, RichText, Stroke, TextureHandle};

use crate::{
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Target {
    Region(Rect),
    Window(usize),
    Display,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Capture(Target),
    SwitchDisplay(String),
    Cancel,
}

pub struct View<'a> {
    pub frozen: Option<&'a TextureHandle>,
    pub display: &'a DisplayDescriptor,
    pub displays: &'a [DisplayDescriptor],
    pub windows: &'a [WindowDescriptor],
    pub auto_start: bool,
}

#[derive(Default)]
pub struct CaptureControls {
    mode: TargetMode,
    region: Selector,
    window: WindowSelector,
}

impl CaptureControls {
    pub fn fixture() -> Self {
        Self {
            window: WindowSelector::fixture(),
            ..Self::default()
        }
    }

    #[cfg(test)]
    fn mode(&self) -> TargetMode {
        self.mode
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
        let mut action = match self.mode {
            TargetMode::Region => self
                .region
                .show_surface(ui, tokens, view.frozen, view.auto_start, Some(bounds))
                .and_then(|action| match action {
                    selector::Action::Confirm => self.current_target().map(Action::Capture),
                    selector::Action::Cancel => Some(Action::Cancel),
                }),
            TargetMode::Window => self
                .window
                .show_surface(
                    ui,
                    tokens,
                    &window_selector::View {
                        frozen: view.frozen,
                        display: view.display,
                        windows: view.windows,
                        auto_start: view.auto_start,
                    },
                    hit_test,
                )
                .map(|target| Action::Capture(window_target(target))),
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
                );
                (clicked && view.auto_start).then_some(Action::Capture(Target::Display))
            }
        };

        if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
            action = Some(Action::Cancel);
        } else if ui.input(|input| input.key_pressed(egui::Key::Enter))
            && let Some(target) = self.current_target()
        {
            action = Some(Action::Capture(target));
        }

        let panel = egui::Area::new(egui::Id::unique("capture-controls-toolbar"))
            .anchor(Align2::CENTER_BOTTOM, egui::vec2(0., -26.))
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                egui::Frame::new()
                    .fill(tokens.color("glass-strong"))
                    .stroke(Stroke::new(1., tokens.color("glass-border")))
                    .corner_radius(tokens.number("r-2xl") as u8)
                    .inner_margin(tokens.number("s-4") as i8)
                    .show(ui, |ui| {
                        tokens.glass_controls(ui);
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
                                let _ = segment(ui, tokens, true, "Screenshot");
                                ui.add_enabled(false, egui::Button::new("Record"))
                                    .on_disabled_hover_text(
                                        "Screen recording is not available yet",
                                    );
                                ui.separator();
                                for (mode, label) in [
                                    (TargetMode::Region, "Region"),
                                    (TargetMode::Window, "Window"),
                                    (TargetMode::Display, "Full screen"),
                                ] {
                                    let enabled = mode != TargetMode::Window || !view.windows.is_empty();
                                    let response = ui.add_enabled_ui(enabled, |ui| {
                                        segment(ui, tokens, self.mode == mode, label)
                                    }).inner;
                                    if !enabled {
                                        response.clone().on_disabled_hover_text(
                                            "Window capture is not available in this desktop session",
                                        );
                                    }
                                    if response.clicked() {
                                        let same = self.mode == mode;
                                        self.mode = mode;
                                        if mode == TargetMode::Display && view.auto_start {
                                            action = Some(Action::Capture(Target::Display));
                                        } else if same
                                            && view.auto_start
                                            && let Some(target) = self.current_target()
                                        {
                                            action = Some(Action::Capture(target));
                                        }
                                    }
                                }
                                if self.mode == TargetMode::Region {
                                    ui.separator();
                                    self.region.show_aspect_picker(ui, tokens, bounds);
                                } else if self.mode == TargetMode::Display
                                    && view.displays.len() > 1
                                {
                                    ui.separator();
                                    let mut display_id = view.display.id.clone();
                                    egui::ComboBox::from_id_salt("capture-controls-display")
                                        .selected_text(display_label(view.display, view.displays))
                                        .show_ui(ui, |ui| {
                                            for (index, display) in view.displays.iter().enumerate()
                                            {
                                                ui.selectable_value(
                                                    &mut display_id,
                                                    display.id.clone(),
                                                    format!(
                                                        "{} — {}×{}{}",
                                                        display.name,
                                                        display.width,
                                                        display.height,
                                                        if display.is_primary {
                                                            " (Primary)"
                                                        } else {
                                                            ""
                                                        }
                                                    ),
                                                )
                                                .on_hover_text(format!("Display {}", index + 1));
                                            }
                                        });
                                    if display_id != view.display.id {
                                        action = Some(Action::SwitchDisplay(display_id));
                                    }
                                }
                                if !view.auto_start {
                                    ui.separator();
                                    let target = self.current_target();
                                    if ui
                                        .add_enabled(
                                            target.is_some(),
                                            egui::Button::new(
                                                RichText::new("Capture")
                                                    .color(tokens.color("theme-accent-ink")),
                                            )
                                            .fill(tokens.color("theme-accent"))
                                            .stroke(Stroke::NONE),
                                        )
                                        .on_hover_text("Take screenshot (Enter)")
                                        .clicked()
                                        && let Some(target) = target
                                    {
                                        action = Some(Action::Capture(target));
                                    }
                                }
                            });
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new("Capture controls are excluded from screenshots")
                                        .small()
                                        .color(tokens.color("glass-text-muted")),
                                );
                                ui.label(
                                    RichText::new(if view.auto_start {
                                        "· Auto-capture is on"
                                    } else {
                                        "· Press Enter to confirm"
                                    })
                                    .small()
                                    .color(tokens.color("glass-text-subtle")),
                                );
                            });
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
}

fn window_target(target: SelectionTarget) -> Target {
    match target {
        SelectionTarget::Display => Target::Display,
        SelectionTarget::Window(index) => Target::Window(index),
    }
}

fn segment(ui: &mut egui::Ui, tokens: &Tokens, selected: bool, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(label).color(tokens.color(if selected {
            "glass-text"
        } else {
            "glass-text-muted"
        })))
        .fill(tokens.color(if selected {
            "glass-raised"
        } else {
            "glass-strong"
        }))
        .stroke(if selected {
            Stroke::new(1., tokens.color("glass-border"))
        } else {
            Stroke::NONE
        }),
    )
}

fn display_label(display: &DisplayDescriptor, displays: &[DisplayDescriptor]) -> String {
    if display.name.trim().is_empty() {
        displays
            .iter()
            .position(|candidate| candidate.id == display.id)
            .map_or_else(
                || "Display".into(),
                |index| format!("Display {}", index + 1),
            )
    } else {
        display.name.clone()
    }
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

    fn run_input(controls: &mut CaptureControls, events: Vec<egui::Event>) -> Option<Action> {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000., 720.));
        let display = display();
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
                frozen: None,
                display: &display,
                displays: std::slice::from_ref(&display),
                windows: &[],
                auto_start: false,
            },
            |_| None,
        );
        let mut output = ctx.end_pass();
        output.textures_delta.clear();
        action
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
}
