use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use eframe::egui::{self, RichText, Stroke};

use crate::tokens::Tokens;

pub const SIZE: egui::Vec2 = egui::vec2(440.0, 116.0);
pub const LIFETIME: Duration = Duration::from_millis(15_200);
/// Shipping closes the window 200 ms after its 15 s `recording-saved-lifecycle`
/// animation ends, so the exit fade finishes this long before expiry.
const CLOSE_AFTER_ANIMATION_MS: f64 = 200.;

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
    shown_at: Instant,
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
            shown_at: now,
            expires_at: now + LIFETIME,
        }
    }

    /// Shipping lifecycle pose: arrive, hold, then fade out ending just before
    /// the window closes. A save that extends the life holds steady instead of
    /// replaying the entrance. Returns whether frames are still changing.
    pub fn pose(
        &self,
        animation: &captures_app::motion::Animation,
        now: Instant,
        reduced_motion: bool,
    ) -> (captures_app::motion::Pose, bool) {
        let since = crate::motion::elapsed_ms(self.shown_at, now);
        let until = self
            .remaining(now)
            .map(|remaining| (remaining.as_secs_f64() * 1000. - CLOSE_AFTER_ANIMATION_MS).max(0.));
        (
            animation.lifecycle_pose(since, until, reduced_motion),
            animation.lifecycle_running(since, until, reduced_motion),
        )
    }

    /// When the exit fade next needs frames while the notice holds still.
    pub fn exit_wake(
        &self,
        animation: &captures_app::motion::Animation,
        now: Instant,
    ) -> Option<Duration> {
        let remaining = self.remaining(now)?;
        let until = (remaining.as_secs_f64() * 1000. - CLOSE_AFTER_ANIMATION_MS).max(0.);
        Some(Duration::from_secs_f64(
            animation.lifecycle_exit_in(until) / 1000.,
        ))
    }

    pub fn begin_save(&mut self) -> Option<Guard> {
        if self.pending || self.saved_path.is_some() {
            return None;
        }
        self.pending = true;
        self.error = None;
        Some(self.guard.clone())
    }

    pub fn title(&self) -> &'static str {
        if self.error.is_some() {
            if self.saved_path.is_some() {
                "Could not show recording"
            } else {
                "Could not save recording"
            }
        } else if self.pending {
            "Saving recording"
        } else if self.saved_path.is_some() {
            "Recording saved"
        } else {
            "Recording ready"
        }
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

impl Notice {
    /// Shipping `.recording-saved-copy p`: the error, or the saved/ready line.
    pub fn detail(&self) -> &str {
        self.error
            .as_deref()
            .unwrap_or(if self.saved_path.is_some() {
                "Saved to your Captures folder."
            } else {
                "Kept in Capture History for 30 days. Save a copy anytime."
            })
    }

    /// Shipping `.recording-saved-reveal`: its label and leading icon.
    pub fn primary(&self) -> (&'static str, &'static str) {
        match (self.saved_path.is_some(), self.pending) {
            (true, true) => ("Opening…", "folder"),
            (true, false) => ("Show in Folder", "folder"),
            (false, true) => ("Saving…", "save"),
            (false, false) => ("Save file", "save"),
        }
    }
}

/// Shipping `.recording-saved-notice` geometry: one row of a 38 px check
/// tile, the copy and the action button (`grid-template-columns: 38px 1fr
/// auto`, `gap` and padding `--s-5`, 38 px right padding for the dismiss ×).
pub struct Layout {
    pub tile: egui::Rect,
    pub copy_left: f32,
    pub button: egui::Rect,
    pub dismiss: egui::Rect,
}

pub fn layout(tokens: &Tokens, button_width: f32) -> Layout {
    let padding = tokens.number("s-5");
    let gap = tokens.number("s-5");
    let height = tokens.number("h-md");
    let tile = egui::Rect::from_min_size(
        egui::pos2(padding, (SIZE.y - 38.) / 2.),
        egui::Vec2::splat(38.),
    );
    let button = egui::Rect::from_min_size(
        egui::pos2(SIZE.x - 38. - button_width, (SIZE.y - height) / 2.),
        egui::vec2(button_width, height),
    );
    // `.recording-saved-dismiss { top: 10px; right: 10px; 24 × 24 }`.
    let dismiss = egui::Rect::from_min_size(egui::pos2(SIZE.x - 34., 10.), egui::Vec2::splat(24.));
    Layout {
        tile,
        copy_left: tile.right() + gap,
        button,
        dismiss,
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
    let semibold = egui::FontFamily::Name("semibold".into());
    let (label, icon) = notice.primary();
    let button_font = egui::FontId::new(tokens.number("text-xs"), semibold.clone());
    let label_width = ui
        .painter()
        .layout_no_wrap(
            label.into(),
            button_font.clone(),
            tokens.color("glass-text"),
        )
        .size()
        .x;
    // `padding: 0 var(--s-4)`, a 14 px icon, `gap: var(--s-3)` and a 1 px border.
    let button_width =
        (2. * tokens.number("s-4") + 14. + tokens.number("s-3") + label_width + 2.).ceil();
    let layout = layout(tokens, button_width);

    // `.recording-saved-icon`: a positive tile with a 20 px, 2.2-stroke check.
    ui.painter()
        .rect_filled(layout.tile, tokens.number("r-lg"), tokens.color("positive"));
    crate::capture_controls::paint_icon(
        ui.painter(),
        "check",
        egui::Rect::from_center_size(layout.tile.center(), egui::Vec2::splat(20.)),
        2.2,
        tokens.color("positive-ink"),
    );

    let copy_width = (layout.button.left() - tokens.number("s-5") - layout.copy_left).max(0.);
    let title_font = egui::FontId::new(tokens.number("text-md"), semibold);
    let title = ui.painter().layout_no_wrap(
        notice.title().into(),
        title_font.clone(),
        tokens.color("glass-text"),
    );
    let detail_font = egui::FontId::proportional(tokens.number("text-xs"));
    let detail_height = ui
        .painter()
        .layout(
            notice.detail().into(),
            detail_font.clone(),
            tokens.color("glass-text-muted"),
            copy_width,
        )
        .size()
        .y;
    let padding = tokens.number("s-5");
    let max_detail = SIZE.y - 2. * padding - title.size().y - 2.;
    let copy_height = title.size().y + 2. + detail_height.min(max_detail);
    let copy = egui::Rect::from_min_size(
        egui::pos2(layout.copy_left, (SIZE.y - copy_height) / 2.),
        egui::vec2(copy_width, copy_height),
    );
    ui.scope_builder(egui::UiBuilder::new().max_rect(copy), |ui| {
        ui.spacing_mut().item_spacing.y = 2.;
        ui.add(egui::Label::new(
            RichText::new(notice.title())
                .font(title_font)
                .color(tokens.color("glass-text")),
        ));
        crate::primitives::glass_scroll_area(
            ui,
            tokens,
            egui::ScrollArea::vertical()
                .id_salt("recording-notice-detail")
                .max_height(max_detail)
                .min_scrolled_height(0.),
            |ui| {
                ui.set_width(copy_width);
                ui.label(
                    RichText::new(notice.detail())
                        .font(detail_font.clone())
                        .color(tokens.color("glass-text-muted")),
                )
                .on_hover_text(notice.error.as_deref().unwrap_or(""));
            },
        );
    });

    let glyph = egui::IdSalt::new("recording-saved-reveal-icon");
    let ink = tokens.color("glass-text");
    ui.scope_builder(egui::UiBuilder::new().max_rect(layout.button), |ui| {
        ui.spacing_mut().button_padding = egui::vec2(tokens.number("s-4"), 0.);
        let visuals = &mut ui.visuals_mut().widgets;
        visuals.inactive.weak_bg_fill = tokens.color("glass-hover");
        visuals.hovered.weak_bg_fill = tokens.color("glass-active");
        visuals.active.weak_bg_fill = tokens.color("glass-active");
        let button = egui::Button::new((
            egui::Atom::custom(glyph, egui::Vec2::splat(14.)),
            RichText::new(label).font(button_font).color(ink),
        ))
        .gap(tokens.number("s-3"))
        .wrap_mode(egui::TextWrapMode::Extend)
        .stroke(Stroke::new(1., tokens.color("glass-border-strong")))
        .corner_radius(tokens.number("r-md"))
        .min_size(layout.button.size());
        let shown = ui
            .add_enabled_ui(!notice.pending, |ui| {
                let shown = button.atom_ui(ui);
                if let Some(rect) = shown.rect(glyph) {
                    crate::capture_controls::paint_icon(ui.painter(), icon, rect, 1.8, ink);
                }
                shown
            })
            .inner;
        if shown.response.clicked() {
            action = Some(match &notice.saved_path {
                Some(path) => Action::Reveal(notice.guard.clone(), path.clone()),
                None => Action::Save(notice.guard.clone()),
            });
        }
    });

    // `.recording-saved-dismiss`: a quiet 24 px ×.
    let response = ui
        .interact(
            layout.dismiss,
            ui.unique_id().with("recording-saved-dismiss"),
            egui::Sense::click(),
        )
        .on_hover_text("Dismiss recording notice");
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Dismiss recording notice")
    });
    let hovered = response.hovered();
    if hovered {
        ui.painter().rect_filled(
            layout.dismiss,
            tokens.number("r-sm"),
            tokens.color("glass-hover"),
        );
    }
    if response.has_focus() {
        crate::primitives::focus_ring(ui, tokens, layout.dismiss, tokens.number("r-sm"));
    }
    crate::capture_controls::paint_icon(
        ui.painter(),
        "close",
        egui::Rect::from_center_size(layout.dismiss.center(), egui::Vec2::splat(14.)),
        1.8,
        tokens.color(if hovered {
            "glass-text"
        } else {
            "glass-text-subtle"
        }),
    );
    if response.clicked() {
        action = Some(Action::Dismiss(notice.guard.clone()));
    }
    action
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn notice_is_one_row_of_check_tile_copy_and_icon_button_with_a_dismiss_x() {
        let tokens = &crate::tokens::load()["dark-mustard"];
        let layout = layout(tokens, 100.);
        let centre = SIZE.y / 2.;
        assert_eq!(layout.tile.size(), egui::Vec2::splat(38.));
        assert_eq!(layout.tile.left(), tokens.number("s-5"));
        assert!((layout.tile.center().y - centre).abs() < 0.5);
        assert!((layout.button.center().y - centre).abs() < 0.5);
        assert_eq!(layout.button.height(), tokens.number("h-md"));
        assert_eq!(layout.button.right(), SIZE.x - 38.);
        assert!(layout.copy_left + 100. < layout.button.left());
        assert_eq!(
            layout.dismiss,
            egui::Rect::from_min_size(egui::pos2(SIZE.x - 34., 10.), egui::Vec2::splat(24.))
        );
        assert!(layout.dismiss.left() >= layout.button.right());

        let mut notice = Notice::new("id".into(), 1, Instant::now());
        assert_eq!(notice.primary(), ("Save file", "save"));
        notice.pending = true;
        assert_eq!(notice.primary(), ("Saving…", "save"));
        notice.pending = false;
        notice.saved_path = Some("/saved.mp4".into());
        assert_eq!(notice.primary(), ("Show in Folder", "folder"));
        assert_eq!(notice.detail(), "Saved to your Captures folder.");
    }

    #[test]
    fn timeout_is_exactly_15_2_seconds_and_does_not_delete_identity() {
        let now = Instant::now();
        let n = Notice::new("id".into(), 7, now);
        assert!(!n.expired(now + LIFETIME - Duration::from_nanos(1)));
        assert!(n.expired(now + LIFETIME));
        assert_eq!(n.guard.artifact_id, "id");
    }
    #[test]
    fn lifecycle_arrives_holds_and_fades_before_the_window_closes() {
        let tokens = &crate::tokens::load()["dark-mustard"];
        let lifecycle = tokens.motion(captures_app::motion::Motion::RecordingSavedLifecycle);
        let now = Instant::now();
        let mut n = Notice::new("id".into(), 1, now);
        let (start, running) = n.pose(&lifecycle, now, false);
        assert_eq!(start.opacity, 0.);
        assert!(running);
        let (steady, running) = n.pose(&lifecycle, now + Duration::from_secs(5), false);
        assert_eq!(steady, captures_app::motion::Pose::REST);
        assert!(!running, "the steady middle schedules no frames");
        let wake = n
            .exit_wake(&lifecycle, now + Duration::from_secs(5))
            .unwrap();
        assert_eq!(wake.as_millis(), 7_900);
        let (fading, running) = n.pose(&lifecycle, now + Duration::from_secs(14), false);
        assert!(fading.opacity > 0. && fading.opacity < 1.);
        assert!(running);
        let (gone, _) = n.pose(&lifecycle, now + Duration::from_millis(15_000), false);
        assert_eq!(gone.opacity, 0.);
        // Reduced motion holds still and visible; the window close removes it.
        let (reduced, running) = n.pose(&lifecycle, now, true);
        assert_eq!(reduced, captures_app::motion::Pose::REST);
        assert!(!running);
        // Pending holds steady; a completed save restarts only the hold.
        assert!(n.begin_save().is_some());
        let later = now + Duration::from_secs(14);
        assert_eq!(
            n.pose(&lifecycle, later, false).0,
            captures_app::motion::Pose::REST
        );
        assert!(n.mark_saved("id", "/saved.mp4".into(), later));
        assert_eq!(
            n.pose(&lifecycle, later, false).0,
            captures_app::motion::Pose::REST
        );
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
