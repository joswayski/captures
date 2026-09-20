use std::{
    sync::mpsc::{self, Receiver},
    thread,
};

use captures_feedback::{FeedbackContext, FeedbackDraft};
use eframe::egui::{self, RichText};

use crate::tokens::Tokens;

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
}

impl Feedback {
    pub fn open(&mut self, ctx: &egui::Context) {
        self.open = true;
        if self.context.is_none() && self.context_rx.is_none() {
            let (tx, rx) = mpsc::channel();
            self.context_rx = Some(rx);
            let wake = ctx.clone();
            thread::spawn(move || {
                let _ = tx.send(captures_feedback::native::context());
                wake.request_repaint();
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
            && self.pending.is_none()
            && !self.message.trim().is_empty()
            && self.message.chars().count() <= 8000
            && self.contact.chars().count() <= 200
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
            category: ["bug", "idea", "other"][self.category].into(),
        };
        let (tx, rx) = mpsc::channel();
        self.pending = Some(rx);
        self.result = None;
        let wake = ctx.clone();
        thread::spawn(move || {
            let _ = tx.send(send(draft));
            wake.request_repaint();
        });
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, t: &Tokens, live: bool) {
        self.poll();
        // Keep submission/status reachable even when system details or errors
        // wrap, or the host reserves space for lifecycle warnings below us.
        egui::Panel::bottom("feedback-actions").show(ui, |ui| {
            ui.set_max_width(664.);
            if !live {
                ui.label("Fixture mode — sending feedback is disabled.");
            }
            if let Some(result) = &self.result {
                match result {
                    Ok(()) => {
                        ui.colored_label(t.color("positive-text"), "Thanks — feedback sent.");
                    }
                    Err(error) => {
                        ui.label(
                            RichText::new(format!("Not sent — {error}")).color(t.color("text")),
                        );
                    }
                }
            }
            if ui
                .add_enabled(
                    self.can_submit(live),
                    egui::Button::new(if self.pending.is_some() {
                        "Sending…"
                    } else {
                        "Send feedback"
                    }),
                )
                .clicked()
            {
                self.submit(ui.ctx(), live);
            }
        });
        egui::ScrollArea::vertical().id_salt("feedback-form").show(ui, |ui| {
            ui.set_max_width(664.);
            ui.spacing_mut().item_spacing.y = t.number("s-3");
            if ui.button("Back to Preferences").clicked() { self.open = false; }
            ui.add_space(t.number("s-6"));
            ui.heading("Send feedback");
            ui.label("Tell us what broke, what is missing, or what you wish worked better.");
            ui.label(RichText::new("Only what you type and the details below are sent to captur.es. No captures, files, or diagnostics are attached.").color(t.color("text-muted")));
            ui.add_space(t.number("s-6"));
            ui.add_enabled_ui(self.pending.is_none(), |ui| {
                ui.label("Category");
                ui.horizontal(|ui| {
                    for (index, label) in ["Bug", "Idea", "Other"].iter().enumerate() {
                        ui.selectable_value(&mut self.category, index, *label);
                    }
                });
                ui.add_space(t.number("s-4"));
                let label = ui.label("Message");
                let message = ui.add(egui::TextEdit::multiline(&mut self.message)
                    .desired_width(f32::INFINITY).desired_rows(7).char_limit(8000)
                    .hint_text(["What happened? What did you expect?", "What problem would the idea solve?", "What would you like us to know?"][self.category]));
                if message.labelled_by(label.id).changed() { self.result = None; }
                ui.add_space(t.number("s-4"));
                let label = ui.label("Contact (optional)");
                ui.add(egui::TextEdit::singleline(&mut self.contact).desired_width(f32::INFINITY)
                    .char_limit(200).hint_text("X handle, GitHub username, email…")).labelled_by(label.id);
                ui.label(RichText::new("We may use this to ask a follow-up question.").small().color(t.color("text-muted")));
            });
            ui.add_space(t.number("s-6"));
            ui.separator();
            ui.strong("Included automatically");
            if let Some(context) = &self.context {
                ui.label(format!("App version: {}", context.app_version));
                ui.label(format!("System: {} · {} · {}", context.os, context.os_version, context.arch));
            } else { ui.label("Loading local app and system details…"); }
            ui.add_space(t.number("s-6"));
        });
    }
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

    fn drain(form: &mut Feedback) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while form.pending.is_some() {
            form.poll();
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(1));
        }
    }
}
