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
    Stop,
    Pause,
    Play,
    Restart,
    Screenshot,
    Microphone,
    Discard,
    Hide,
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
}

pub fn show(ui: &mut egui::Ui, tokens: &Tokens, view: View<'_>) -> Option<Action> {
    let mut action = None;
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
                                    ui.allocate_exact_size(Vec2::splat(12.), egui::Sense::hover());
                                ui.painter().circle_filled(
                                    status_rect.center(),
                                    5.,
                                    tokens.color("theme-signal"),
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
                            if view.paused { Icon::Play } else { Icon::Pause },
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
                            Icon::Restart,
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
                            Icon::Screenshot,
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
                        microphone_meter(ui, tokens, &view);
                        let microphone_label = if view.microphone_muted {
                            "Unmute microphone"
                        } else {
                            "Mute microphone"
                        };
                        if view.has_microphone {
                            if control(
                                ui,
                                Icon::Microphone,
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
                                Icon::Microphone,
                                "Microphone unavailable: no microphone selected",
                                "Select a microphone before starting a recording",
                            );
                        }
                        if control(
                            ui,
                            Icon::Discard,
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
                            Icon::Hide,
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

fn paint_icon(ui: &egui::Ui, rect: Rect, icon: Icon, color: egui::Color32) {
    let painter = ui.painter();
    let center = rect.center();
    let stroke = Stroke::new(1.5, color);
    match icon {
        Icon::Stop => {
            painter.rect_filled(Rect::from_center_size(center, Vec2::splat(8.)), 1., color);
        }
        Icon::Pause => {
            for x in [-3., 3.] {
                painter.rect_filled(
                    Rect::from_center_size(center + Vec2::new(x, 0.), Vec2::new(3., 11.)),
                    1.,
                    color,
                );
            }
        }
        Icon::Play => {
            painter.add(egui::Shape::convex_polygon(
                vec![
                    center + Vec2::new(-4., -6.),
                    center + Vec2::new(6., 0.),
                    center + Vec2::new(-4., 6.),
                ],
                color,
                Stroke::NONE,
            ));
        }
        Icon::Restart => {
            painter.circle_stroke(center, 6., stroke);
            painter.line_segment(
                [center + Vec2::new(-7., -1.), center + Vec2::new(-7., -6.)],
                stroke,
            );
            painter.line_segment(
                [center + Vec2::new(-7., -6.), center + Vec2::new(-2., -6.)],
                stroke,
            );
        }
        Icon::Screenshot => {
            painter.rect_stroke(
                Rect::from_center_size(center, Vec2::new(13., 9.)),
                2.,
                stroke,
                StrokeKind::Middle,
            );
            painter.circle_stroke(center, 2.5, stroke);
        }
        Icon::Microphone => {
            painter.rect_stroke(
                Rect::from_center_size(center + Vec2::new(0., -2.), Vec2::new(6., 10.)),
                3.,
                stroke,
                StrokeKind::Middle,
            );
            painter.line_segment(
                [center + Vec2::new(-6., 1.), center + Vec2::new(-4., 5.)],
                stroke,
            );
            painter.line_segment(
                [center + Vec2::new(4., 5.), center + Vec2::new(6., 1.)],
                stroke,
            );
            painter.line_segment(
                [center + Vec2::new(0., 5.), center + Vec2::new(0., 8.)],
                stroke,
            );
        }
        Icon::Discard => {
            painter.rect_stroke(
                Rect::from_min_max(center + Vec2::new(-4., -3.), center + Vec2::new(4., 6.)),
                1.,
                stroke,
                StrokeKind::Middle,
            );
            painter.line_segment(
                [center + Vec2::new(-6., -6.), center + Vec2::new(6., -6.)],
                stroke,
            );
            painter.line_segment(
                [center + Vec2::new(-2., -8.), center + Vec2::new(2., -8.)],
                stroke,
            );
        }
        Icon::Hide => {
            painter.circle_stroke(center, 6., stroke);
            painter.circle_filled(center, 2., color);
            painter.line_segment(
                [center + Vec2::new(-7., 7.), center + Vec2::new(7., -7.)],
                Stroke::new(2., color),
            );
        }
    }
}

/// Shipping `recordingStatusLabel` copy for the states this HUD renders.
fn status_label(paused: bool) -> &'static str {
    if paused { "Paused" } else { "Recording" }
}

#[cfg(test)]
mod tests {
    use super::*;

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
