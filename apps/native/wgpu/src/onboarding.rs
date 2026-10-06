//! First-run setup and permission recovery, styled like the shipping Tauri
//! setup window (`Onboarding.tsx` + `windows.css`). Copy and per-state
//! decisions come from `captures_app::onboarding`, shared with AppKit.
use captures_app::motion::{Motion, ONBOARDING_CTA_SPREAD};
use captures_app::onboarding::{self as shared, Presentation, Status};
use eframe::egui::{
    self, Align, FontId, Layout, Pos2, Rect, RichText, Stroke, Vec2, accesskit::Role,
};

use crate::{accessibility, tokens::Tokens};

pub const WAYLAND_NOTE: &str = "Wayland screenshots and display recording use your desktop portal. Native region/window selection is not available yet.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Screen,
    Microphone,
}

/// Which request is in flight, for "Opening…" labels and disabled controls.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Busy {
    pub any: bool,
    pub opening: Option<Target>,
}

impl Busy {
    pub fn from_action(action: Option<shared::Action>) -> Self {
        Self {
            any: action.is_some(),
            opening: match action {
                Some(shared::Action::RequestScreen) => Some(Target::Screen),
                Some(shared::Action::RequestMicrophone) => Some(Target::Microphone),
                _ => None,
            },
        }
    }
}

const STAGE_WIDTH: f32 = 620.;
const ICON: f32 = 22.;
const ACTIONS_WIDTH: f32 = 132.;

/// Title and lede, left-aligned like `.onboarding-copy`. First-run setup
/// (`welcome`) adds the app mark and the "Welcome to Captures" eyebrow;
/// permission recovery uses the plain heading. A short setup window omits
/// the lede (`None`), as shipping's `@media (max-height: 600px)` does.
pub fn header(ui: &mut egui::Ui, t: &Tokens, title: &str, lede: Option<&str>, welcome: bool) {
    // `<header aria-labelledby="onboarding-setup-title">`; the mark is
    // `aria-hidden` (it has no AccessKit node).
    ui.scope(|ui| {
        accessibility::set_group(ui, Role::Group, title);
        header_contents(ui, t, title, lede, welcome);
    });
}

fn header_contents(ui: &mut egui::Ui, t: &Tokens, title: &str, lede: Option<&str>, welcome: bool) {
    if welcome {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(40.), egui::Sense::hover());
        app_mark(ui.painter(), t, rect);
        ui.add_space(t.number("s-2"));
        ui.label(
            RichText::new(shared::EYEBROW.to_uppercase())
                .size(t.number("text-2xs"))
                .color(t.color("text-subtle"))
                .extra_letter_spacing(0.6)
                .strong(),
        );
        ui.add_space(-t.number("s-2"));
    }
    let heading = ui.label(
        RichText::new(title)
            .size(t.number("text-2xl"))
            .color(t.color("text"))
            .strong(),
    );
    accessibility::heading(&heading, 1, title);
    if let Some(lede) = lede {
        ui.add_space(-t.number("s-2"));
        ui.label(
            RichText::new(lede)
                .size(t.number("text-md"))
                .color(t.color("text-subtle")),
        );
    }
}

/// Shared permission cards (`.onboarding-permissions`). `view` is `None`
/// while the first check is in flight. Returns the action the user chose.
pub fn cards(
    ui: &mut egui::Ui,
    t: &Tokens,
    view: Option<&Presentation>,
    busy: Busy,
) -> Option<Target> {
    let mut clicked = None;
    egui::Frame::new()
        .fill(t.color("surface-raised"))
        .stroke(Stroke::new(1., t.color("border-subtle")))
        .corner_radius(t.number("r-xl") as u8)
        .show(ui, |ui| {
            // `.onboarding-permissions` is `aria-live="polite"`.
            accessibility::set_live(ui);
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.;
            let screen = card(
                ui,
                t,
                Glyph::Screen,
                shared::SCREEN_TITLE,
                false,
                view.map_or(shared::CHECKING, |view| view.screen_description),
                view.and_then(|view| view.screen_status),
                view.and_then(|view| view.screen_action),
                busy,
                busy.opening == Some(Target::Screen),
            );
            if screen {
                clicked = Some(Target::Screen);
            }
            if let Some(view) = view.filter(|view| view.show_microphone) {
                let y = ui.cursor().top();
                let x = ui.max_rect().x_range();
                ui.painter()
                    .hline(x, y, Stroke::new(1., t.color("border-subtle")));
                let microphone = card(
                    ui,
                    t,
                    Glyph::Microphone,
                    shared::MICROPHONE_TITLE,
                    true,
                    view.microphone_description,
                    view.microphone_status,
                    view.microphone_action,
                    busy,
                    busy.opening == Some(Target::Microphone),
                );
                if microphone {
                    clicked = Some(Target::Microphone);
                }
            }
        });
    clicked
}

#[derive(Clone, Copy)]
enum Glyph {
    Screen,
    Microphone,
}

#[allow(clippy::too_many_arguments)]
fn card(
    ui: &mut egui::Ui,
    t: &Tokens,
    glyph: Glyph,
    title: &str,
    optional: bool,
    description: &str,
    status: Option<Status>,
    action: Option<&str>,
    busy: Busy,
    opening: bool,
) -> bool {
    let mut clicked = false;
    let pad = t.number("s-6");
    egui::Frame::new().inner_margin(pad as i8).show(ui, |ui| {
        // `<article>` named by its `<h3>`.
        accessibility::set_group(ui, Role::Article, title);
        ui.set_width(ui.available_width());
        let gap = t.number("s-5");
        let copy_width = (ui.available_width() - ICON - ACTIONS_WIDTH - 2. * gap).max(120.);
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 0.;
            let (icon, _) = ui.allocate_exact_size(Vec2::splat(ICON), egui::Sense::hover());
            permission_glyph(ui.painter(), t, icon.translate(Vec2::new(0., 1.)), glyph);
            ui.add_space(gap);
            ui.allocate_ui_with_layout(
                Vec2::new(copy_width, 0.),
                Layout::top_down(Align::Min),
                |ui| {
                    ui.set_width(copy_width);
                    ui.spacing_mut().item_spacing.y = t.number("s-2");
                    ui.spacing_mut().interact_size.y = 0.;
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = t.number("s-4");
                        let heading = ui.label(
                            RichText::new(title)
                                .size(t.number("text-md"))
                                .color(t.color("text"))
                                .strong(),
                        );
                        accessibility::heading(&heading, 3, title);
                        if optional {
                            ui.label(
                                RichText::new(shared::OPTIONAL)
                                    .size(t.number("text-xs"))
                                    .color(t.color("text-faint")),
                            );
                        }
                    });
                    ui.add(
                        egui::Label::new(
                            RichText::new(description)
                                .size(t.number("text-sm"))
                                .color(t.color("text-subtle")),
                        )
                        .wrap(),
                    );
                },
            );
            ui.add_space(gap);
            ui.allocate_ui_with_layout(
                Vec2::new(ui.available_width(), 0.),
                Layout::top_down(Align::Max),
                |ui| {
                    ui.spacing_mut().item_spacing.y = t.number("s-3");
                    if let Some(status) = status {
                        status_pill(ui, t, status);
                    }
                    if let Some(action) = action {
                        let label = if opening { shared::OPENING } else { action };
                        let button =
                            egui::Button::new(RichText::new(label).size(t.number("text-sm")))
                                .min_size(Vec2::new(0., t.number("h-md")));
                        clicked = ui.add_enabled(!busy.any, button).clicked();
                    }
                },
            );
        });
    });
    clicked
}

/// `.onboarding-permission-status`: "Granted ✓" / "Ready ✓" in positive
/// text; "Restart required" / "Still off" as neutral hints.
fn status_pill(ui: &mut egui::Ui, t: &Tokens, status: Status) {
    let color = if status.ready {
        t.color("positive-text")
    } else {
        t.color("text-subtle")
    };
    let font = FontId::proportional(t.number("text-sm"));
    let galley = ui
        .painter()
        .layout_no_wrap(status.label.to_owned(), font, color);
    let check = if status.ready {
        14. + t.number("s-2")
    } else {
        0.
    };
    let size = Vec2::new(galley.size().x + check, galley.size().y);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
    let text_pos = Pos2::new(rect.left(), rect.center().y - galley.size().y / 2.);
    ui.painter().galley(text_pos, galley, color);
    if status.ready {
        let c = Pos2::new(rect.right() - 7., rect.center().y);
        let points = [
            c + Vec2::new(-4.4, 0.2),
            c + Vec2::new(-1.6, 3.4),
            c + Vec2::new(4.8, -3.4),
        ];
        ui.painter().line(points.to_vec(), Stroke::new(2., color));
    }
    // Shipping's check mark is `aria-hidden`, so the status reads as its text.
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, status.label));
}

/// Neutral high-contrast action (`.onboarding-primary-button`). With
/// `emphasis` the ready button plays shipping's `onboarding-cta-pulse`: a
/// `--surface-active` halo that swells to 5 pt and fades, which rests
/// invisible under reduced motion.
pub fn primary_button(
    ui: &mut egui::Ui,
    t: &Tokens,
    label: &str,
    enabled: bool,
    emphasis: bool,
) -> bool {
    let button = egui::Button::new(
        RichText::new(label)
            .size(t.number("text-md"))
            .color(t.color("solid-ink"))
            .strong(),
    )
    .fill(t.color("solid"))
    .stroke(Stroke::NONE)
    .corner_radius(t.number("r-md") as u8)
    .min_size(Vec2::new(148., t.number("h-xl")));
    let response = ui.add_enabled(enabled, button);
    let since_id = response.id.with("cta-pulse-since");
    if enabled && emphasis {
        let now = ui.input(|input| input.time);
        let since = ui.data_mut(|data| *data.get_temp_mut_or_insert_with(since_id, || now));
        let reduced = crate::motion::reduced(ui.ctx());
        let pose = t
            .motion(Motion::OnboardingCtaPulse)
            .pose_repeating((now - since) * 1000., reduced);
        let spread = (ONBOARDING_CTA_SPREAD * pose.opacity) as f32;
        if spread > 0.01 {
            // `box-shadow: 0 0 0 <spread> <colour>` sits flush outside the button.
            ui.painter().rect_stroke(
                response.rect,
                t.number("r-md"),
                Stroke::new(
                    spread,
                    t.color("surface-active")
                        .gamma_multiply(pose.opacity as f32),
                ),
                egui::StrokeKind::Outside,
            );
        }
        if !reduced {
            ui.ctx().request_repaint();
        }
    } else {
        ui.data_mut(|data| data.remove::<f64>(since_id));
    }
    response.clicked()
}

pub fn secondary_button(ui: &mut egui::Ui, t: &Tokens, label: &str, enabled: bool) -> bool {
    let button = egui::Button::new(RichText::new(label).size(t.number("text-md")))
        .min_size(Vec2::new(0., t.number("h-xl")));
    ui.add_enabled(enabled, button).clicked()
}

/// `.onboarding-error`: danger surface with border, full width.
pub fn error_block(ui: &mut egui::Ui, t: &Tokens, lines: &[&str]) {
    egui::Frame::new()
        .fill(t.color("danger-surface"))
        .stroke(Stroke::new(1., t.color("danger-border")))
        .corner_radius(t.number("r-md") as u8)
        .inner_margin(egui::Margin::symmetric(
            t.number("s-5") as i8,
            t.number("s-4") as i8,
        ))
        .show(ui, |ui| {
            // `.onboarding-error` is `role="alert"`.
            accessibility::set_role(ui, Role::Alert);
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = t.number("s-2");
            for line in lines {
                ui.add(
                    egui::Label::new(
                        RichText::new(*line)
                            .size(t.number("text-sm"))
                            .color(t.color("danger-text")),
                    )
                    .wrap(),
                );
            }
        });
}

/// Neutral platform capability note.
pub fn note(ui: &mut egui::Ui, t: &Tokens, text: &str) {
    ui.add(
        egui::Label::new(
            RichText::new(text)
                .size(t.number("text-sm"))
                .color(t.color("theme-signal")),
        )
        .wrap(),
    );
}

/// Centered stage of at most 620 px (`.onboarding-stage`), vertically
/// centered using the previous frame's measured height.
pub fn stage(ui: &mut egui::Ui, t: &Tokens, short: bool, add: impl FnOnce(&mut egui::Ui)) {
    let outer = ui.max_rect();
    let id = egui::Id::unique("onboarding-stage-height");
    // `padding: var(--s-9) var(--s-8)`, or `var(--s-6) var(--s-7)` when short.
    let padding = if short {
        Vec2::new(t.number("s-7"), t.number("s-6"))
    } else {
        Vec2::new(t.number("s-8"), t.number("s-9"))
    };
    let width = (outer.width() - 2. * padding.x).clamp(0., STAGE_WIDTH);
    let previous: f32 = ui.data(|data| data.get_temp(id)).unwrap_or(0.);
    let top = ((outer.height() - previous) / 2.).max(padding.y);
    let rect = Rect::from_min_size(
        Pos2::new(outer.center().x - width / 2., outer.top() + top),
        Vec2::new(width, (outer.height() - top - padding.y).max(0.)),
    );
    let response = ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(Layout::top_down(Align::Min)),
        |ui| {
            ui.set_width(width);
            add(ui);
        },
    );
    let height = response.response.rect.height();
    if (height - previous).abs() > 0.5 {
        ui.data_mut(|data| data.insert_temp(id, height));
        ui.ctx().request_repaint();
    }
}

/// `.onboarding-actions`: right-aligned row, primary last.
pub fn actions(ui: &mut egui::Ui, t: &Tokens, add: impl FnOnce(&mut egui::Ui)) {
    // Leave room for the primary button's outside halo.
    let size = Vec2::new(ui.available_width(), t.number("h-xl") + 8.);
    ui.allocate_ui_with_layout(size, Layout::right_to_left(Align::Center), |ui| {
        ui.add_space(4.);
        ui.spacing_mut().item_spacing.x = t.number("s-4");
        add(ui);
    });
}

/// 40 px accent tile with capture corners and a spark (`AppMarkIcon`).
fn app_mark(painter: &egui::Painter, t: &Tokens, rect: Rect) {
    painter.rect_filled(rect, t.number("r-lg"), t.color("theme-accent"));
    let ink = t.color("theme-accent-ink");
    let stroke = Stroke::new(1.7, ink);
    // The 24-unit icon grid drawn 1:1, centered in the tile.
    let origin = rect.center() - Vec2::splat(12.);
    let p = |x: f32, y: f32| origin + Vec2::new(x, y);
    for (a, b, c) in [
        ((9., 4.), (4., 4.), (4., 9.)),
        ((15., 4.), (20., 4.), (20., 9.)),
        ((20., 15.), (20., 20.), (15., 20.)),
        ((9., 20.), (4., 20.), (4., 15.)),
    ] {
        painter.line(vec![p(a.0, a.1), p(b.0, b.1), p(c.0, c.1)], stroke);
    }
    let center = p(12., 12.);
    let spark = [
        center + Vec2::new(0., -3.5),
        center + Vec2::new(0.9, -0.9),
        center + Vec2::new(3.5, 0.),
        center + Vec2::new(0.9, 0.9),
        center + Vec2::new(0., 3.5),
        center + Vec2::new(-0.9, 0.9),
        center + Vec2::new(-3.5, 0.),
        center + Vec2::new(-0.9, -0.9),
    ];
    painter.add(egui::Shape::convex_polygon(
        spark.to_vec(),
        ink,
        Stroke::NONE,
    ));
}

/// Monitor and microphone outlines in `--text-subtle` (20 px in a 22 px box).
fn permission_glyph(painter: &egui::Painter, t: &Tokens, rect: Rect, glyph: Glyph) {
    let color = t.color("text-subtle");
    let stroke = Stroke::new(1.7, color);
    let scale = 20. / 24.;
    let origin = rect.center() - Vec2::splat(12. * scale);
    let p = |x: f32, y: f32| origin + Vec2::new(x, y) * scale;
    match glyph {
        Glyph::Screen => {
            painter.rect_stroke(
                Rect::from_min_max(p(3., 4.), p(21., 17.)),
                2.5 * scale,
                stroke,
                egui::StrokeKind::Middle,
            );
            painter.line_segment([p(8., 21.), p(16., 21.)], stroke);
            painter.line_segment([p(12., 17.), p(12., 21.)], stroke);
        }
        Glyph::Microphone => {
            painter.rect_stroke(
                Rect::from_min_max(p(9., 3.), p(15., 14.)),
                3. * scale,
                stroke,
                egui::StrokeKind::Middle,
            );
            let arc: Vec<Pos2> = (0..=12)
                .map(|step| {
                    let angle = std::f32::consts::PI * step as f32 / 12.;
                    p(12. + 6. * angle.cos(), 11. + 6. * angle.sin())
                })
                .collect();
            painter.line(arc, stroke);
            painter.line_segment([p(12., 17.), p(12., 21.)], stroke);
            painter.line_segment([p(9., 21.), p(15., 21.)], stroke);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(granted: bool) -> shared::State {
        shared::State {
            platform: "macos".into(),
            onboarding_completed: false,
            screen_recording_required: true,
            screen_recording_granted: granted,
            screen_recording_can_request: false,
            screen_recording_requested_this_launch: true,
            microphone_granted: false,
            microphone_can_request: true,
            microphone_requested_this_launch: false,
        }
    }

    fn render(view: &Presentation, error: bool) -> egui::accesskit::TreeUpdate {
        let ctx = egui::Context::default();
        let t = crate::tokens::load().remove("light-mustard").unwrap();
        crate::accessibility::tests::tree(&ctx, Vec2::new(800., 600.), vec![], |ui| {
            header(ui, &t, view.title, Some(shared::LEDE), true);
            cards(ui, &t, Some(view), Busy::default());
            if error {
                error_block(ui, &t, &["Setup could not continue: denied"]);
            }
        })
    }

    #[test]
    fn cards_statuses_and_actions_expose_shipping_names_and_roles() {
        use crate::accessibility::tests::{contains, find, find_role, find_value};
        use egui::accesskit::{Live, Role};
        let tree = render(&state(false).presentation(), true);
        let (header, _) =
            find_role(&tree, Role::Group, "Required permissions").expect("aria-labelledby header");
        let heading = tree
            .nodes
            .iter()
            .find(|(_, node)| node.role() == Role::Heading && node.level() == Some(1))
            .expect("h1");
        assert_eq!(heading.1.label(), Some("Required permissions"));
        assert!(contains(&tree, header, heading.0));

        let (screen, _) =
            find_role(&tree, Role::Article, shared::SCREEN_TITLE).expect("screen card");
        let (live, _) = tree
            .nodes
            .iter()
            .find(|(_, node)| node.live() == Some(Live::Polite))
            .expect("aria-live region");
        assert!(contains(&tree, *live, screen));
        let (status, node) = find_value(&tree, "Restart required").expect("status");
        assert_eq!(node.role(), Role::Label);
        assert!(contains(&tree, screen, status));
        let (action, node) = find(&tree, "Open Settings").expect("screen action");
        assert_eq!(node.role(), Role::Button);
        assert!(contains(&tree, screen, action));
        assert!(
            tree.nodes
                .iter()
                .any(|(id, node)| node.role() == Role::Heading
                    && node.level() == Some(3)
                    && node.label() == Some(shared::SCREEN_TITLE)
                    && contains(&tree, screen, *id))
        );

        let (microphone, _) =
            find_role(&tree, Role::Article, shared::MICROPHONE_TITLE).expect("microphone card");
        let (allow, _) = find(&tree, "Allow microphone").expect("microphone action");
        assert!(contains(&tree, microphone, allow));

        let (alert, _) = tree
            .nodes
            .iter()
            .find(|(_, node)| node.role() == Role::Alert)
            .expect("role=alert");
        let (text, _) = find_value(&tree, "Setup could not continue: denied").expect("error text");
        assert!(contains(&tree, *alert, text));

        // The check mark is aria-hidden: the granted status reads as its text.
        let ready = render(&state(true).presentation(), false);
        let (screen, _) =
            find_role(&ready, Role::Article, shared::SCREEN_TITLE).expect("screen card");
        let (granted, _) = find_value(&ready, "Granted").expect("granted status");
        assert!(contains(&ready, screen, granted));
        assert!(
            !ready
                .nodes
                .iter()
                .any(|(_, node)| node.role() == Role::Alert)
        );
    }

    #[test]
    fn ready_start_pulses_its_halo_and_rests_without_it_under_reduced_motion() {
        let t = crate::tokens::load().remove("light-mustard").unwrap();
        let halo = |reduced: bool, emphasis: bool, frames: usize| {
            let ctx = egui::Context::default();
            let mut widths = Vec::new();
            for _ in 0..frames {
                let mut output = ctx.run_ui(Default::default(), |ui| {
                    crate::motion::set_reduced(ui.ctx(), reduced);
                    primary_button(ui, &t, shared::START, true, emphasis);
                });
                output.textures_delta.clear();
                widths = output
                    .shapes
                    .iter()
                    .filter_map(|shape| match &shape.shape {
                        egui::Shape::Rect(rect)
                            if rect.stroke.width > 0.
                                && rect.stroke_kind == egui::StrokeKind::Outside =>
                        {
                            Some(rect.stroke.width)
                        }
                        _ => None,
                    })
                    .collect();
            }
            widths
        };
        // The halo grows from nothing toward the 5 pt spread.
        let pulsing = halo(false, true, 1);
        assert!(pulsing.iter().all(|width| *width <= 5.), "{pulsing:?}");
        assert!(
            halo(true, true, 2).is_empty(),
            "reduced motion rests invisible"
        );
        assert!(
            halo(false, false, 2).is_empty(),
            "no halo until setup is ready"
        );
    }

    #[test]
    fn busy_labels_follow_the_in_flight_request() {
        assert_eq!(Busy::from_action(None), Busy::default());
        let screen = Busy::from_action(Some(shared::Action::RequestScreen));
        assert!(screen.any);
        assert_eq!(screen.opening, Some(Target::Screen));
        let complete = Busy::from_action(Some(shared::Action::Complete));
        assert!(complete.any && complete.opening.is_none());
    }
}
