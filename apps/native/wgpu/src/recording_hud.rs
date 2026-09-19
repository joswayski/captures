use eframe::egui::{self, Align, Layout, Rect, RichText, Stroke, StrokeKind, Vec2};

use crate::tokens::Tokens;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    Pause,
    Resume,
    Stop,
    Discard,
}

#[derive(Clone, Copy)]
enum Icon {
    Stop,
    Pause,
    Play,
    Restart,
    Screenshot,
    Audio,
    Microphone,
    Discard,
    Hide,
}

pub struct View<'a> {
    pub paused: bool,
    pub elapsed_ms: u64,
    pub notice: &'a str,
    pub warning: bool,
}

pub fn show(ui: &mut egui::Ui, tokens: &Tokens, view: View<'_>) -> Option<Action> {
    let mut action = None;
    tokens.glass_controls(ui);
    ui.add_space(6.);
    ui.horizontal(|ui| {
        ui.add_space(6.);
        egui::Frame::new()
            .fill(tokens.color("glass-strong"))
            .stroke(Stroke::new(1., tokens.color("glass-border")))
            .corner_radius(tokens.number("r-2xl") as u8)
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
                                    ui.monospace(format_duration(view.elapsed_ms));
                                    ui.label(
                                        RichText::new(if view.paused {
                                            "PAUSED"
                                        } else {
                                            "RECORDING"
                                        })
                                        .small()
                                        .color(tokens.color("glass-text-muted")),
                                    );
                                });
                            },
                        );
                        if control(ui, Icon::Stop, "Stop and save recording", true, tokens)
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
                        unavailable(
                            ui,
                            Icon::Restart,
                            "Restart recording",
                            "Restart is not available in this build",
                        );
                        unavailable(
                            ui,
                            Icon::Screenshot,
                            "Take screenshot",
                            "Screenshots are not available while recording",
                        );
                        unavailable(
                            ui,
                            Icon::Audio,
                            "System audio",
                            "Audio controls are set before recording",
                        );
                        unavailable(
                            ui,
                            Icon::Microphone,
                            "Microphone",
                            "Microphone controls are set before recording",
                        );
                        if control(ui, Icon::Discard, "Discard recording", false, tokens).clicked()
                        {
                            action = Some(Action::Discard);
                        }
                        unavailable(
                            ui,
                            Icon::Hide,
                            "Hide recording controls",
                            "Hiding controls is not available in this build",
                        );
                    });
                });
            });
    });
    action
}

fn control(
    ui: &mut egui::Ui,
    icon: Icon,
    description: &str,
    signal: bool,
    tokens: &Tokens,
) -> egui::Response {
    let mut button = egui::Button::new("").min_size(Vec2::splat(32.));
    if signal {
        button = button.fill(tokens.color("theme-signal"));
    }
    let response = ui.add(button).on_hover_text(description);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, description));
    paint_icon(
        ui,
        response.rect,
        icon,
        ui.visuals().widgets.active.fg_stroke.color,
    );
    response
}

fn unavailable(ui: &mut egui::Ui, icon: Icon, label: &str, description: &str) {
    let response = ui
        .add_enabled(false, egui::Button::new("").min_size(Vec2::splat(32.)))
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
        Icon::Audio => {
            painter.line_segment(
                [center + Vec2::new(-5., 0.), center + Vec2::new(5., 0.)],
                stroke,
            );
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

fn format_duration(elapsed_ms: u64) -> String {
    let elapsed_seconds = elapsed_ms / 1_000;
    format!("{}:{:02}", elapsed_seconds / 60, elapsed_seconds % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_uses_unpadded_minutes_and_padded_seconds() {
        assert_eq!(format_duration(0), "0:00");
        assert_eq!(format_duration(94_000), "1:34");
    }
}
