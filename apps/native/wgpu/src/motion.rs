//! Paints shipping motion (`captures_app::motion`) in egui.
//!
//! Poses are visual only: like the other egui visual transforms they do not
//! move hit targets, and the entrance offsets are a few points. Blur has no
//! egui equivalent and is omitted. Callers request repaints only while an
//! animation runs, so an idle surface schedules nothing.

use std::time::Instant;

use captures_app::motion::{Pose, Tween};
use eframe::egui::{self, emath::TSTransform};

/// Milliseconds since `since`, for sampling an animation.
pub fn elapsed_ms(since: Instant, now: Instant) -> f64 {
    now.saturating_duration_since(since).as_secs_f64() * 1000.
}

/// CSS `transform: translateY() scale()` about the centre of `rect`.
pub fn transform(pose: Pose, rect: egui::Rect) -> TSTransform {
    let centre = rect.center().to_vec2();
    TSTransform::from_translation(centre + egui::vec2(0., pose.translate_y as f32))
        * TSTransform::from_scaling(pose.scale as f32)
        * TSTransform::from_translation(-centre)
}

/// Paint `add_contents` with `pose` applied to everything it draws in this
/// layer. The wrapper's child scope is always present, resting or not, so
/// widget ids (and a press that spans the end of an animation) stay stable.
pub fn with_pose<R>(
    ui: &mut egui::Ui,
    pose: Pose,
    rect: egui::Rect,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    ui.with_visual_transform(transform(pose, rect), |ui| {
        if pose.opacity < 1. {
            ui.multiply_opacity(pose.opacity.clamp(0., 1.) as f32);
        }
        add_contents(ui)
    })
    .inner
}

fn reduced_id() -> egui::Id {
    egui::Id::unique("captures-reduced-motion")
}

/// Publish the effective reduced-motion preference for this frame, so widgets
/// deep in a surface can honour it without threading a flag through.
pub fn set_reduced(ctx: &egui::Context, reduced: bool) {
    ctx.data_mut(|data| data.insert_temp(reduced_id(), reduced));
}

pub fn reduced(ctx: &egui::Context) -> bool {
    ctx.data(|data| data.get_temp(reduced_id()).unwrap_or(false))
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Slide {
    from: egui::Rect,
    to: egui::Rect,
    started: f64,
}

impl Slide {
    fn at(&self, now: f64, tween: &Tween, reduced: bool) -> egui::Rect {
        let t = tween.progress((now - self.started) * 1000., reduced) as f32;
        egui::Rect::from_min_max(
            self.from.min.lerp(self.to.min, t),
            self.from.max.lerp(self.to.max, t),
        )
    }
}

/// A shipping sliding indicator (`.capture-segmented-indicator`): one shape
/// that moves and resizes to the selected segment with a transition instead
/// of jumping. Reserve its paint slot before the segments so it sits under
/// them, then [`Self::finish`] with the selected segment's rect. Positions are
/// kept relative to `origin`, so moving the whole control does not animate.
pub struct SlidingIndicator {
    id: egui::Id,
    shape: egui::layers::ShapeIdx,
    origin: egui::Pos2,
}

impl SlidingIndicator {
    pub fn begin(ui: &mut egui::Ui, id: egui::Id, origin: egui::Pos2) -> Self {
        Self {
            id,
            shape: ui.painter().add(egui::Shape::Noop),
            origin,
        }
    }

    /// Paint at the animated rect; the first placement does not animate, like
    /// shipping's `.ready` class. Returns the painted rect.
    pub fn finish(
        self,
        ui: &egui::Ui,
        target: egui::Rect,
        tween: &Tween,
        paint: impl FnOnce(egui::Rect) -> egui::Shape,
    ) -> egui::Rect {
        let now = ui.input(|input| input.time);
        let reduced = reduced(ui.ctx());
        let offset = self.origin.to_vec2();
        let target = target.translate(-offset);
        let previous: Option<Slide> = ui.data(|data| data.get_temp(self.id));
        let slide = match previous {
            Some(slide) if slide.to == target => slide,
            Some(slide) => Slide {
                from: slide.at(now, tween, reduced),
                to: target,
                started: now,
            },
            None => Slide {
                from: target,
                to: target,
                started: f64::NEG_INFINITY,
            },
        };
        ui.data_mut(|data| data.insert_temp(self.id, slide));
        if tween.running((now - slide.started) * 1000., reduced) {
            ui.ctx().request_repaint();
        }
        let rect = slide.at(now, tween, reduced).translate(offset);
        ui.painter().set(self.shape, paint(rect));
        rect
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transform_scales_about_the_centre_and_moves_down_for_positive_y() {
        let rect = egui::Rect::from_min_size(egui::pos2(10., 20.), egui::vec2(100., 40.));
        let pose = Pose {
            opacity: 0.5,
            translate_y: 8.,
            scale: 0.5,
            blur: 0.,
        };
        let t = transform(pose, rect);
        assert_eq!(t * rect.center(), rect.center() + egui::vec2(0., 8.));
        assert_eq!(t * rect.min, egui::pos2(35., 38.));
        assert_eq!(transform(Pose::REST, rect), TSTransform::IDENTITY);
    }

    #[test]
    fn widget_ids_do_not_change_when_an_animation_settles() {
        let ctx = egui::Context::default();
        let moving = Pose {
            opacity: 0.2,
            translate_y: 6.,
            scale: 0.98,
            blur: 0.,
        };
        let mut ids = Vec::new();
        for pose in [moving, Pose::REST] {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                let rect = ui.max_rect();
                let id = with_pose(ui, pose, rect, |ui| ui.button("steady").id);
                ids.push(id);
            });
            output.textures_delta.clear();
        }
        assert_eq!(ids[0], ids[1]);
    }

    #[test]
    fn sliding_indicator_eases_to_a_new_segment_and_snaps_when_reduced() {
        let tokens = &crate::tokens::load()["dark-mustard"];
        let tween = captures_app::motion::Transition::SegmentedIndicator
            .resolve(tokens)
            .unwrap();
        let left = egui::Rect::from_min_size(egui::pos2(10., 10.), egui::vec2(40., 20.));
        let right = egui::Rect::from_min_size(egui::pos2(60., 10.), egui::vec2(80., 20.));
        let ctx = egui::Context::default();
        let frame = |target: egui::Rect, time: f64, reduced: bool| {
            let mut painted = egui::Rect::NOTHING;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    time: Some(time),
                    ..Default::default()
                },
                |ui| {
                    set_reduced(ui.ctx(), reduced);
                    let id = egui::Id::unique("indicator-test");
                    let indicator = SlidingIndicator::begin(ui, id, egui::Pos2::ZERO);
                    painted = indicator.finish(ui, target, &tween, |rect| {
                        egui::Shape::rect_filled(rect, 0., egui::Color32::WHITE)
                    });
                },
            );
            output.textures_delta.clear();
            let repaint = output
                .viewport_output
                .values()
                .any(|viewport| viewport.repaint_delay.is_zero());
            (painted, repaint)
        };
        assert_eq!(
            frame(left, 0., false).0,
            left,
            "first placement does not slide"
        );
        let (start, moving) = frame(right, 1., false);
        assert_eq!(start, left);
        assert!(moving, "frames only while sliding");
        let (middle, _) = frame(right, 1.14, false);
        assert!(middle.left() > left.left() && middle.left() < right.left());
        assert!(middle.width() > left.width() && middle.width() < right.width());
        assert_eq!(frame(right, 1.28, false).0, right);
        assert_eq!(frame(left, 2., true).0, left, "reduced motion snaps");
    }
}
