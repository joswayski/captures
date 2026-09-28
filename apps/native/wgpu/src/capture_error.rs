//! Shipping `report_capture_error` on Windows and Linux: a failed tray,
//! shortcut or capture-menu capture shows a modal "Captures" error dialog
//! with one OK button. It is its own window, so it is visible while History
//! is hidden, and the History error card never repeats it.
//!
//! Shipping draws the stock dialog through `rfd`, which needs `zenity` on
//! Linux; this host draws the same copy in its own window instead.
use std::sync::{Arc, Mutex};

use captures_app::capture_error;
use eframe::egui::{self, RichText, Stroke};

use crate::live::{CaptureFailure, FailureReport, request_hidden_root_paint};
use crate::preferences::Preferences;
use crate::tokens::Tokens;

const WIDTH: f32 = 440.;
const HEIGHT: f32 = 168.;

#[derive(Clone, Debug, Eq, PartialEq)]
struct Shown {
    generation: u64,
    title: &'static str,
    message: String,
}

#[derive(Default)]
pub struct CaptureErrorDialog {
    shown: Arc<Mutex<Option<Shown>>>,
    next: u64,
}

impl CaptureErrorDialog {
    /// Presents `failure` the shipping way. Returns true when it opened
    /// permission recovery (over History) instead of the dialog.
    pub fn report(
        &mut self,
        failure: &CaptureFailure,
        preferences: &mut Preferences,
        ctx: &egui::Context,
    ) -> bool {
        match failure.report() {
            FailureReport::PermissionRecovery => {
                preferences.open_permission_recovery();
                true
            }
            FailureReport::Dialog { title, message } => {
                self.show(title, message);
                request_hidden_root_paint(ctx);
                ctx.request_repaint();
                false
            }
        }
    }

    fn show(&mut self, title: &'static str, message: String) {
        self.next += 1;
        *self.shown.lock().unwrap() = Some(Shown {
            generation: self.next,
            title,
            message,
        });
    }

    #[cfg(test)]
    fn current(&self) -> Option<(&'static str, String)> {
        self.shown
            .lock()
            .unwrap()
            .as_ref()
            .map(|shown| (shown.title, shown.message.clone()))
    }

    pub fn viewport(&self, ctx: &egui::Context, tokens: &Tokens) {
        let Some(shown) = self.shown.lock().unwrap().clone() else {
            return;
        };
        let size = egui::vec2(WIDTH, HEIGHT);
        let builder = egui::ViewportBuilder::default()
            .with_title(shown.title)
            .with_inner_size(size)
            .with_min_inner_size(size)
            .with_max_inner_size(size)
            .with_resizable(false)
            .with_minimize_button(false)
            .with_maximize_button(false)
            .with_always_on_top()
            .with_active(true);
        #[cfg(target_os = "linux")]
        let builder = builder.with_window_type(egui::X11WindowType::Dialog);
        let state = self.shown.clone();
        let tokens = tokens.clone();
        let viewport = egui::ViewportId::from_hash_of(("capture-error", shown.generation));
        ctx.show_viewport_deferred(viewport, builder, move |ui, _| {
            let dismissed = body(ui, &tokens, &shown.message)
                || ui.input(|input| {
                    input.viewport().close_requested()
                        || input.key_pressed(egui::Key::Enter)
                        || input.key_pressed(egui::Key::Escape)
                });
            if dismissed {
                let mut current = state.lock().unwrap();
                if current
                    .as_ref()
                    .is_some_and(|current| current.generation == shown.generation)
                {
                    *current = None;
                }
                drop(current);
                request_hidden_root_paint(ui.ctx());
                ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
            }
        });
    }
}

/// The message over `--surface-raised`, OK at the trailing edge. Returns
/// true when OK was pressed.
fn body(ui: &mut egui::Ui, t: &Tokens, message: &str) -> bool {
    let mut ok = false;
    egui::CentralPanel::default()
        .frame(
            egui::Frame::new()
                .fill(t.color("surface-raised"))
                .inner_margin(t.number("s-6") as i8),
        )
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = t.number("s-4");
            let actions = t.number("h-xl") + t.number("s-4");
            egui::ScrollArea::vertical()
                .max_height((ui.available_height() - actions).max(0.))
                .show(ui, |ui| {
                    crate::accessibility::set_role(ui, egui::accesskit::Role::Alert);
                    ui.add(
                        egui::Label::new(
                            RichText::new(message)
                                .size(t.number("text-md"))
                                .color(t.color("text")),
                        )
                        .wrap(),
                    );
                });
            let height = t.number("h-xl");
            let button = egui::Button::new(
                RichText::new(capture_error::OK)
                    .size(t.number("text-md"))
                    .color(t.color("solid-ink"))
                    .strong(),
            )
            .fill(t.color("solid"))
            .stroke(Stroke::NONE)
            .corner_radius(t.number("r-md") as u8);
            // Trailing edge, like the stock dialog's OK.
            let max = ui.max_rect().max;
            let rect = egui::Rect::from_min_max(egui::pos2(max.x - 88., max.y - height), max);
            ok = ui.put(rect, button).clicked();
        });
    ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screenshot_failures_use_the_shipping_capture_dialog() {
        let mut dialog = CaptureErrorDialog::default();
        let failure = CaptureFailure {
            error: "The selected display is no longer available.".into(),
            recording: false,
        };
        let FailureReport::Dialog { title, message } = failure.report() else {
            panic!("a plain capture failure uses the dialog");
        };
        dialog.show(title, message);
        assert_eq!(
            dialog.current(),
            Some((
                "Captures",
                "Captures could not start the capture: The selected display is no longer available."
                    .to_owned()
            ))
        );
    }

    #[test]
    fn recording_and_permission_failures_follow_shipping() {
        let recording = CaptureFailure {
            error: "FFmpeg is missing".into(),
            recording: true,
        };
        assert_eq!(
            recording.report(),
            FailureReport::Dialog {
                title: "Captures Recording",
                message: "FFmpeg is missing".into(),
            }
        );
        let denied = CaptureFailure {
            error: format!(
                "Could not start the capture: {}",
                captures_capture::CaptureError::PermissionDenied
            ),
            recording: false,
        };
        assert_eq!(denied.report(), FailureReport::PermissionRecovery);
    }

    #[test]
    fn a_new_failure_replaces_the_open_dialog() {
        let mut dialog = CaptureErrorDialog::default();
        dialog.show(capture_error::TITLE, "first".into());
        dialog.show(capture_error::TITLE, "second".into());
        assert_eq!(
            dialog.current().map(|(_, message)| message).as_deref(),
            Some("second")
        );
        assert_eq!(dialog.next, 2);
    }
}
