//! Paints shipping motion (`captures_app::motion`) in egui.
//!
//! Poses are visual only: like the other egui visual transforms they do not
//! move hit targets, and the entrance offsets are a few points. Blur has no
//! egui equivalent and is omitted. Callers request repaints only while an
//! animation runs, so an idle surface schedules nothing.

use std::time::Instant;

use captures_app::motion::Pose;
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
}
