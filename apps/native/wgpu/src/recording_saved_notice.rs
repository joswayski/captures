use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use eframe::egui::{self, RichText, Stroke};

use crate::tokens::Tokens;

pub const SIZE: egui::Vec2 = egui::vec2(440.0, 116.0);
pub const LIFETIME: Duration = Duration::from_millis(15_200);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Guard {
    pub artifact_id: String,
    pub generation: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Action {
    Save(Guard),
    Reveal(Guard, PathBuf),
    Dismiss(Guard),
}

#[derive(Clone, Debug)]
pub struct Notice {
    pub guard: Guard,
    pub saved_path: Option<PathBuf>,
    pub pending: bool,
    pub error: Option<String>,
    expires_at: Instant,
}

impl Notice {
    pub fn new(artifact_id: String, generation: u64, now: Instant) -> Self {
        Self {
            guard: Guard {
                artifact_id,
                generation,
            },
            saved_path: None,
            pending: false,
            error: None,
            expires_at: now + LIFETIME,
        }
    }

    pub fn begin_save(&mut self) -> Option<Guard> {
        if self.pending || self.saved_path.is_some() {
            return None;
        }
        self.pending = true;
        self.error = None;
        Some(self.guard.clone())
    }

    pub fn save_result(
        &mut self,
        guard: &Guard,
        result: Result<PathBuf, String>,
        now: Instant,
    ) -> bool {
        if &self.guard != guard || !self.pending {
            return false;
        }
        self.pending = false;
        self.expires_at = now + LIFETIME;
        match result {
            Ok(path) => {
                self.saved_path = Some(path);
                self.error = None;
            }
            Err(error) => self.error = Some(format!("Could not save the recording: {error}")),
        }
        true
    }

    pub fn mark_saved(&mut self, artifact_id: &str, path: PathBuf, now: Instant) -> bool {
        if self.guard.artifact_id != artifact_id {
            return false;
        }
        self.saved_path = Some(path);
        self.pending = false;
        self.error = None;
        self.expires_at = now + LIFETIME;
        true
    }

    pub fn expired(&self, now: Instant) -> bool {
        !self.pending && now >= self.expires_at
    }

    pub fn fail(&mut self, error: String, now: Instant) {
        self.error = Some(error);
        self.expires_at = now + LIFETIME;
    }

    pub fn remaining(&self, now: Instant) -> Option<Duration> {
        (!self.pending).then(|| self.expires_at.saturating_duration_since(now))
    }
}

pub fn show(ui: &mut egui::Ui, tokens: &Tokens, notice: &Notice) -> Option<Action> {
    let mut action = None;
    let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, SIZE);
    ui.painter()
        .rect_filled(rect, tokens.number("r-xl"), tokens.color("glass-strong"));
    ui.painter().rect_stroke(
        rect,
        tokens.number("r-xl"),
        Stroke::new(1., tokens.color("glass-border")),
        egui::StrokeKind::Inside,
    );
    tokens.glass_controls(ui);
    let icon = egui::Rect::from_min_size(egui::pos2(18., 18.), egui::vec2(20., 20.));
    ui.painter()
        .circle_filled(icon.center(), 10., tokens.color("positive"));
    ui.painter().line_segment(
        [
            icon.center() + egui::vec2(-5., 0.),
            icon.center() + egui::vec2(-1., 4.),
        ],
        Stroke::new(2., tokens.color("positive-ink")),
    );
    ui.painter().line_segment(
        [
            icon.center() + egui::vec2(-1., 4.),
            icon.center() + egui::vec2(5., -4.),
        ],
        Stroke::new(2., tokens.color("positive-ink")),
    );
    ui.scope_builder(
        egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(
            egui::pos2(46., 14.),
            egui::vec2(352., 52.),
        )),
        |ui| {
            ui.label(
                RichText::new(if notice.saved_path.is_some() {
                    "Recording saved"
                } else {
                    "Recording ready"
                })
                .strong()
                .color(tokens.color("glass-text")),
            );
            egui::ScrollArea::vertical()
                .id_salt("recording-notice-detail")
                .max_height(32.)
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(notice.error.as_deref().unwrap_or(
                            if notice.saved_path.is_some() {
                                "Saved to your Captures folder."
                            } else {
                                "Kept in Capture History for 30 days. Save a copy anytime."
                            },
                        ))
                        .small()
                        .color(tokens.color("glass-text")),
                    )
                    .on_hover_text(notice.error.as_deref().unwrap_or(""));
                });
        },
    );
    let label = if notice.saved_path.is_some() {
        if notice.pending {
            "Opening…"
        } else {
            "Show in Folder"
        }
    } else if notice.pending {
        "Saving…"
    } else {
        "Save file"
    };
    let button = egui::Rect::from_min_size(egui::pos2(246., 76.), egui::vec2(116., 28.));
    ui.scope_builder(egui::UiBuilder::new().max_rect(button), |ui| {
        if ui
            .add_enabled(
                !notice.pending,
                egui::Button::new(label).min_size(button.size()),
            )
            .clicked()
        {
            action = Some(match &notice.saved_path {
                Some(path) => Action::Reveal(notice.guard.clone(), path.clone()),
                None => Action::Save(notice.guard.clone()),
            });
        }
    });
    let close = egui::Rect::from_min_size(egui::pos2(SIZE.x - 30., 6.), egui::vec2(24., 24.));
    let response = ui
        .put(close, egui::Button::new("×").frame(false))
        .on_hover_text("Dismiss recording notice");
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Dismiss recording notice")
    });
    if response.clicked() {
        action = Some(Action::Dismiss(notice.guard.clone()));
    }
    action
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn timeout_is_exactly_15_2_seconds_and_does_not_delete_identity() {
        let now = Instant::now();
        let n = Notice::new("id".into(), 7, now);
        assert!(!n.expired(now + LIFETIME - Duration::from_nanos(1)));
        assert!(n.expired(now + LIFETIME));
        assert_eq!(n.guard.artifact_id, "id");
    }
    #[test]
    fn pending_gates_duplicates_and_suspends_expiry() {
        let now = Instant::now();
        let mut n = Notice::new("id".into(), 1, now);
        assert!(n.begin_save().is_some());
        assert!(n.begin_save().is_none());
        assert!(!n.expired(now + LIFETIME * 2));
        assert_eq!(n.remaining(now), None);
    }
    #[test]
    fn stale_reply_cannot_mutate_replacement_with_same_artifact() {
        let now = Instant::now();
        let mut old = Notice::new("same".into(), 1, now);
        let g = old.begin_save().unwrap();
        let mut new = Notice::new("same".into(), 2, now);
        let current = new.begin_save().unwrap();
        assert!(!new.save_result(&g, Ok("wrong".into()), now));
        assert!(new.saved_path.is_none());
        assert!(new.pending);
        assert!(new.save_result(&current, Ok("right".into()), now));
        assert_eq!(new.saved_path, Some("right".into()));
    }
    #[test]
    fn error_allows_retry_and_result_restarts_full_lifetime() {
        let now = Instant::now();
        let mut n = Notice::new("id".into(), 1, now);
        let g = n.begin_save().unwrap();
        assert!(n.save_result(&g, Err("disk full".into()), now + Duration::from_secs(20)));
        assert!(n.error.as_deref().unwrap().contains("disk full"));
        assert!(!n.expired(now + Duration::from_secs(20) + LIFETIME - Duration::from_nanos(1)));
        assert!(n.expired(now + Duration::from_secs(20) + LIFETIME));
        let retry = n.begin_save().unwrap();
        let completed = now + Duration::from_secs(40);
        assert!(n.save_result(&retry, Ok("/exports/actual.mp4".into()), completed));
        assert_eq!(n.saved_path, Some("/exports/actual.mp4".into()));
        assert!(n.error.is_none());
        assert!(!n.expired(completed + LIFETIME - Duration::from_nanos(1)));
        assert!(n.expired(completed + LIFETIME));
    }
}
