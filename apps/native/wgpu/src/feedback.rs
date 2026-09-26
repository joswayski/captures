//! Shipping `Feedback.tsx` in its own window: the "Captures" eyebrow, heading
//! and intro, a settings card with category radio cards, message and optional
//! contact, an "Included automatically" details card, and a right-aligned
//! footer with the status to the left of Send. Copy, placeholders and limits
//! come from `captures_app::feedback`; sending from `captures-feedback`.
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex, MutexGuard, OnceLock, PoisonError,
        mpsc::{self, Receiver},
    },
    thread,
};

use captures_app::feedback as copy;
use captures_feedback::{FeedbackContext, FeedbackDraft};
use eframe::egui::{
    self, Color32, FontFamily, FontId, Rect, RichText, Sense, Stroke, StrokeKind, Vec2, pos2, vec2,
};

use crate::{preferences_widgets as widgets, tokens::Tokens};

const LAYOUT_PROBE_ENV: &str = "CAPTURES_NATIVE_LAYOUT_PROBE";

/// Named control rectangles (`[min x, min y, max x, max y]`, window points)
/// for smoke tests, emitted as `feedback-layout` when they change.
type ProbeRects = BTreeMap<String, [i32; 4]>;

fn probe_env() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os(LAYOUT_PROBE_ENV).is_some())
}

/// The feedback window. Shipping opens feedback in its own window rather than
/// inside Preferences.
pub fn viewport_id() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("feedback")
}

/// Worker replies change the deferred callback's state. Paint the window, and
/// the (possibly hidden) root that keeps registering it.
fn wake(ctx: &egui::Context) {
    ctx.request_repaint_of(egui::ViewportId::ROOT);
    ctx.send_viewport_cmd_to(
        egui::ViewportId::ROOT,
        egui::ViewportCommand::RequestPaintWhileHidden,
    );
    ctx.request_repaint_of(viewport_id());
}

#[derive(Default)]
pub struct Feedback {
    pub open: bool,
    message: String,
    contact: String,
    category: usize,
    context: Option<FeedbackContext>,
    context_rx: Option<Receiver<FeedbackContext>>,
    pending: Option<Receiver<Result<(), String>>>,
    result: Option<Result<(), String>>,
    probes: Option<ProbeRects>,
    last_probes: Option<ProbeRects>,
    /// Collect probes without the environment variable (unit tests).
    probing: bool,
}

impl Feedback {
    /// Callers run inside a root pass (tray action or the About card), which
    /// registers the window; an already open window is focused instead.
    pub fn open(&mut self, ctx: &egui::Context) {
        if self.open {
            ctx.send_viewport_cmd_to(viewport_id(), egui::ViewportCommand::Focus);
        }
        self.open = true;
        if self.context.is_none() && self.context_rx.is_none() {
            let (tx, rx) = mpsc::channel();
            self.context_rx = Some(rx);
            let wake_ctx = ctx.clone();
            thread::spawn(move || {
                let _ = tx.send(captures_feedback::native::context());
                wake(&wake_ctx);
            });
        }
    }

    fn poll(&mut self) {
        if let Some(context) = self.context_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.context = Some(context);
            self.context_rx = None;
        }
        if let Some(result) = self.pending.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.pending = None;
            if result.is_ok() {
                self.message.clear();
            }
            self.result = Some(result);
        }
    }

    fn can_submit(&self, live: bool) -> bool {
        live && self.context.is_some()
            && copy::can_submit(&self.message, &self.contact, self.pending.is_some())
    }

    fn submit(&mut self, ctx: &egui::Context, live: bool) {
        self.submit_with(ctx, live, captures_feedback::native::submit);
    }

    fn submit_with(
        &mut self,
        ctx: &egui::Context,
        live: bool,
        send: impl FnOnce(FeedbackDraft) -> Result<(), String> + Send + 'static,
    ) {
        if !self.can_submit(live) {
            return;
        }
        let contact = self.contact.trim();
        let draft = FeedbackDraft {
            message: self.message.trim().into(),
            contact: (!contact.is_empty()).then(|| contact.to_owned()),
            category: copy::CATEGORIES[self.category].id.into(),
        };
        let (tx, rx) = mpsc::channel();
        self.pending = Some(rx);
        self.result = None;
        let wake_ctx = ctx.clone();
        thread::spawn(move || {
            let _ = tx.send(send(draft));
            wake(&wake_ctx);
        });
    }

    fn probe(&mut self, name: &str, rect: Rect) {
        if let Some(probes) = &mut self.probes {
            probes.insert(
                name.to_owned(),
                [rect.min.x, rect.min.y, rect.max.x, rect.max.y]
                    .map(|value| value.clamp(-1e6, 1e6).round() as i32),
            );
        }
    }

    /// The footer status: shipping's error/sent notes, or why a fixture
    /// cannot send.
    fn status(&self, live: bool) -> Option<(&str, &'static str)> {
        match &self.result {
            Some(Err(error)) => Some((error.as_str(), "error")),
            Some(Ok(())) => Some((copy::SENT, "sent")),
            None if !live => Some((copy::FIXTURE_DISABLED, "note")),
            None => None,
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, t: &Tokens, live: bool) {
        self.poll();
        if probe_env() || self.probing {
            self.probes = Some(ProbeRects::new());
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(t.color("surface-canvas")))
            .show(ui, |ui| {
                crate::primitives::scroll_area(
                    ui,
                    t,
                    egui::ScrollArea::vertical()
                        .id_salt("feedback-form")
                        .auto_shrink([false, false]),
                    |ui| {
                        self.probe("Page", ui.clip_rect());
                        let side = t.number("s-8");
                        egui::Frame::new()
                            .inner_margin(egui::Margin {
                                left: side as i8,
                                right: side as i8,
                                top: t.number("s-9") as i8,
                                bottom: t.number("s-11") as i8,
                            })
                            .show(ui, |ui| {
                                // `.feedback-shell`: a centered 640 px column.
                                let available = ui.available_width();
                                let width = available.min(copy::SHELL_MAX_WIDTH);
                                let min = ui.cursor().min + vec2((available - width) / 2., 0.);
                                let column = Rect::from_min_size(min, vec2(width, f32::INFINITY));
                                ui.scope_builder(egui::UiBuilder::new().max_rect(column), |ui| {
                                    ui.set_width(width);
                                    self.form(ui, t, live);
                                });
                            });
                    },
                );
            });
        if let Some(probes) = self.probes.take()
            && self.last_probes.as_ref() != Some(&probes)
        {
            if probe_env() {
                crate::emit("feedback-layout", serde_json::json!({ "controls": probes }));
            }
            self.last_probes = Some(probes);
        }
    }

    fn form(&mut self, ui: &mut egui::Ui, t: &Tokens, live: bool) {
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        header(ui, t);
        ui.add_space(t.number("s-6"));
        let sending = self.pending.is_some();
        card(ui, t, |ui| {
            ui.add_enabled_ui(!sending, |ui| {
                self.categories(ui, t);
                rule(ui, t, t.number("s-5"));
                let label = field_label(ui, t, copy::MESSAGE_LABEL);
                ui.add_space(t.number("s-3"));
                let placeholder = copy::CATEGORIES[self.category].placeholder;
                let id = ui.make_persistent_id("feedback-message");
                let frame = field_frame(ui, t, id);
                let message = ui.add(
                    egui::TextEdit::multiline(&mut self.message)
                        .id(id)
                        .frame(frame)
                        .desired_width(f32::INFINITY)
                        .min_size(vec2(0., 148.))
                        .font(FontId::proportional(t.number("text-md")))
                        .text_color(t.color("text"))
                        .char_limit(copy::MESSAGE_LIMIT)
                        .hint_text(RichText::new(placeholder).color(t.color("text-faint"))),
                );
                field_focus(ui, t, &message);
                self.probe("Message", message.rect);
                if message.labelled_by(label.id).changed() {
                    self.result = None;
                }
                rule(ui, t, t.number("s-5"));
                let label = ui
                    .horizontal(|ui| {
                        let label = field_label(ui, t, copy::CONTACT_LABEL);
                        ui.add_space(t.number("s-2"));
                        optional_badge(ui, t);
                        label
                    })
                    .inner;
                ui.add_space(t.number("s-3"));
                let id = ui.make_persistent_id("feedback-contact");
                let frame = field_frame(ui, t, id);
                let contact = ui.add(
                    egui::TextEdit::singleline(&mut self.contact)
                        .id(id)
                        .frame(frame)
                        .desired_width(f32::INFINITY)
                        .font(FontId::proportional(t.number("text-md")))
                        .text_color(t.color("text"))
                        .char_limit(copy::CONTACT_LIMIT)
                        .hint_text(
                            RichText::new(copy::CONTACT_PLACEHOLDER).color(t.color("text-faint")),
                        ),
                );
                field_focus(ui, t, &contact);
                self.probe("Contact", contact.rect);
                contact.labelled_by(label.id);
                ui.add_space(t.number("s-3"));
                help_text(ui, t, copy::CONTACT_HELP, f32::INFINITY);
            });
        });
        ui.add_space(t.number("s-6"));
        card(ui, t, |ui| {
            ui.label(
                RichText::new(copy::META_TITLE)
                    .size(t.number("text-md"))
                    .color(t.color("text")),
            );
            rule(ui, t, t.number("s-4"));
            let system = self
                .context
                .as_ref()
                .map(|context| copy::system_label(&context.os, &context.os_version, &context.arch));
            let version = self
                .context
                .as_ref()
                .map(|context| context.app_version.as_str());
            meta_row(
                ui,
                t,
                copy::APP_VERSION_LABEL,
                version.unwrap_or(copy::LOADING),
            );
            ui.add_space(t.number("s-3"));
            meta_row(
                ui,
                t,
                copy::SYSTEM_LABEL,
                system.as_deref().unwrap_or(copy::LOADING),
            );
        });
        ui.add_space(t.number("s-6"));
        self.footer(ui, t, live);
    }

    /// Shipping `.feedback-categories`: a labelled group of three radio cards.
    fn categories(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        let label = field_label(ui, t, copy::CATEGORY_LABEL);
        ui.add_space(t.number("s-3"));
        ui.scope_builder(
            egui::UiBuilder::new().id_salt("feedback-categories"),
            |ui| {
                let group_id = ui.scope_id();
                ui.ctx().accesskit_node_builder(group_id, |node| {
                    node.set_role(egui::accesskit::Role::RadioGroup);
                    node.push_labelled_by(label.id.accesskit_id());
                });
                let gap = t.number("s-3");
                let pad = t.number("s-4");
                let width = ((ui.available_width() - gap * 2.) / 3.).max(0.);
                let text_width = (width - pad * 2.).max(1.);
                let galleys = copy::CATEGORIES.map(|category| {
                    let title = ui.painter().layout(
                        category.label.to_owned(),
                        FontId::proportional(t.number("text-md")),
                        t.color("text"),
                        text_width,
                    );
                    let description = ui.painter().layout(
                        category.description.to_owned(),
                        FontId::proportional(t.number("text-xs")),
                        t.color("text-subtle"),
                        text_width,
                    );
                    (title, description)
                });
                let height = galleys
                    .iter()
                    .map(|(title, description)| {
                        pad * 2. + title.size().y + 3. + description.size().y
                    })
                    .fold(62_f32, f32::max);
                let (row, _) =
                    ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
                let enabled = ui.is_enabled();
                for (index, (category, (title, description))) in
                    copy::CATEGORIES.iter().zip(galleys).enumerate()
                {
                    let rect = Rect::from_min_size(
                        row.min + vec2(index as f32 * (width + gap), 0.),
                        vec2(width, height),
                    );
                    let response = ui.interact(rect, group_id.with(index), Sense::click());
                    let active = self.category == index;
                    response.widget_info(|| {
                        egui::WidgetInfo::selected(
                            egui::WidgetType::RadioButton,
                            enabled,
                            active,
                            category.label,
                        )
                    });
                    if response.clicked() {
                        self.category = index;
                    }
                    let hovered = enabled && response.hovered() && !active;
                    let radius = t.number("r-md");
                    let (fill, border) = if active {
                        ("surface-selected", "theme-accent")
                    } else if hovered {
                        ("surface-hover", "border-strong")
                    } else {
                        ("surface-canvas", "border")
                    };
                    let painter = ui.painter();
                    painter.rect(
                        rect,
                        radius,
                        t.color(fill),
                        // Border plus the 1 px inset accent shadow when active.
                        Stroke::new(if active { 2. } else { 1. }, t.color(border)),
                        StrokeKind::Inside,
                    );
                    let title_pos = rect.min + Vec2::splat(pad);
                    let title_height = title.size().y;
                    painter.galley(title_pos, title, t.color("text"));
                    painter.galley(
                        title_pos + vec2(0., title_height + 3.),
                        description,
                        t.color("text-subtle"),
                    );
                    if response.has_focus() {
                        crate::primitives::focus_ring(ui, t, rect, radius);
                    }
                    self.probe(&format!("Category.{}", category.label), rect);
                }
            },
        );
    }

    /// Shipping `.feedback-actions`: the status fills the space to the left of
    /// a right-aligned Send button.
    fn footer(&mut self, ui: &mut egui::Ui, t: &Tokens, live: bool) {
        let label = if self.pending.is_some() {
            copy::SENDING
        } else {
            copy::SEND
        };
        let font = semibold(ui, t.number("text-md"));
        let button_text =
            ui.painter()
                .layout_no_wrap(label.to_owned(), font, t.color("theme-accent-ink"));
        let button_size = vec2(
            button_text.size().x + t.number("s-6") * 2.,
            t.number("h-lg"),
        );
        let width = ui.available_width();
        let gap = t.number("s-5");
        let status_width = (width - button_size.x - gap).max(1.);
        let status = self.status(live).map(|(message, kind)| {
            let color = match kind {
                "error" => "danger-text",
                "sent" => "positive-text",
                _ => "text-muted",
            };
            let galley = ui.painter().layout(
                message.to_owned(),
                FontId::proportional(t.number("text-sm")),
                t.color(color),
                (status_width - t.number("s-4") * 2.).max(1.),
            );
            (galley, kind, message.to_owned())
        });
        let status_height = status
            .as_ref()
            .map_or(0., |(galley, _, _)| galley.size().y + t.number("s-3") * 2.);
        let (row, _) = ui.allocate_exact_size(
            vec2(width, button_size.y.max(status_height)),
            Sense::hover(),
        );
        if let Some((galley, kind, message)) = status {
            let rect = Rect::from_min_size(
                pos2(row.left(), row.center().y - status_height / 2.),
                vec2(status_width, status_height),
            );
            let fill = match kind {
                "error" => "danger-surface",
                "sent" => "positive-surface",
                _ => "surface-sunken",
            };
            ui.painter()
                .rect_filled(rect, t.number("r-md"), t.color(fill));
            ui.painter().galley(
                rect.min + vec2(t.number("s-4"), t.number("s-3")),
                galley,
                t.color("text"),
            );
            let response = ui.interact(rect, ui.scope_id().with("feedback-status"), Sense::hover());
            response
                .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, &message));
            self.probe("Status", rect);
        }
        let rect = Rect::from_min_size(
            pos2(
                row.right() - button_size.x,
                row.center().y - button_size.y / 2.,
            ),
            button_size,
        );
        let enabled = self.can_submit(live);
        let response = ui.interact(
            rect,
            ui.scope_id().with("feedback-send"),
            if enabled {
                Sense::click()
            } else {
                Sense::hover()
            },
        );
        response
            .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
        let alpha = if enabled { 1. } else { 0.45 };
        let fill = if enabled && response.hovered() {
            "theme-accent-hover"
        } else {
            "theme-accent"
        };
        let radius = t.number("r-md");
        ui.painter()
            .rect_filled(rect, radius, t.color(fill).gamma_multiply(alpha));
        ui.painter().galley(
            rect.center() - button_text.size() / 2.,
            button_text,
            t.color("theme-accent-ink").gamma_multiply(alpha),
        );
        if response.has_focus() {
            crate::primitives::focus_ring(ui, t, rect, radius);
        }
        self.probe("Send", rect);
        if enabled && response.clicked() {
            self.submit(ui.ctx(), live);
        }
    }
}

/// Owns the form across window close/reopen so drafts and in-flight sends
/// survive, and registers the deferred feedback viewport while it is open.
#[derive(Default)]
pub struct FeedbackWindow {
    form: Arc<Mutex<Feedback>>,
    painted: Option<(Color32, Color32)>,
}

impl FeedbackWindow {
    fn lock(&self) -> MutexGuard<'_, Feedback> {
        self.form.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn open(&self, ctx: &egui::Context) {
        self.lock().open(ctx);
    }

    #[cfg(test)]
    pub fn is_open(&self) -> bool {
        self.lock().open
    }

    /// Call on every root pass. Dropping the registration closes the window.
    pub fn show(&mut self, ctx: &egui::Context, t: &Tokens, live: bool) {
        if !self.lock().open {
            self.painted = None;
            return;
        }
        let form = self.form.clone();
        let tokens = t.clone();
        ctx.show_viewport_deferred(
            viewport_id(),
            egui::ViewportBuilder::default()
                .with_title(copy::WINDOW_TITLE)
                .with_inner_size([copy::WINDOW_WIDTH, copy::WINDOW_HEIGHT])
                .with_min_inner_size([copy::WINDOW_MIN_WIDTH, copy::WINDOW_MIN_HEIGHT])
                .with_resizable(true),
            move |ui, _| {
                let mut form = form.lock().unwrap_or_else(PoisonError::into_inner);
                if ui.input(|input| input.viewport().close_requested()) {
                    // Keep the draft: stop registering the viewport instead.
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::CancelClose);
                    form.open = false;
                    wake(ui.ctx());
                    return;
                }
                form.ui(ui, &tokens, live);
            },
        );
        // Appearance/theme changes re-register the callback on the root; paint
        // the window too so it does not wait for pointer motion.
        let painted = (t.color("surface-canvas"), t.color("theme-accent"));
        if self.painted != Some(painted) {
            self.painted = Some(painted);
            ctx.request_repaint_of(viewport_id());
        }
    }
}

/// `--weight-semibold` when the host installed the system semibold face.
fn semibold(ui: &egui::Ui, size: f32) -> FontId {
    let family = FontFamily::Name(crate::ui_fonts::SEMIBOLD.into());
    if ui.fonts(|fonts| fonts.definitions().families.contains_key(&family)) {
        FontId::new(size, family)
    } else {
        FontId::proportional(size)
    }
}

/// `.feedback-header`: eyebrow, heading and the 56ch intro.
fn header(ui: &mut egui::Ui, t: &Tokens) {
    ui.label(
        RichText::new(copy::EYEBROW.to_uppercase())
            .font(semibold(ui, t.number("text-2xs")))
            .extra_letter_spacing(t.number("text-2xs") * 0.04)
            .color(t.color("text-subtle")),
    );
    ui.add_space(t.number("s-2"));
    ui.label(
        RichText::new(copy::TITLE)
            .font(semibold(ui, t.number("text-2xl")))
            .color(t.color("text")),
    );
    ui.add_space(t.number("s-4"));
    help_text(ui, t, copy::INTRO, 56. * t.number("text-sm") * 0.56);
}

fn help_text(ui: &mut egui::Ui, t: &Tokens, text: &str, max_width: f32) {
    let width = ui.available_width().min(max_width);
    ui.allocate_ui(vec2(width, 0.), |ui| {
        ui.add(
            egui::Label::new(
                RichText::new(text)
                    .size(t.number("text-sm"))
                    .color(t.color("text-subtle")),
            )
            .wrap(),
        );
    });
}

/// `.field-label`.
fn field_label(ui: &mut egui::Ui, t: &Tokens, text: &str) -> egui::Response {
    ui.label(
        RichText::new(text)
            .size(t.number("text-sm"))
            .color(t.color("text-muted")),
    )
}

/// `.feedback-optional`: an uppercase pill beside the Contact label.
fn optional_badge(ui: &mut egui::Ui, t: &Tokens) {
    let size = t.number("text-2xs");
    let galley = ui.painter().layout_no_wrap(
        copy::OPTIONAL_BADGE.to_uppercase(),
        FontId::proportional(size),
        t.color("text-subtle"),
    );
    let tracking = size * 0.04 * copy::OPTIONAL_BADGE.chars().count() as f32;
    let (rect, _) = ui.allocate_exact_size(
        vec2(
            galley.size().x + tracking + t.number("s-3") * 2.,
            galley.size().y + 2.,
        ),
        Sense::hover(),
    );
    ui.painter()
        .rect_filled(rect, rect.height() / 2., t.color("surface-sunken"));
    let job = egui::text::LayoutJob::single_section(
        copy::OPTIONAL_BADGE.to_uppercase(),
        egui::TextFormat {
            font_id: FontId::proportional(size),
            color: t.color("text-subtle"),
            extra_letter_spacing: size * 0.04,
            ..Default::default()
        },
    );
    let galley = ui.painter().layout_job(job);
    ui.painter().galley(
        rect.center() - galley.size() / 2.,
        galley,
        t.color("text-subtle"),
    );
}

/// `.ui-input` frame for the message and contact fields.
fn field_frame(ui: &egui::Ui, t: &Tokens, id: egui::Id) -> egui::Frame {
    let focused = ui.memory(|memory| memory.has_focus(id));
    egui::Frame::new()
        .fill(t.color("surface-field"))
        .stroke(Stroke::new(
            1.,
            t.color(if focused {
                "theme-accent"
            } else {
                "control-border"
            }),
        ))
        .corner_radius(t.number("r-md") as u8)
        .inner_margin(egui::Margin::symmetric(
            t.number("s-5") as i8,
            t.number("s-4") as i8,
        ))
}

/// `--focus-ring-tight` around a focused field.
fn field_focus(ui: &egui::Ui, t: &Tokens, response: &egui::Response) {
    if response.has_focus() {
        crate::primitives::focus_ring(ui, t, response.rect, t.number("r-md"));
    }
}

/// `.settings-card`.
fn card(ui: &mut egui::Ui, t: &Tokens, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(t.color("surface-raised"))
        .stroke(Stroke::new(1., t.color("border-subtle")))
        .corner_radius(t.number("r-xl") as u8)
        .inner_margin(t.number("s-6") as i8)
        .shadow(widgets::shadow_sm(ui.visuals().dark_mode))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            add(ui);
        });
}

/// `.settings-card > * + *`: the card gap, a subtle rule, then `--s-5`.
fn rule(ui: &mut egui::Ui, t: &Tokens, gap: f32) {
    ui.add_space(gap);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.), Sense::hover());
    ui.painter().hline(
        rect.x_range(),
        rect.center().y,
        Stroke::new(1., t.color("border-subtle")),
    );
    ui.add_space(t.number("s-5"));
}

/// `.feedback-meta` row: a 112 px label column and a monospace value.
fn meta_row(ui: &mut egui::Ui, t: &Tokens, label: &str, value: &str) {
    ui.horizontal_top(|ui| {
        ui.allocate_ui(vec2(112., 0.), |ui| {
            ui.set_width(112.);
            ui.label(
                RichText::new(label)
                    .size(t.number("text-sm"))
                    .color(t.color("text-subtle")),
            );
        });
        ui.add_space(t.number("s-4"));
        ui.add(
            egui::Label::new(
                RichText::new(value)
                    .font(FontId::monospace(t.number("text-sm")))
                    .color(t.color("text")),
            )
            .wrap(),
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::{Duration, Instant},
    };

    #[test]
    fn context_and_submission_workers_wake_root_while_an_editor_is_active() {
        for load_context in [true, false] {
            let ctx = egui::Context::default();
            let wakes = crate::root_repaint::observe_from_child(&ctx);
            let mut form = Feedback::default();
            if load_context {
                form.open(&ctx);
            } else {
                form.context = Some(captures_feedback::native::context());
                form.message = "Explicit test feedback".into();
                form.submit_with(&ctx, true, |_| Ok(()));
            }
            assert_eq!(
                wakes.recv_timeout(Duration::from_secs(5)).unwrap(),
                egui::ViewportId::ROOT
            );
            form.poll();
            if load_context {
                assert!(form.context.is_some());
                assert!(form.context_rx.is_none());
            } else {
                assert_eq!(form.result, Some(Ok(())));
                assert!(form.message.is_empty());
            }
            ctx.end_pass().textures_delta.clear();
        }
    }

    #[test]
    fn explicit_submit_is_gated_and_failed_draft_survives_navigation_and_retry() {
        let ctx = egui::Context::default();
        let mut form = Feedback {
            context: Some(captures_feedback::native::context()),
            ..Default::default()
        };
        assert!(!form.can_submit(true));
        form.message = "  selector overlaps the toolbar  ".into();
        form.category = 1;
        form.contact = "   ".into();
        assert!(!form.can_submit(false));
        let (release, wait) = mpsc::channel();
        let count = Arc::new(AtomicUsize::new(0));
        let sends = count.clone();
        form.submit_with(&ctx, true, move |draft| {
            sends.fetch_add(1, Ordering::SeqCst);
            assert_eq!(draft.message, "selector overlaps the toolbar");
            assert_eq!(draft.category, "idea");
            assert_eq!(draft.contact, None);
            wait.recv().unwrap();
            Err("Offline — try again".into())
        });
        form.submit_with(&ctx, true, |_| panic!("duplicate send"));
        form.open = false;
        form.open(&ctx);
        assert!(!form.can_submit(true));
        release.send(()).unwrap();
        drain(&mut form);
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert_eq!(form.message, "  selector overlaps the toolbar  ");
        assert_eq!(form.result, Some(Err("Offline — try again".into())));
        form.submit_with(&ctx, true, |_| Ok(()));
        drain(&mut form);
        assert!(form.message.is_empty());
        assert_eq!(form.result, Some(Ok(())));
    }

    #[test]
    fn unicode_limits_and_fixture_mode_prevent_submission() {
        let ctx = egui::Context::default();
        let mut form = Feedback {
            context: Some(captures_feedback::native::context()),
            message: "🦀".repeat(8000),
            contact: "é".repeat(200),
            ..Default::default()
        };
        assert!(form.can_submit(true));
        form.submit_with(&ctx, false, |_| panic!("fixture sent feedback"));
        form.message.push('x');
        assert!(!form.can_submit(true));
        form.message.pop();
        form.contact.push('x');
        assert!(!form.can_submit(true));
    }

    fn render(
        ctx: &egui::Context,
        form: &mut Feedback,
        tokens: &Tokens,
        live: bool,
        size: Vec2,
        events: Vec<egui::Event>,
    ) -> ProbeRects {
        form.probing = true;
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, size)),
                events,
                ..Default::default()
            },
            |ui| form.ui(ui, tokens, live),
        );
        output.textures_delta.clear();
        form.last_probes.clone().unwrap_or_default()
    }

    fn rect(probes: &ProbeRects, name: &str) -> Rect {
        let [x0, y0, x1, y1] = *probes
            .get(name)
            .unwrap_or_else(|| panic!("missing {name} in {:?}", probes.keys()));
        Rect::from_min_max(pos2(x0 as f32, y0 as f32), pos2(x1 as f32, y1 as f32))
    }

    #[test]
    fn shipping_layout_fits_default_and_minimum_windows() {
        for (appearance, size) in [
            (
                "dark-mustard",
                vec2(copy::WINDOW_WIDTH, copy::WINDOW_HEIGHT),
            ),
            (
                "light-mustard",
                vec2(copy::WINDOW_MIN_WIDTH, copy::WINDOW_MIN_HEIGHT),
            ),
        ] {
            let tokens = crate::tokens::load()[appearance].clone();
            let ctx = egui::Context::default();
            let mut form = Feedback {
                context: Some(captures_feedback::native::context()),
                ..Default::default()
            };
            let mut probes = ProbeRects::new();
            for _ in 0..3 {
                probes = render(&ctx, &mut form, &tokens, false, size, vec![]);
            }
            let side = tokens.number("s-8");
            let page = rect(&probes, "Page");
            let bug = rect(&probes, "Category.Bug");
            let idea = rect(&probes, "Category.Idea");
            let other = rect(&probes, "Category.Other");
            // Three equal radio cards in one row inside the settings card.
            assert_eq!(bug.top(), other.top());
            assert!(bug.right() < idea.left() && idea.right() < other.left());
            assert!((bug.width() - other.width()).abs() <= 1.);
            assert!(bug.height() >= 62.);
            let message = rect(&probes, "Message");
            assert!(message.top() > bug.bottom() && message.height() >= 148.);
            assert!(rect(&probes, "Contact").top() > message.bottom());
            // Right-aligned footer, status to its left (fixtures cannot send).
            let send = rect(&probes, "Send");
            let status = rect(&probes, "Status");
            assert!((page.right() - side - send.right()).abs() <= 1., "{send:?}");
            assert!(status.right() < send.left() && status.left() >= page.left() + side - 1.);
            assert!(
                send.bottom() > page.bottom(),
                "shipping's footer scrolls with the form"
            );
        }
    }

    #[test]
    fn radio_cards_select_the_category_and_its_placeholder() {
        let tokens = crate::tokens::load()["dark-mustard"].clone();
        let ctx = egui::Context::default();
        let mut form = Feedback::default();
        let size = vec2(copy::WINDOW_WIDTH, copy::WINDOW_HEIGHT);
        let mut probes = ProbeRects::new();
        for _ in 0..3 {
            probes = render(&ctx, &mut form, &tokens, true, size, vec![]);
        }
        let idea = rect(&probes, "Category.Idea").center();
        let press = |pressed| egui::Event::PointerButton {
            pos: idea,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        render(
            &ctx,
            &mut form,
            &tokens,
            true,
            size,
            vec![egui::Event::PointerMoved(idea)],
        );
        render(&ctx, &mut form, &tokens, true, size, vec![press(true)]);
        render(&ctx, &mut form, &tokens, true, size, vec![press(false)]);
        assert_eq!(form.category, 1);
        assert_eq!(
            copy::CATEGORIES[form.category].placeholder,
            "What's the idea? What problem would it solve?"
        );
        // No context yet: shipping's loading value, and Send stays disabled.
        assert!(!form.can_submit(true));
    }

    fn drain(form: &mut Feedback) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while form.pending.is_some() {
            form.poll();
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        }
    }
}
