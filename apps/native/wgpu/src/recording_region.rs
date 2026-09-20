//! Passive region guide. Every painted pixel stays outside the recorded area,
//! including on X11 where the compositor cannot exclude capture windows.
use captures_recording::CaptureRect;
use eframe::egui::{self, Color32, Mesh, Pos2, Rect};

use crate::tokens::Tokens;

fn outside(bounds: Rect, hole: Rect) -> [Rect; 4] {
    let hole = hole.intersect(bounds);
    [
        Rect::from_min_max(bounds.min, Pos2::new(bounds.max.x, hole.min.y)),
        Rect::from_min_max(Pos2::new(bounds.min.x, hole.max.y), bounds.max),
        Rect::from_min_max(
            Pos2::new(bounds.min.x, hole.min.y),
            Pos2::new(hole.min.x, hole.max.y),
        ),
        Rect::from_min_max(
            Pos2::new(hole.max.x, hole.min.y),
            Pos2::new(bounds.max.x, hole.max.y),
        ),
    ]
}

fn hole(rect: CaptureRect, pixels_per_point: f32) -> Rect {
    // Round outward, never inward, on fractional-scale displays.
    let min = egui::pos2(rect.x as f32, rect.y as f32);
    let max = min + egui::vec2(rect.width as f32, rect.height as f32);
    Rect::from_min_max(
        (min * pixels_per_point).floor() / pixels_per_point,
        (max * pixels_per_point).ceil() / pixels_per_point,
    )
}

pub fn show(ui: &egui::Ui, tokens: &Tokens, rect: CaptureRect) {
    let bounds = ui.input(|input| input.content_rect());
    let hole = hole(rect, ui.ctx().pixels_per_point()).intersect(bounds);
    let mut mesh = Mesh::default();
    let mut fill = |bounds: Rect, color: Color32| {
        for rect in outside(bounds, hole) {
            if rect.is_positive() {
                // Plain quads, not antialiased strokes: no inner-edge fringe
                // may leak into the recording's transparent hole.
                mesh.add_colored_rect(rect, color);
            }
        }
    };
    fill(bounds, tokens.color("glass-veil"));
    fill(
        hole.expand(tokens.number("s-1")).intersect(bounds),
        tokens.color("theme-accent"),
    );
    ui.painter().add(egui::Shape::mesh(mesh));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guide_quads_never_enter_the_region_at_edges_or_fractional_scale() {
        let bounds = Rect::from_min_size(Pos2::ZERO, egui::vec2(1024., 768.));
        for scale in [1., 1.25, 1.5, 2.] {
            for rect in [
                CaptureRect {
                    x: 101,
                    y: 53,
                    width: 317,
                    height: 179,
                },
                CaptureRect {
                    x: 0,
                    y: 0,
                    width: 1024,
                    height: 768,
                },
                CaptureRect {
                    x: 813,
                    y: 645,
                    width: 211,
                    height: 123,
                },
            ] {
                let exact = Rect::from_min_size(
                    egui::pos2(rect.x as f32, rect.y as f32),
                    egui::vec2(rect.width as f32, rect.height as f32),
                );
                let hole = hole(rect, scale);
                assert!(hole.contains_rect(exact));
                let quads = outside(bounds, hole);
                for painted in quads {
                    assert!(!painted.intersect(exact).is_positive());
                }
                let painted_area: f32 = quads.iter().map(Rect::area).sum();
                assert!((painted_area + hole.area() - bounds.area()).abs() < 0.1);
            }
        }
    }
}
