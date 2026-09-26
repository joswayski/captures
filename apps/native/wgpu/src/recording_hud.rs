use captures_app::recording_hud::{Control, ControlView};
use captures_recording::RecordingState;
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

#[derive(Clone, Copy)]
pub struct View<'a> {
    /// Lifecycle state the HUD presents (running, paused, saving or failed).
    pub state: RecordingState,
    pub busy: bool,
    pub has_microphone: bool,
    pub microphone_muted: bool,
    pub microphone_peak: f32,
    pub elapsed_ms: u64,
    /// The one-line capture privacy notice above the controls.
    pub notice: &'a str,
    /// Shipping `.recording-hud-error`: one signal line below the card.
    pub error: Option<&'a str>,
    pub hide_available: bool,
    pub reduced_motion: bool,
}

fn control_rects_id() -> egui::Id {
    egui::Id::unique("recording-hud-control-rects")
}

pub fn show(ui: &mut egui::Ui, tokens: &Tokens, view: View<'_>) -> Option<Action> {
    let mut action = None;
    let policy = captures_app::recording_hud::present(&captures_app::recording_hud::Input {
        state: view.state,
        busy: view.busy,
        has_microphone: view.has_microphone,
        microphone_muted: view.microphone_muted,
        hide_available: view.hide_available,
    });
    let bounds = ui.max_rect();
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
    let mut tooltip = None;
    ui.add_space(6.);
    let card = ui
        .horizontal(|ui| {
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
                        ui.add(
                            egui::Label::new(notice_job(tokens, view.notice))
                                .wrap_mode(egui::TextWrapMode::Truncate),
                        );
                        ui.horizontal(|ui| {
                            ui.allocate_ui_with_layout(
                                Vec2::new(103., 32.),
                                Layout::left_to_right(Align::Center),
                                |ui| {
                                    let (status_rect, _) = ui.allocate_exact_size(
                                        Vec2::splat(18.),
                                        egui::Sense::hover(),
                                    );
                                    paint_status_dot(
                                        ui,
                                        status_rect.center(),
                                        tokens.color(policy.dot_token),
                                        policy.dot_halo,
                                        pulse_phase(
                                            ui.input(|input| input.time),
                                            !policy.pulsing || view.reduced_motion,
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
                                            RichText::new(policy.status_label.to_uppercase())
                                                .small()
                                                .color(tokens.color("glass-text-muted")),
                                        );
                                    });
                                },
                            );
                            for control in &policy.controls {
                                if control.control == Control::Microphone {
                                    // Shipping hides the level meter without a selected
                                    // microphone; the slot stays reserved so controls keep
                                    // their positions on both hosts.
                                    if policy.show_meter {
                                        microphone_meter(ui, tokens, &view);
                                    } else {
                                        ui.add_space(tokens.number("s-9"));
                                    }
                                }
                                let (response, progress) = control_button(ui, control, tokens);
                                if progress > 0. {
                                    tooltip = Some((response.rect, control.clone(), progress));
                                }
                                if response.clicked() {
                                    action = Some(action_for(control.control, &view));
                                }
                            }
                        });
                    });
                })
                .response
                .rect
        })
        .inner;
    if let Some(error) = view.error {
        // Shipping `.recording-hud-error`: 5 px below the card, 10 px insets,
        // one 2xs line in the signal text color with an ellipsis.
        let top = card.bottom() + captures_app::recording_hud::ERROR_GAP as f32;
        let inset = captures_app::recording_hud::ERROR_INSET as f32;
        let rect = Rect::from_min_max(
            egui::pos2(card.left() + inset, top),
            egui::pos2(
                card.right() - inset,
                (top + tokens.number("text-2xs") * 1.6).min(bounds.bottom()),
            ),
        );
        ui.put(
            rect,
            egui::Label::new(
                RichText::new(error)
                    .size(tokens.number("text-2xs"))
                    .color(tokens.color("theme-signal-text")),
            )
            .truncate(),
        );
    }
    if let Some((anchor, control, progress)) = tooltip {
        paint_tooltip(ui, tokens, anchor, &control, progress, bounds);
    }
    action
}

fn action_for(control: Control, view: &View<'_>) -> Action {
    match control {
        Control::Stop => Action::Stop,
        Control::PauseResume if view.state == RecordingState::Paused => Action::Resume,
        Control::PauseResume => Action::Pause,
        Control::Restart => Action::Restart,
        Control::Screenshot => Action::Screenshot,
        Control::Microphone => Action::SetMicrophoneMuted(!view.microphone_muted),
        Control::Delete => Action::Discard,
        Control::Hide => Action::Hide,
    }
}

fn microphone_meter(ui: &mut egui::Ui, tokens: &Tokens, view: &View<'_>) {
    // Clear immediately during lifecycle changes, before a queued sample can arrive.
    let paused = view.state == RecordingState::Paused;
    let peak = if view.state != RecordingState::Recording
        || view.busy
        || !view.has_microphone
        || view.microphone_muted
    {
        0.
    } else {
        view.microphone_peak
    };
    let label = if !view.has_microphone {
        "Microphone level unavailable: no microphone selected".into()
    } else if view.microphone_muted {
        "Microphone muted".into()
    } else if paused {
        "Microphone level: paused".into()
    } else if view.busy || view.state != RecordingState::Recording {
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

/// One HUD button plus its styled tooltip progress (0 hidden … 1 shown).
fn control_button(
    ui: &mut egui::Ui,
    control: &ControlView,
    tokens: &Tokens,
) -> (egui::Response, f32) {
    let signal = control.control == Control::Stop;
    let enabled = control.enabled;
    let selected = control.selected;
    let icon = control.icon.map_or(Icon::Stop, Icon::Shipping);
    let response = ui
        .scope(|ui| {
            let mut button = egui::Button::new("").min_size(Vec2::splat(32.));
            if signal {
                let widgets = &mut ui.visuals_mut().widgets;
                widgets.inactive.weak_bg_fill = tokens.color("theme-signal-surface");
                widgets.inactive.bg_stroke =
                    Stroke::new(1., tokens.color("theme-signal").gamma_multiply(0.4));
                widgets.hovered.weak_bg_fill = tokens.color("theme-signal");
                widgets.active.weak_bg_fill = tokens.color("theme-signal");
                if !enabled {
                    // Shipping dims a disabled Stop (saving, failed) to 32% opacity.
                    button = button
                        .fill(tokens.color("theme-signal-surface").gamma_multiply(0.32))
                        .stroke(Stroke::new(
                            1.,
                            tokens.color("theme-signal").gamma_multiply(0.4 * 0.32),
                        ));
                }
            } else if selected && enabled {
                button = button
                    .fill(tokens.color("glass-active"))
                    .stroke(Stroke::new(
                        1.,
                        tokens.color("theme-accent").gamma_multiply(0.3),
                    ));
            }
            // The styled tooltip replaces egui's delayed hover text.
            ui.add_enabled(enabled, button)
        })
        .inner;
    remember_control(ui, response.rect);
    let label = control.label;
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    if response.has_focus() {
        ui.painter().rect_stroke(
            response.rect,
            tokens.number("r-sm") as u8,
            Stroke::new(2., tokens.color("theme-accent")),
            StrokeKind::Inside,
        );
    }
    let hovered = enabled && (response.hovered() || response.is_pointer_button_down_on());
    let color = tokens.color(if !enabled {
        "glass-text-subtle"
    } else if signal && !hovered {
        "theme-signal"
    } else if selected {
        "theme-accent"
    } else if hovered {
        "glass-text"
    } else {
        "glass-text-muted"
    });
    paint_icon(ui, response.rect, icon, color);
    // Shipping shows the tooltip on hover or keyboard focus, disabled buttons
    // included, with no delay; it fades and slides in over `--dur-1`.
    let showing = ui.rect_contains_pointer(response.rect) || response.has_focus();
    let progress = ui.ctx().animate_bool_with_time(
        response.id.with("tooltip"),
        showing,
        tokens.number("dur-1") / 1000.,
    );
    (response, progress)
}

/// Shipping `.recording-tooltip > [role="tooltip"]`: fixed-glass pill below the
/// button with an xs medium label, placed by the shared policy.
fn paint_tooltip(
    ui: &egui::Ui,
    tokens: &Tokens,
    anchor: Rect,
    control: &ControlView,
    progress: f32,
    bounds: Rect,
) {
    use captures_app::{recording_hud, tray_notice::LogicalRect};
    let logical = |rect: Rect| {
        LogicalRect::new(
            f64::from(rect.left()),
            f64::from(rect.top()),
            f64::from(rect.width()),
            f64::from(rect.height()),
        )
    };
    let color = tokens.color("glass-text").gamma_multiply(progress);
    let font = egui::FontId::proportional(tokens.number("text-xs"));
    let max_text = recording_hud::TOOLTIP_MAX_WIDTH - recording_hud::TOOLTIP_PADDING_X * 2. - 2.;
    let mut job = egui::text::LayoutJob::single_section(
        control.tooltip.to_owned(),
        egui::TextFormat::simple(font, color),
    );
    job.wrap = egui::text::TextWrapping {
        max_width: max_text as f32,
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    let galley = ui.painter().layout_job(job);
    let frame = recording_hud::tooltip_frame(
        logical(anchor),
        f64::from(galley.size().x),
        f64::from(galley.size().y),
        control.tooltip_right_aligned,
        f64::from(progress),
        logical(bounds),
    );
    let rect = Rect::from_min_size(
        egui::pos2(frame.x as f32, frame.y as f32),
        Vec2::new(frame.width as f32, frame.height as f32),
    );
    let painter = ui.ctx().layer_painter(egui::LayerId::new(
        egui::Order::Tooltip,
        egui::Id::unique("recording-hud-tooltip"),
    ));
    painter.rect(
        rect,
        tokens.number("r-sm") as u8,
        tokens.color("glass-strong").gamma_multiply(progress),
        Stroke::new(1., tokens.color("glass-border").gamma_multiply(progress)),
        StrokeKind::Inside,
    );
    let text = egui::pos2(
        rect.left() + (rect.width() - galley.size().x) / 2.,
        rect.top() + (rect.height() - galley.size().y) / 2.,
    );
    painter.galley(text, galley, color);
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

/// Shipping `.recording-hud-privacy`: one subtle 2xs line with **will**/**won’t** emphasized.
fn notice_job(tokens: &Tokens, notice: &str) -> egui::text::LayoutJob {
    let font = egui::FontId::proportional(tokens.number("text-2xs"));
    let format = |token| egui::TextFormat::simple(font.clone(), tokens.color(token));
    let mut job = egui::text::LayoutJob {
        halign: Align::Center,
        ..Default::default()
    };
    let emphasis = ["won’t", "will"].iter().find_map(|word| {
        notice
            .find(&format!(" {word} "))
            .map(|at| (at + 1, word.len()))
    });
    match emphasis {
        Some((start, len)) => {
            job.append(&notice[..start], 0., format("glass-text-subtle"));
            job.append(&notice[start..start + len], 0., format("glass-text"));
            job.append(&notice[start + len..], 0., format("glass-text-subtle"));
        }
        None => job.append(notice, 0., format("glass-text-subtle")),
    }
    job
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

fn paint_status_dot(
    ui: &egui::Ui,
    center: egui::Pos2,
    color: egui::Color32,
    halo: bool,
    phase: f32,
) {
    let scale = 1. - 0.16 * phase;
    let opacity = 1. - 0.4 * phase;
    // Shipping `box-shadow: 0 0 0 4px` halo at 16% of the dot color; the failed
    // dot has none.
    if halo {
        ui.painter().circle_filled(
            center,
            (5. + 4.) * scale,
            color.gamma_multiply(0.16 * opacity),
        );
    }
    ui.painter()
        .circle_filled(center, 5. * scale, color.gamma_multiply(opacity));
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
    fn notice_emphasizes_will_and_wont_like_shipping() {
        let tokens = crate::tokens::load().remove("dark-mustard").unwrap();
        let job = notice_job(
            &tokens,
            "These controls will show in recordings · Use Hide controls to keep them out",
        );
        let colors = |job: &egui::text::LayoutJob| -> Vec<egui::Color32> {
            job.sections
                .iter()
                .map(|section| section.format.color)
                .collect()
        };
        let (subtle, text) = (
            tokens.color("glass-text-subtle"),
            tokens.color("glass-text"),
        );
        assert_eq!(colors(&job), [subtle, text, subtle]);
        let job = notice_job(&tokens, "These controls won’t show in recordings");
        assert_eq!(colors(&job), [subtle, text, subtle]);
        assert_eq!(job.text, "These controls won’t show in recordings");
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
                        state: if paused {
                            RecordingState::Paused
                        } else {
                            RecordingState::Recording
                        },
                        busy,
                        has_microphone,
                        microphone_muted: muted,
                        microphone_peak: peak,
                        elapsed_ms: 0,
                        notice: "",
                        error: None,
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

    fn render(view: View<'_>) -> (Vec<egui::Shape>, Vec<String>) {
        let tokens = crate::tokens::load().remove("dark-mustard").unwrap();
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(430., 102.))),
                ..Default::default()
            },
            |ui| {
                show(ui, &tokens, view);
            },
        );
        output.textures_delta.clear();
        let labels = output
            .platform_output
            .accesskit_update
            .map(|update| {
                update
                    .nodes
                    .iter()
                    .flat_map(|(_, node)| [node.label(), node.value()])
                    .flatten()
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        (
            output.shapes.into_iter().map(|shape| shape.shape).collect(),
            labels,
        )
    }

    fn view(state: RecordingState, error: Option<&str>) -> View<'_> {
        View {
            state,
            busy: false,
            has_microphone: true,
            microphone_muted: false,
            microphone_peak: 0.,
            elapsed_ms: 0,
            notice: "These controls won’t show in recordings",
            error,
            hide_available: true,
            reduced_motion: true,
        }
    }

    #[test]
    fn failed_hud_offers_retry_and_shows_the_inline_error() {
        let (_, labels) = render(view(
            RecordingState::Failed,
            Some("No microphone device is available"),
        ));
        assert!(
            labels.iter().any(|label| label == "Retry recording"),
            "{labels:?}"
        );
        assert!(!labels.iter().any(|label| label == "Restart recording"));
        assert!(
            labels
                .iter()
                .any(|label| label == "No microphone device is available"),
            "{labels:?}"
        );
        let (_, labels) = render(view(RecordingState::Recording, None));
        assert!(labels.iter().any(|label| label == "Restart recording"));
        assert!(
            !labels
                .iter()
                .any(|label| label.contains("microphone device"))
        );
    }

    #[test]
    fn failed_dot_is_subtle_without_a_halo() {
        let tokens = crate::tokens::load().remove("dark-mustard").unwrap();
        let (shapes, _) = render(view(RecordingState::Failed, None));
        let subtle = tokens.color("glass-text-subtle");
        let circles: Vec<_> = shapes
            .iter()
            .filter_map(|shape| match shape {
                egui::Shape::Circle(circle) => Some(circle.fill),
                _ => None,
            })
            .collect();
        assert_eq!(circles, [subtle], "one subtle dot and no 16% halo");
    }
}
