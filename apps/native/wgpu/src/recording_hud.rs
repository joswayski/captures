use eframe::egui::{self, Align, Layout, Rect, RichText, Stroke, StrokeKind, Vec2};

use crate::tokens::Tokens;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    Pause,
    Resume,
    Restart,
    Screenshot,
    Stop,
    SetMicrophoneMuted(bool),
    Discard,
    Hide,
}

#[derive(Clone, Copy)]
enum Icon {
    /// The shipping stop control paints a filled signal square rather than an SVG.
    Stop,
    Shipping(&'static str),
}

pub struct View<'a> {
    pub paused: bool,
    pub busy: bool,
    pub has_microphone: bool,
    pub microphone_muted: bool,
    pub microphone_peak: f32,
    pub elapsed_ms: u64,
    pub notice: &'a str,
    pub warning: bool,
    pub hide_available: bool,
    pub reduced_motion: bool,
}

fn control_rects_id() -> egui::Id {
    egui::Id::unique("recording-hud-control-rects")
}

pub fn show(ui: &mut egui::Ui, tokens: &Tokens, view: View<'_>) -> Option<Action> {
    let mut action = None;
    // Shipping `startHudDrag`: the HUD background moves the window, controls never do.
    // Registered first so every control sits above it in hit testing.
    let background = ui.interact(
        ui.max_rect(),
        ui.scope_id().with("recording-hud-drag"),
        egui::Sense::drag(),
    );
    let previous_controls: Vec<Rect> = ui
        .ctx()
        .data_mut(|data| data.remove_temp(control_rects_id()))
        .unwrap_or_default();
    if background.drag_started()
        && ui
            .input(|input| input.pointer.press_origin())
            .is_some_and(|origin| !previous_controls.iter().any(|rect| rect.contains(origin)))
    {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }
    tokens.glass_controls(ui);
    let widgets = &mut ui.visuals_mut().widgets;
    for widget in [&mut widgets.inactive, &mut widgets.noninteractive] {
        widget.bg_fill = egui::Color32::TRANSPARENT;
        widget.weak_bg_fill = egui::Color32::TRANSPARENT;
    }
    for widget in [
        &mut widgets.inactive,
        &mut widgets.noninteractive,
        &mut widgets.hovered,
        &mut widgets.active,
        &mut widgets.open,
    ] {
        widget.bg_stroke = Stroke::NONE;
        widget.corner_radius = (tokens.number("r-sm") as u8).into();
    }
    ui.add_space(6.);
    ui.horizontal(|ui| {
        ui.add_space(6.);
        egui::Frame::new()
            .fill(tokens.color("glass-strong"))
            .stroke(Stroke::new(1., tokens.color("glass-border")))
            .corner_radius(tokens.number("r-xl") as u8)
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.set_width(394.);
                ui.spacing_mut().item_spacing = Vec2::new(4., 6.);
                ui.with_layout(Layout::top_down(Align::Center), |ui| {
                    ui.label(RichText::new(view.notice).small().color(tokens.color(
                        if view.warning {
                            "theme-signal"
                        } else {
                            "glass-text-muted"
                        },
                    )));
                    ui.horizontal(|ui| {
                        ui.allocate_ui_with_layout(
                            Vec2::new(103., 32.),
                            Layout::left_to_right(Align::Center),
                            |ui| {
                                let (status_rect, _) =
                                    ui.allocate_exact_size(Vec2::splat(18.), egui::Sense::hover());
                                paint_status_dot(
                                    ui,
                                    status_rect.center(),
                                    tokens.color(if view.paused {
                                        "theme-accent"
                                    } else {
                                        "theme-signal"
                                    }),
                                    pulse_phase(
                                        ui.input(|input| input.time),
                                        view.paused || view.reduced_motion,
                                    ),
                                );
                                ui.vertical_centered_justified(|ui| {
                                    ui.monospace(
                                        captures_app::recording_timeline::format_recording_time(
                                            view.elapsed_ms,
                                        ),
                                    );
                                    // Shipping CSS uppercases the status label.
                                    ui.label(
                                        RichText::new(status_label(view.paused).to_uppercase())
                                            .small()
                                            .color(tokens.color("glass-text-muted")),
                                    );
                                });
                            },
                        );
                        if control(
                            ui,
                            Icon::Stop,
                            "Stop and save recording",
                            true,
                            !view.busy,
                            false,
                            tokens,
                        )
                        .clicked()
                        {
                            action = Some(Action::Stop);
                        }
                        if control(
                            ui,
                            Icon::Shipping(if view.paused { "resume" } else { "pause" }),
                            if view.paused {
                                "Resume recording"
                            } else {
                                "Pause recording"
                            },
                            false,
                            !view.busy,
                            false,
                            tokens,
                        )
                        .clicked()
                        {
                            action = Some(if view.paused {
                                Action::Resume
                            } else {
                                Action::Pause
                            });
                        }
                        if control(
                            ui,
                            Icon::Shipping("restart"),
                            "Restart recording",
                            false,
                            !view.busy,
                            false,
                            tokens,
                        )
                        .clicked()
                        {
                            action = Some(Action::Restart);
                        }
                        if control(
                            ui,
                            Icon::Shipping("capture"),
                            "Take a region screenshot",
                            false,
                            !view.busy,
                            false,
                            tokens,
                        )
                        .clicked()
                        {
                            action = Some(Action::Screenshot);
                        }
                        // Shipping hides the level meter without a selected microphone; the
                        // slot stays reserved so controls keep their positions on both hosts.
                        if view.has_microphone {
                            microphone_meter(ui, tokens, &view);
                        } else {
                            ui.add_space(tokens.number("s-9"));
                        }
                        let microphone_label = if view.microphone_muted {
                            "Unmute microphone"
                        } else {
                            "Mute microphone"
                        };
                        if view.has_microphone {
                            if control(
                                ui,
                                Icon::Shipping(if view.microphone_muted {
                                    "microphone-muted"
                                } else {
                                    "microphone"
                                }),
                                microphone_label,
                                false,
                                !view.busy,
                                view.microphone_muted,
                                tokens,
                            )
                            .clicked()
                            {
                                action = Some(Action::SetMicrophoneMuted(!view.microphone_muted));
                            }
                        } else {
                            unavailable(
                                ui,
                                Icon::Shipping("microphone"),
                                "Microphone unavailable: no microphone selected",
                                "Select a microphone before starting a recording",
                            );
                        }
                        if control(
                            ui,
                            Icon::Shipping("trash"),
                            "Delete recording",
                            false,
                            !view.busy,
                            false,
                            tokens,
                        )
                        .clicked()
                        {
                            action = Some(Action::Discard);
                        }
                        if control(
                            ui,
                            Icon::Shipping("hide-controls"),
                            if view.hide_available {
                                "Hide recording controls"
                            } else {
                                "Hide unavailable because no tray restore path is available"
                            },
                            false,
                            !view.busy && view.hide_available,
                            false,
                            tokens,
                        )
                        .clicked()
                        {
                            action = Some(Action::Hide);
                        }
                    });
                });
            });
    });
    action
}

fn microphone_meter(ui: &mut egui::Ui, tokens: &Tokens, view: &View<'_>) {
    // Clear immediately during lifecycle changes, before a queued sample can arrive.
    let peak = if view.paused || view.busy || !view.has_microphone || view.microphone_muted {
        0.
    } else {
        view.microphone_peak
    };
    let label = if !view.has_microphone {
        "Microphone level unavailable: no microphone selected".into()
    } else if view.microphone_muted {
        "Microphone muted".into()
    } else if view.paused {
        "Microphone level: paused".into()
    } else if view.busy {
        "Microphone level: recording controls busy".into()
    } else {
        format!("Microphone level {}%", (peak * 100.).round())
    };
    let (slot, response) =
        ui.allocate_exact_size(Vec2::splat(tokens.number("s-9")), egui::Sense::hover());
    let track = Rect::from_center_size(
        slot.center(),
        Vec2::new(slot.width() - tokens.number("s-2"), tokens.number("s-2")),
    );
    ui.painter().rect_filled(
        track,
        tokens.number("r-pill") as u8,
        tokens.color("glass-active"),
    );
    if peak > 0. {
        let fill = Rect::from_min_size(track.min, Vec2::new(track.width() * peak, track.height()));
        ui.painter().rect_filled(
            fill,
            tokens.number("r-pill") as u8,
            tokens.color("glass-text"),
        );
    }
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::ProgressIndicator, true, &label)
    });
    response.on_hover_text(label);
}

fn control(
    ui: &mut egui::Ui,
    icon: Icon,
    description: &str,
    signal: bool,
    enabled: bool,
    selected: bool,
    tokens: &Tokens,
) -> egui::Response {
    ui.scope(|ui| {
        let mut button = egui::Button::new("").min_size(Vec2::splat(32.));
        if signal {
            let widgets = &mut ui.visuals_mut().widgets;
            widgets.inactive.weak_bg_fill = tokens.color("theme-signal-surface");
            widgets.inactive.bg_stroke =
                Stroke::new(1., tokens.color("theme-signal").gamma_multiply(0.4));
            widgets.hovered.weak_bg_fill = tokens.color("theme-signal");
            widgets.active.weak_bg_fill = tokens.color("theme-signal");
        } else if selected && enabled {
            button = button
                .fill(tokens.color("glass-active"))
                .stroke(Stroke::new(
                    1.,
                    tokens.color("theme-accent").gamma_multiply(0.3),
                ));
        }
        let response = ui.add_enabled(enabled, button).on_hover_text(description);
        remember_control(ui, response.rect);
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, description)
        });
        if response.has_focus() {
            ui.painter().rect_stroke(
                response.rect,
                tokens.number("r-sm") as u8,
                Stroke::new(2., tokens.color("theme-accent")),
                StrokeKind::Inside,
            );
        }
        let color = tokens.color(if !enabled {
            "glass-text-subtle"
        } else if signal && !response.hovered() && !response.is_pointer_button_down_on() {
            "theme-signal"
        } else if selected {
            "theme-accent"
        } else if response.hovered() || response.is_pointer_button_down_on() {
            "glass-text"
        } else {
            "glass-text-muted"
        });
        paint_icon(ui, response.rect, icon, color);
        response
    })
    .inner
}

fn unavailable(ui: &mut egui::Ui, icon: Icon, label: &str, description: &str) {
    let response = ui
        .add_enabled(
            false,
            egui::Button::new("")
                .frame(false)
                .min_size(Vec2::splat(32.)),
        )
        .on_disabled_hover_text(description);
    remember_control(ui, response.rect);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, false, label));
    paint_icon(
        ui,
        response.rect,
        icon,
        ui.visuals()
            .widgets
            .noninteractive
            .fg_stroke
            .color
            .gamma_multiply(0.45),
    );
}

fn remember_control(ui: &egui::Ui, rect: Rect) {
    ui.ctx().data_mut(|data| {
        data.get_temp_mut_or_default::<Vec<Rect>>(control_rects_id())
            .push(rect);
    });
}

fn paint_icon(ui: &egui::Ui, rect: Rect, icon: Icon, color: egui::Color32) {
    let painter = ui.painter();
    let center = rect.center();
    match icon {
        Icon::Stop => {
            painter.rect_filled(Rect::from_center_size(center, Vec2::splat(11.)), 2., color);
        }
        Icon::Shipping(name) => {
            // Shipping icons are 24-unit SVGs drawn at 16 px with a 1.8 unit stroke.
            let side = 16.;
            let scale = side / 24.;
            let origin = center - Vec2::splat(side / 2.);
            let stroke = Stroke::new(1.8 * scale, color);
            for line in captures_app::icons::polylines(name).unwrap_or_default() {
                let points: Vec<egui::Pos2> = line
                    .iter()
                    .map(|[x, y]| origin + Vec2::new(x * scale, y * scale))
                    .collect();
                painter.add(egui::Shape::line(points, stroke));
            }
        }
    }
}

/// Shipping `recording-pulse`: a 1.6 s ease-in-out cycle that dims to 0.6 opacity and
/// shrinks to 0.84 scale at its midpoint. Returns 0 at rest and 1 at the midpoint.
fn pulse_phase(time: f64, still: bool) -> f32 {
    if still {
        return 0.;
    }
    let t = (time.rem_euclid(1.6) / 1.6) as f32;
    (1. - (t * std::f32::consts::TAU).cos()) / 2.
}

fn paint_status_dot(ui: &egui::Ui, center: egui::Pos2, color: egui::Color32, phase: f32) {
    let scale = 1. - 0.16 * phase;
    let opacity = 1. - 0.4 * phase;
    // Shipping `box-shadow: 0 0 0 4px` halo at 16% of the dot color.
    ui.painter().circle_filled(
        center,
        (5. + 4.) * scale,
        color.gamma_multiply(0.16 * opacity),
    );
    ui.painter()
        .circle_filled(center, 5. * scale, color.gamma_multiply(opacity));
}

/// Shipping `recordingStatusLabel` copy for the states this HUD renders.
fn status_label(paused: bool) -> &'static str {
    if paused { "Paused" } else { "Recording" }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pulse_rests_while_paused_and_peaks_mid_cycle() {
        assert_eq!(pulse_phase(0.8, true), 0.);
        assert!(pulse_phase(0., false).abs() < 1e-6);
        assert!((pulse_phase(0.8, false) - 1.).abs() < 1e-6);
        assert!(pulse_phase(1.6, false).abs() < 1e-4);
    }

    #[test]
    fn every_hud_icon_has_shipping_geometry() {
        for name in [
            "pause",
            "resume",
            "restart",
            "capture",
            "microphone",
            "microphone-muted",
            "trash",
            "hide-controls",
        ] {
            assert!(
                captures_app::icons::polylines(name).is_some_and(|lines| !lines.is_empty()),
                "{name}"
            );
        }
    }

    #[test]
    fn status_label_uses_shipping_copy() {
        assert_eq!(status_label(false), "Recording");
        assert_eq!(status_label(true), "Paused");
    }

    #[test]
    fn microphone_meter_paints_the_sample_and_clears_inactive_states() {
        let tokens = crate::tokens::load().remove("dark-mustard").unwrap();
        for (peak, paused, busy, has_microphone, muted, width) in [
            (0.625, false, false, true, false, 17.5),
            (1., false, false, true, false, 28.),
            (0., false, false, true, false, 0.),
            (0.625, true, false, true, false, 0.),
            (0.625, false, true, true, false, 0.),
            (0.625, false, false, false, false, 0.),
            (0.625, false, false, true, true, 0.),
        ] {
            let ctx = egui::Context::default();
            let mut output = ctx.run_ui(Default::default(), |ui| {
                microphone_meter(
                    ui,
                    &tokens,
                    &View {
                        paused,
                        busy,
                        has_microphone,
                        microphone_muted: muted,
                        microphone_peak: peak,
                        elapsed_ms: 0,
                        notice: "",
                        warning: false,
                        hide_available: false,
                        reduced_motion: false,
                    },
                );
            });
            output.textures_delta.clear();
            let fills: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Rect(rect) if rect.fill == tokens.color("glass-text") => {
                        Some(rect.rect.width())
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(fills, if width == 0. { vec![] } else { vec![width] });
        }
    }
}
