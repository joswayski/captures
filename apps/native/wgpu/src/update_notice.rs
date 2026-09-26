//! Update notice surface: a solid `--surface-raised` card with a tray caret in
//! a transparent window. Copy and state come from `captures_app::update_notice`;
//! placement comes from `captures_app::tray_notice`. The native rewrite has no
//! signed updater yet, so the workbench drives this with the shared stub source.
use std::f32::consts::TAU;

use captures_app::{
    motion::Motion,
    tray_notice::{Caret, LogicalRect, Placement, TRAY_NOTICE_CARET_SIZE as CARET_SIZE},
    update_notice::CARET_SPAN,
    update_notice::{Action, Icon, IconTone, Presentation},
};
use eframe::egui::{self, Color32, CornerRadius, FontId, RichText, Stroke};

use crate::tokens::Tokens;

pub const TITLE: &str = "Captures Update";

fn corner(tokens: &Tokens, name: &str) -> CornerRadius {
    CornerRadius::same(tokens.number(name) as u8)
}

fn rect(rect: LogicalRect) -> egui::Rect {
    egui::Rect::from_min_size(
        egui::pos2(rect.x as f32, rect.y as f32),
        egui::vec2(rect.width as f32, rect.height as f32),
    )
}

fn text(tokens: &Tokens, value: &str, size: &str, color: &str) -> RichText {
    RichText::new(value)
        .font(FontId::proportional(tokens.number(size)))
        .color(tokens.color(color))
}

/// Uppercase caption buttons ("What's new" / "Hide").
fn caption_button(ui: &mut egui::Ui, tokens: &Tokens, label: &str) -> egui::Response {
    let response = ui.add(
        egui::Button::new(text(
            tokens,
            &label.to_uppercase(),
            "text-xs",
            "text-subtle",
        ))
        .frame(false),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    response
}

fn paint_icon(ui: &egui::Ui, tokens: &Tokens, area: egui::Rect, icon: Icon, tone: IconTone) {
    let painter = ui.painter();
    let (fill, ink) = match tone {
        IconTone::Accent => ("theme-accent", "theme-accent-ink"),
        IconTone::Positive => ("positive", "positive-ink"),
        IconTone::Signal => ("theme-signal", "theme-signal-ink"),
        IconTone::Neutral => ("surface-sunken", "text"),
    };
    painter.rect_filled(area, corner(tokens, "r-lg"), tokens.color(fill));
    if tone == IconTone::Neutral {
        painter.rect_stroke(
            area,
            corner(tokens, "r-lg"),
            Stroke::new(1., tokens.color("border")),
            egui::StrokeKind::Inside,
        );
    }
    let ink = tokens.color(ink);
    let stroke = Stroke::new(1.8, ink);
    let c = area.center();
    match icon {
        Icon::App => {
            // Viewfinder corners around a dot: the capture mark.
            for (sx, sy) in [(-1., -1.), (1., -1.), (-1., 1.), (1., 1.)] {
                let corner = c + egui::vec2(7. * sx, 7. * sy);
                painter.line_segment([corner, corner - egui::vec2(4. * sx, 0.)], stroke);
                painter.line_segment([corner, corner - egui::vec2(0., 4. * sy)], stroke);
            }
            painter.circle_filled(c, 2.2, ink);
        }
        Icon::Check => {
            painter.line_segment([c + egui::vec2(-5., 0.), c + egui::vec2(-1., 4.)], stroke);
            painter.line_segment([c + egui::vec2(-1., 4.), c + egui::vec2(6., -4.)], stroke);
        }
        Icon::Warning => {
            let top = c + egui::vec2(0., -7.);
            let left = c + egui::vec2(-7.5, 6.);
            let right = c + egui::vec2(7.5, 6.);
            painter.add(egui::Shape::closed_line(vec![top, right, left], stroke));
            painter.line_segment([c + egui::vec2(0., -2.), c + egui::vec2(0., 1.5)], stroke);
            painter.circle_filled(c + egui::vec2(0., 3.8), 1., ink);
        }
        Icon::Spinner => {
            let start = (ui.input(|input| input.time) as f32 * TAU / 0.72) % TAU;
            let points = (0..=18)
                .map(|step| {
                    let angle = start + step as f32 / 18. * TAU * 0.75;
                    c + egui::vec2(angle.cos(), angle.sin()) * 6.
                })
                .collect();
            painter.add(egui::Shape::line(points, Stroke::new(2., ink)));
            ui.ctx().request_repaint();
        }
    }
}

fn sunken_box(tokens: &Tokens) -> egui::Frame {
    egui::Frame::new()
        .fill(tokens.color("surface-sunken"))
        .stroke(Stroke::new(1., tokens.color("border-subtle")))
        .corner_radius(corner(tokens, "r-lg"))
        .inner_margin(tokens.number("s-5") as i8)
}

fn progress_bar(ui: &mut egui::Ui, tokens: &Tokens, fraction: Option<f32>) {
    let (bar, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 4.), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(bar, CornerRadius::same(2), tokens.color("surface-canvas"));
    let fill = match fraction {
        Some(fraction) => egui::Rect::from_min_size(
            bar.min,
            egui::vec2(bar.width() * fraction.clamp(0., 1.), bar.height()),
        ),
        None => {
            // Indeterminate: a 34% segment sliding across the track.
            let phase = (ui.input(|input| input.time) as f32 / 1.25).fract();
            let x = bar.left() + bar.width() * (phase * 1.35 - 0.35);
            ui.ctx().request_repaint();
            egui::Rect::from_min_size(egui::pos2(x, bar.top()), egui::vec2(bar.width() * 0.34, 4.))
                .intersect(bar)
        }
    };
    painter.rect_filled(fill, CornerRadius::same(2), tokens.color("info"));
}

fn footer_button(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    button: &captures_app::update_notice::Button,
    primary: bool,
) -> bool {
    let (fill, border, ink) = if primary {
        ("theme-accent", "theme-accent", "theme-accent-ink")
    } else {
        ("surface-raised", "control-border", "text-muted")
    };
    let width = if primary { 104. } else { 0. };
    ui.add_enabled(
        button.enabled,
        egui::Button::new(text(tokens, &button.label, "text-sm", ink))
            .fill(tokens.color(fill))
            .stroke(Stroke::new(1., tokens.color(border)))
            .corner_radius(corner(tokens, "r-md"))
            .min_size(egui::vec2(width, tokens.number("h-md"))),
    )
    .clicked()
}

/// Paints the transparent notice window: shadow, card, caret and content.
pub fn show(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    presentation: &Presentation,
    placement: &Placement,
) -> Option<Action> {
    let mut action = None;
    let card = rect(placement.card_rect());
    let radius = corner(tokens, "r-xl");
    let painter = ui.painter().clone();
    painter.add(
        egui::epaint::Shadow {
            offset: [0, 8],
            blur: 20,
            spread: 0,
            color: Color32::from_black_alpha(90),
        }
        .as_shape(card, radius),
    );
    painter.rect_filled(card, radius, tokens.color("surface-raised"));
    painter.rect_stroke(
        card,
        radius,
        Stroke::new(1., tokens.color("border")),
        egui::StrokeKind::Inside,
    );
    let caret_x = placement.caret_x as f32;
    let half = CARET_SPAN as f32 / 2.;
    let size = CARET_SIZE as f32;
    let caret = match placement.caret {
        Caret::Top => Some(vec![
            egui::pos2(caret_x, 0.),
            egui::pos2(caret_x + half, size),
            egui::pos2(caret_x - half, size),
        ]),
        Caret::Bottom => {
            let bottom = placement.height as f32;
            Some(vec![
                egui::pos2(caret_x - half, bottom - size),
                egui::pos2(caret_x + half, bottom - size),
                egui::pos2(caret_x, bottom),
            ])
        }
        Caret::None => None,
    };
    if let Some(points) = caret {
        painter.add(egui::Shape::convex_polygon(
            points,
            tokens.color("surface-raised"),
            Stroke::NONE,
        ));
    }

    let s4 = tokens.number("s-4");
    let s5 = tokens.number("s-5");
    let s6 = tokens.number("s-6");
    let card_ui = &mut ui.new_child(
        egui::UiBuilder::new()
            .max_rect(card)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    card_ui.set_clip_rect(card);
    card_ui.style_mut().spacing.item_spacing = egui::vec2(s4, s4);

    // Header: icon, title/description, optional "What's new".
    let icon = egui::Rect::from_min_size(card.min + egui::vec2(s6, s6), egui::vec2(36., 36.));
    let reveal_width = if presentation.reveal_notes.is_some() {
        96.
    } else {
        0.
    };
    let copy = egui::Rect::from_min_max(
        egui::pos2(icon.right() + s4, card.top() + s6 - 2.),
        egui::pos2(card.right() - s6 - reveal_width, icon.bottom() + 4.),
    );
    paint_icon(
        card_ui,
        tokens,
        icon,
        presentation.icon,
        presentation.icon_tone,
    );
    card_ui.scope_builder(egui::UiBuilder::new().max_rect(copy), |ui| {
        ui.spacing_mut().item_spacing.y = 2.;
        ui.label(text(tokens, &presentation.title, "text-lg", "text").strong());
        ui.label(text(
            tokens,
            &presentation.description,
            "text-sm",
            "text-subtle",
        ));
    });
    if let Some(reveal) = &presentation.reveal_notes {
        let area = egui::Rect::from_min_max(
            egui::pos2(card.right() - s6 - reveal_width, icon.top()),
            egui::pos2(card.right() - s6, icon.bottom()),
        );
        card_ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(area)
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
            |ui| {
                if caption_button(ui, tokens, &reveal.label).clicked() {
                    action = Some(reveal.action.clone());
                }
            },
        );
    }
    let header_bottom = icon.bottom() + s5 + 3.;

    // Footer and warning are anchored to the bottom of the card.
    let footer_height = if presentation.footer.is_some() {
        s4 + tokens.number("h-md") + s6
    } else {
        s4
    };
    let footer_top = card.bottom() - footer_height;
    let warning_top = if let Some(warning) = &presentation.close_warning {
        let text_left = card.left() + s6 + s4 + 16. + tokens.number("s-3");
        let galley = card_ui.painter().layout(
            warning.clone(),
            FontId::proportional(tokens.number("text-sm")),
            tokens.color("caution-text"),
            card.right() - s6 - s4 - text_left,
        );
        let height = galley.size().y + tokens.number("s-3") * 2.;
        let area = egui::Rect::from_min_max(
            egui::pos2(card.left() + s6, footer_top - tokens.number("s-3") - height),
            egui::pos2(card.right() - s6, footer_top - tokens.number("s-3")),
        );
        card_ui.painter().rect(
            area,
            corner(tokens, "r-md"),
            tokens.color("caution-surface"),
            Stroke::new(1., tokens.color("caution-text").gamma_multiply(0.32)),
            egui::StrokeKind::Inside,
        );
        let glyph = egui::Rect::from_min_size(
            area.min + egui::vec2(tokens.number("s-4"), (height - 16.) / 2.),
            egui::vec2(16., 16.),
        );
        let ink = Stroke::new(1.6, tokens.color("caution-text"));
        let c = glyph.center();
        card_ui.painter().add(egui::Shape::closed_line(
            vec![
                c + egui::vec2(0., -6.),
                c + egui::vec2(6.5, 5.),
                c + egui::vec2(-6.5, 5.),
            ],
            ink,
        ));
        card_ui
            .painter()
            .line_segment([c + egui::vec2(0., -2.), c + egui::vec2(0., 1.5)], ink);
        card_ui.painter().galley(
            egui::pos2(text_left, area.top() + tokens.number("s-3")),
            galley,
            tokens.color("caution-text"),
        );
        let response = card_ui.interact(
            area,
            card_ui.scope_id().with("close-warning"),
            egui::Sense::hover(),
        );
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, warning));
        area.top() - tokens.number("s-3")
    } else {
        footer_top
    };

    if let Some(footer) = &presentation.footer {
        let area = egui::Rect::from_min_max(
            egui::pos2(card.left() + s6, footer_top + s4),
            egui::pos2(card.right() - s6, card.bottom() - s6),
        );
        card_ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(area)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
            |ui| {
                if footer_button(ui, tokens, &footer.dismiss, false) {
                    action = Some(footer.dismiss.action.clone());
                }
                if let Some(primary) = &footer.primary {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if footer_button(ui, tokens, primary, true) {
                            action = Some(primary.action.clone());
                        }
                    });
                }
            },
        );
    }

    // Body between the header and the warning/footer.
    let body = egui::Rect::from_min_max(
        egui::pos2(card.left() + s6, header_bottom),
        egui::pos2(card.right() - s6, (warning_top - s4).max(header_bottom)),
    );
    card_ui.scope_builder(egui::UiBuilder::new().max_rect(body), |ui| {
        ui.set_clip_rect(body.intersect(card));
        ui.spacing_mut().item_spacing.y = tokens.number("s-3");
        // Rows are as tall as their text, like the shipping list markup.
        ui.spacing_mut().interact_size.y = 0.;
        if let Some(notes) = &presentation.notes {
            sunken_box(tokens).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.set_min_height(body.height() - s5 * 2. - 2.);
                ui.horizontal(|ui| {
                    ui.label(
                        text(
                            tokens,
                            &notes.heading.to_uppercase(),
                            "text-2xs",
                            "text-subtle",
                        )
                        .strong(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if caption_button(ui, tokens, &notes.hide.label).clicked() {
                            action = Some(notes.hide.action.clone());
                        }
                    });
                });
                if let Some(intro) = &notes.intro {
                    ui.add(egui::Label::new(text(tokens, intro, "text-sm", "text-subtle")).wrap());
                }
                if let Some(empty) = &notes.empty {
                    ui.label(text(tokens, empty, "text-sm", "text-subtle"));
                }
                crate::primitives::scroll_area(
                    ui,
                    tokens,
                    egui::ScrollArea::vertical()
                        .id_salt("update-notes")
                        .auto_shrink([false, false]),
                    |ui| {
                        for (index, group) in notes.groups.iter().enumerate() {
                            if index > 0 {
                                ui.add_space(tokens.number("s-2"));
                                ui.separator();
                            }
                            if notes.stacked {
                                ui.label(
                                    text(tokens, &group.display_version, "text-sm", "text")
                                        .strong(),
                                );
                            }
                            for item in &group.items {
                                ui.horizontal_wrapped(|ui| {
                                    let dot = ui.cursor().min + egui::vec2(2.5, 8.);
                                    ui.painter().circle_filled(
                                        dot,
                                        2.5,
                                        tokens.color("text-faint"),
                                    );
                                    ui.add_space(tokens.number("s-5"));
                                    ui.spacing_mut().item_spacing.x = 4.;
                                    ui.label(text(tokens, &item.text, "text-sm", "text-muted"));
                                    if let Some(pull) = &item.pull_request {
                                        let label = format!("#{}", pull.number);
                                        let response = ui.add(
                                            egui::Button::new(
                                                text(tokens, &label, "text-sm", "text").underline(),
                                            )
                                            .frame(false),
                                        );
                                        let name = format!("Open pull request {}", pull.number);
                                        response.widget_info(|| {
                                            egui::WidgetInfo::labeled(
                                                egui::WidgetType::Link,
                                                true,
                                                &name,
                                            )
                                        });
                                        if response.clicked() {
                                            action = Some(Action::OpenPullRequest {
                                                url: pull.url.clone(),
                                            });
                                        }
                                    }
                                });
                            }
                        }
                    },
                );
            });
        }
        if let Some(download) = &presentation.download {
            sunken_box(tokens).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(text(tokens, &download.label, "text-sm", "text-subtle"));
                    ui.label(text(tokens, &download.detail, "text-sm", "text-faint"));
                    if let Some(percent) = &download.percent_label {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(text(tokens, percent, "text-sm", "text").strong());
                        });
                    }
                });
                progress_bar(
                    ui,
                    tokens,
                    download.percent.map(|percent| f32::from(percent) / 100.),
                );
            });
        }
        if let Some(restart) = &presentation.restart {
            sunken_box(tokens).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(text(tokens, &restart.message, "text-sm", "text-subtle"));
                progress_bar(ui, tokens, Some(f32::from(restart.seconds_remaining) / 3.));
            });
        }
        if let Some(error) = &presentation.error {
            egui::Frame::new()
                .fill(tokens.color("danger-surface"))
                .stroke(Stroke::new(1., tokens.color("danger-border")))
                .corner_radius(corner(tokens, "r-lg"))
                .inner_margin(s5 as i8)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.add(
                        egui::Label::new(text(tokens, &error.message, "text-sm", "danger-text"))
                            .wrap(),
                    );
                });
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 0.;
                ui.label(text(
                    tokens,
                    &error.fallback_prefix,
                    "text-sm",
                    "text-subtle",
                ));
                let response = ui.add(
                    egui::Button::new(
                        text(tokens, &error.fallback_link, "text-sm", "text").underline(),
                    )
                    .frame(false),
                );
                if response.clicked() {
                    action = Some(Action::OpenDownloadPage);
                }
                ui.label(text(
                    tokens,
                    &error.fallback_suffix,
                    "text-sm",
                    "text-subtle",
                ));
            });
        }
        if let Some(message) = &presentation.status_message {
            ui.label(text(tokens, message, "text-sm", "text-subtle"));
        }
    });
    action
}

/// Where the workbench pretends the tray icon is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FixtureTray {
    Top,
    Bottom,
    None,
}

impl FixtureTray {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "top" => Some(Self::Top),
            "bottom" => Some(Self::Bottom),
            "none" => Some(Self::None),
            _ => None,
        }
    }

    fn rect(self, monitor: LogicalRect) -> Option<LogicalRect> {
        let x = monitor.x + monitor.width - 180.;
        match self {
            Self::Top => Some(LogicalRect {
                x,
                y: monitor.y,
                width: 24.,
                height: 24.,
            }),
            Self::Bottom => Some(LogicalRect {
                x,
                y: monitor.y + monitor.height - 36.,
                width: 24.,
                height: 36.,
            }),
            Self::None => None,
        }
    }
}

/// Workbench host for the notice, driven by the shared stub status source.
/// It never downloads, installs, relaunches or opens URLs; link actions are
/// reported as events instead.
pub struct FixtureHost {
    fixture: String,
    tray: FixtureTray,
    status: Option<captures_app::update_notice::UpdateStatus>,
    visible: bool,
    view: captures_app::update_notice::ViewState,
    simulating: bool,
    next_tick: Option<std::time::Instant>,
    synced_setting: bool,
    last_report: Option<serde_json::Value>,
    /// Start of the shipping `ui-pop-in` entrance for the visible notice.
    shown_at: Option<std::time::Instant>,
    /// Start of the restart state; shipping's `update-restart-exit` fade is
    /// timed from it (3 s delay).
    restart_at: Option<std::time::Instant>,
    /// The Updated card kept on screen after the stub "restart" so its exit
    /// fade can finish.
    leaving: Option<(Presentation, Placement)>,
    tx: std::sync::mpsc::Sender<Action>,
    rx: std::sync::mpsc::Receiver<Action>,
}

impl FixtureHost {
    pub fn new(fixture: &str, tray: FixtureTray) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut host = Self {
            fixture: String::new(),
            tray,
            status: None,
            visible: false,
            view: captures_app::update_notice::ViewState {
                show_changelog: true,
                ..Default::default()
            },
            simulating: false,
            next_tick: None,
            synced_setting: false,
            last_report: None,
            shown_at: None,
            restart_at: None,
            leaving: None,
            tx,
            rx,
        };
        host.select(fixture);
        host
    }

    pub fn select(&mut self, fixture: &str) {
        self.fixture = fixture.to_owned();
        self.status = captures_app::update_notice::fixture(fixture);
        self.visible = self.status.is_some();
        self.view.action_error = None;
        self.view.installing = false;
        self.simulating = false;
        self.next_tick = None;
        self.shown_at = None;
        self.restart_at = None;
        self.leaving = None;
    }

    /// Root-window controls for switching fixture statuses.
    pub fn controls(&mut self, ui: &mut egui::Ui, tokens: &Tokens) {
        ui.label(text(
            tokens,
            "Stub status source: no updater, download, install or relaunch is connected.",
            "text-sm",
            "text-muted",
        ));
        ui.add_space(tokens.number("s-4"));
        ui.horizontal_wrapped(|ui| {
            for name in captures_app::update_notice::FIXTURES {
                if ui
                    .add(egui::Button::new(name).selected(self.visible && self.fixture == name))
                    .clicked()
                {
                    self.select(name);
                }
            }
        });
    }

    fn apply(
        &mut self,
        action: Action,
        blocked: bool,
        preferences: &mut crate::preferences::Preferences,
    ) {
        use captures_app::update_notice::{StubEvent, UpdateStatus, stub_next};
        crate::emit("update-notice-action", serde_json::json!(action));
        match action {
            Action::Dismiss if blocked => {
                crate::emit("update-notice-dismiss-blocked", serde_json::json!({}));
            }
            Action::Dismiss => {
                self.visible = false;
                self.shown_at = None;
                self.simulating = false;
                self.next_tick = None;
                crate::emit("update-notice-dismissed", serde_json::json!({}));
            }
            Action::ShowNotes | Action::HideNotes => {
                let show = action == Action::ShowNotes;
                self.view.show_changelog = show;
                preferences.set_show_update_changelog(show);
            }
            Action::Install | Action::Check => {
                let Some(status) = &self.status else { return };
                if matches!(
                    status,
                    UpdateStatus::Available {
                        installable: false,
                        ..
                    }
                ) {
                    crate::emit(
                        "update-notice-open-url",
                        serde_json::json!({"kind": "release", "opened": false}),
                    );
                    return;
                }
                let event = if action == Action::Install {
                    StubEvent::Install
                } else {
                    StubEvent::Check
                };
                self.view.action_error = None;
                self.status = stub_next(status, event);
                self.simulating = true;
                self.next_tick = None;
            }
            Action::OpenPullRequest { url } => crate::emit(
                "update-notice-open-url",
                serde_json::json!({"kind": "pull_request", "url": url, "opened": false}),
            ),
            Action::OpenDownloadPage => crate::emit(
                "update-notice-open-url",
                serde_json::json!({"kind": "download", "url": captures_app::update_notice::DOWNLOAD_PAGE_URL, "opened": false}),
            ),
        }
    }

    fn tick(&mut self, ctx: &egui::Context, placement: Option<Placement>) {
        use captures_app::update_notice::{StubEvent, stub_next, stub_tick_interval_ms};
        if !self.simulating || !self.visible {
            return;
        }
        let Some(status) = &self.status else { return };
        let Some(interval) = stub_tick_interval_ms(status) else {
            self.next_tick = None;
            return;
        };
        let now = std::time::Instant::now();
        let due = *self
            .next_tick
            .get_or_insert(now + std::time::Duration::from_millis(interval));
        if now < due {
            ctx.request_repaint_after(due - now);
            return;
        }
        let before = self.presentation();
        self.status = stub_next(status, StubEvent::Tick);
        self.next_tick = None;
        if self.status.is_none() {
            // The stub "restart" finished; nothing is relaunched.
            self.visible = false;
            self.shown_at = None;
            self.leaving = placement.map(|placement| (before, placement));
            crate::emit("update-notice-restart-simulated", serde_json::json!({}));
        }
        ctx.request_repaint();
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        tokens: &Tokens,
        preferences: &mut crate::preferences::Preferences,
        reduced_motion: bool,
    ) {
        if !self.synced_setting && !preferences.is_loading() {
            self.synced_setting = true;
            if let Ok(settings) = preferences.snapshot() {
                self.view.show_changelog = settings.show_update_changelog;
            }
        }
        let blocked = self.presentation().dismiss_blocked;
        while let Ok(action) = self.rx.try_recv() {
            self.apply(action, blocked, preferences);
        }
        let placement = self
            .visible
            .then(|| self.placement(ctx, &self.presentation()));
        self.tick(ctx, placement);
        let now = std::time::Instant::now();
        let restart_exit = tokens.motion(Motion::UpdateNoticeRestartExit);
        if !self.visible {
            self.report(None);
            // Shipping fades the Updated card out 3 s into the restart state.
            let Some((presentation, placement)) = self.leaving.clone() else {
                return;
            };
            let elapsed = self
                .restart_at
                .map_or(f64::INFINITY, |at| crate::motion::elapsed_ms(at, now));
            if elapsed >= restart_exit.total_ms(reduced_motion) {
                self.leaving = None;
                self.restart_at = None;
                return;
            }
            let pose = restart_exit.pose_at(elapsed, reduced_motion);
            self.paint(ctx, tokens, presentation, placement, pose, false);
            ctx.request_repaint();
            return;
        }
        let presentation = self.presentation();
        let placement = self.placement(ctx, &presentation);
        self.report(Some((&presentation, &placement)));
        let shown_at = *self.shown_at.get_or_insert(now);
        let pose = if presentation.restart.is_some() {
            let started = *self.restart_at.get_or_insert(now);
            let elapsed = crate::motion::elapsed_ms(started, now);
            if restart_exit.running(elapsed, reduced_motion) {
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(
                    ((restart_exit.delay_ms - elapsed).max(0.) / 1000.).max(1. / 120.),
                ));
            }
            restart_exit.pose_at(elapsed, reduced_motion)
        } else {
            self.restart_at = None;
            let pop_in = tokens.motion(Motion::UpdateNoticeIn);
            let elapsed = crate::motion::elapsed_ms(shown_at, now);
            if pop_in.running(elapsed, reduced_motion) {
                ctx.request_repaint();
            }
            pop_in.pose_at(elapsed, reduced_motion)
        };
        self.paint(ctx, tokens, presentation, placement, pose, true);
    }

    fn placement(&self, ctx: &egui::Context, presentation: &Presentation) -> Placement {
        let size = ctx
            .input(|input| input.viewport().monitor_size)
            .unwrap_or(egui::vec2(1440., 900.));
        let monitor = LogicalRect {
            x: 0.,
            y: 0.,
            width: f64::from(size.x),
            height: f64::from(size.y),
        };
        captures_app::update_notice::placement(
            monitor,
            monitor,
            self.tray.rect(monitor),
            cfg!(target_os = "macos"),
            presentation.card_width,
            presentation.card_height,
        )
    }

    /// Paint the transparent notice window. `interactive` is false for the
    /// exit fade after the stub restart, which accepts no actions.
    fn paint(
        &self,
        ctx: &egui::Context,
        tokens: &Tokens,
        presentation: Presentation,
        placement: Placement,
        pose: captures_app::motion::Pose,
        interactive: bool,
    ) {
        let viewport = egui::ViewportId::from_hash_of("update-notice");
        let window = egui::vec2(placement.width as f32, placement.height as f32);
        let builder = egui::ViewportBuilder::default()
            .with_title(TITLE)
            .with_position(egui::pos2(placement.x as f32, placement.y as f32))
            .with_inner_size(window)
            .with_min_inner_size(window)
            .with_max_inner_size(window)
            .with_decorations(false)
            .with_resizable(false)
            .with_transparent(true)
            .with_has_shadow(false)
            .with_always_on_top()
            .with_taskbar(false)
            .with_active(false);
        let tokens = tokens.clone();
        let sender = self.tx.clone();
        ctx.show_viewport_deferred(viewport, builder, move |ui, _| {
            let ctx = ui.ctx().clone();
            let escape = ui.input(|input| input.key_pressed(egui::Key::Escape));
            let close = ui.input(|input| input.viewport().close_requested());
            if close && (presentation.dismiss_blocked || !interactive) {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            }
            let card = rect(placement.card_rect());
            let action = crate::motion::with_pose(ui, pose, card, |ui| {
                show(ui, &tokens, &presentation, &placement)
            })
            .or_else(|| (escape || close).then_some(Action::Dismiss))
            .filter(|_| interactive);
            if let Some(action) = action {
                let _ = sender.send(action);
                ctx.request_repaint_of(egui::ViewportId::ROOT);
            }
        });
        ctx.request_repaint_of(viewport);
    }

    fn presentation(&self) -> Presentation {
        captures_app::update_notice::present(self.status.as_ref(), &self.view)
    }

    /// One event per rendered state so fixtures can assert shared copy.
    fn report(&mut self, shown: Option<(&Presentation, &Placement)>) {
        let report = shown.map(|(p, placement)| {
            serde_json::json!({
                "visualState": p.visual_state,
                "title": p.title,
                "description": p.description,
                "notes": p.notes.as_ref().map(|notes| serde_json::json!({
                    "stacked": notes.stacked,
                    "groups": notes.groups.iter().map(|g| &g.display_version).collect::<Vec<_>>(),
                    "items": notes.groups.iter().map(|g| g.items.len()).sum::<usize>(),
                })),
                "revealNotes": p.reveal_notes.is_some(),
                "download": p.download.as_ref().map(|d| (&d.detail, d.percent)),
                "restart": p.restart.as_ref().map(|r| &r.message),
                "error": p.error.as_ref().map(|e| &e.message),
                "statusMessage": p.status_message,
                "closeWarning": p.close_warning,
                "footer": p.footer.as_ref().map(|f| serde_json::json!({
                    "dismiss": f.dismiss.label,
                    "primary": f.primary.as_ref().map(|b| &b.label),
                    "enabled": f.dismiss.enabled,
                })),
                "dismissBlocked": p.dismiss_blocked,
                "cardHeight": p.card_height,
                "window": [placement.x, placement.y, placement.width, placement.height],
                "caret": placement.caret,
                "caretX": placement.caret_x,
            })
        });
        let report = report.unwrap_or(serde_json::Value::Null);
        if self.last_report.as_ref() != Some(&report) {
            crate::emit("update-notice", report.clone());
            self.last_report = Some(report);
        }
    }
}
