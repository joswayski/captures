use gpui::{Bounds, Pixels, point, px, size};

pub const FRAME_WIDTH: f32 = 340.;
pub const CARD_WIDTH: f32 = 284.;
pub const CARD_HEIGHT: f32 = 160.;
pub const GAP: f32 = 24.;
pub const PADDING: f32 = 28.;
pub const CONTROL_GUTTER: f32 = 52.;
pub const SLOT: f32 = CARD_HEIGHT + GAP;
const SCREEN_GAP: f32 = 12.;
const POSE_K: f32 = 24.;
const RECEDING_STEP: f32 = 0.55;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Anchor {
    Top,
    Bottom,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Side {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Placement {
    pub anchor: Anchor,
    pub side: Side,
}

impl Placement {
    pub fn parse(value: &str) -> Self {
        Self {
            anchor: if value.starts_with("top") {
                Anchor::Top
            } else {
                Anchor::Bottom
            },
            side: if value.ends_with("right") {
                Side::Right
            } else {
                Side::Left
            },
        }
    }
}

pub fn pose_depth(depth: usize) -> f32 {
    let depth = depth as f32;
    if depth == 0. {
        0.
    } else {
        depth * (POSE_K + RECEDING_STEP * depth) / (depth + POSE_K)
    }
}

pub fn expanded_height(count: usize, available: f32) -> f32 {
    expanded_height_at(count as f32, available)
}

fn expanded_height_at(count: f32, available: f32) -> f32 {
    let cards = count.max(1.);
    (PADDING + CONTROL_GUTTER + cards * CARD_HEIGHT + (cards - 1.) * GAP)
        .min(available.max(CARD_HEIGHT + CONTROL_GUTTER))
}

pub fn window_bounds(display: Bounds<Pixels>, placement: Placement) -> Bounds<Pixels> {
    let available_height = (f32::from(display.size.height) - 2. * SCREEN_GAP).max(1.);
    let x = match placement.side {
        Side::Left => display.origin.x,
        Side::Right => display.origin.x + display.size.width - px(FRAME_WIDTH),
    };
    Bounds::new(
        point(x, display.origin.y + px(SCREEN_GAP)),
        size(px(FRAME_WIDTH), px(available_height)),
    )
}

pub fn expanded_card_top(index: usize, count: usize, frame_height: f32, anchor: Anchor) -> f32 {
    let stack_height = expanded_height(count, frame_height);
    let stack_top = match anchor {
        Anchor::Top => 0.,
        Anchor::Bottom => frame_height - stack_height,
    };
    stack_top
        + match anchor {
            Anchor::Top => CONTROL_GUTTER + index as f32 * SLOT,
            Anchor::Bottom => PADDING + index as f32 * SLOT,
        }
}

pub fn expanded_visual_index(index: usize, count: usize, anchor: Anchor) -> usize {
    match anchor {
        Anchor::Top => count.saturating_sub(index + 1),
        Anchor::Bottom => index,
    }
}

/// Continuous held-slot removal, including stacks taller than their window.
pub fn settling_card_top(index: usize, progress: &[f32], frame_height: f32, anchor: Anchor) -> f32 {
    let count = progress.len();
    let original = expanded_card_top(
        expanded_visual_index(index, count, anchor),
        count,
        frame_height,
        anchor,
    );
    let before: f32 = match anchor {
        Anchor::Top => progress[index + 1..].iter().sum(),
        Anchor::Bottom => progress[..index].iter().sum(),
    };
    let stack_shift = match anchor {
        Anchor::Top => 0.,
        Anchor::Bottom => {
            expanded_height(count, frame_height)
                - expanded_height_at(count as f32 - progress.iter().sum::<f32>(), frame_height)
        }
    };
    original + stack_shift - before * SLOT
}

pub fn collapsed_card_top(depth: usize, frame_height: f32, anchor: Anchor, hovered: bool) -> f32 {
    let peek = pose_depth(depth) * if hovered { 16. } else { 13. };
    match anchor {
        Anchor::Top => CONTROL_GUTTER + peek,
        Anchor::Bottom => frame_height - CONTROL_GUTTER - CARD_HEIGHT - peek,
    }
}

pub fn collapse_progress(elapsed_seconds: f32, collapsing: bool, reduced_motion: bool) -> f32 {
    if reduced_motion {
        return if collapsing { 1. } else { 0. };
    }
    let linear = (elapsed_seconds / 0.52).clamp(0., 1.);
    let eased = 1. - (1. - linear).powi(3);
    if collapsing { eased } else { 1. - eased }
}

pub fn mix(from: f32, to: f32, progress: f32) -> f32 {
    from + (to - from) * progress.clamp(0., 1.)
}

pub fn rejection_x(elapsed: f32) -> f32 {
    let t = (elapsed / 0.42).clamp(0., 1.);
    let keys = [
        (0., 0.),
        (0.10, -9.),
        (0.30, 9.),
        (0.50, -6.),
        (0.65, 4.),
        (0.80, -2.),
        (0.90, 2.),
        (1., 0.),
    ];
    for pair in keys.windows(2) {
        if t <= pair[1].0 {
            let mix = (t - pair[0].0) / (pair[1].0 - pair[0].0);
            return self::mix(pair[0].1, pair[1].1, mix);
        }
    }
    0.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_corner_placement_uses_display_edges_and_reserved_vertical_gap() {
        let display = Bounds::new(point(px(-1920.), px(40.)), size(px(1920.), px(1080.)));
        let top_left = window_bounds(display, Placement::parse("top_left"));
        let bottom_right = window_bounds(display, Placement::parse("bottom_right"));
        assert_eq!(top_left.origin, point(px(-1920.), px(52.)));
        assert_eq!(bottom_right.origin, point(px(-340.), px(52.)));
        assert_eq!(top_left.size, size(px(340.), px(1056.)));
    }

    #[test]
    fn deep_piles_recede_smoothly_without_a_depth_cliff() {
        let steps = (1..20)
            .map(|depth| pose_depth(depth) - pose_depth(depth - 1))
            .collect::<Vec<_>>();
        assert!(steps.windows(2).all(|pair| pair[1] <= pair[0]));
        assert!(steps.iter().all(|step| *step > RECEDING_STEP));
        assert!(pose_depth(8) > pose_depth(4));
    }

    #[test]
    fn asymmetric_anchors_put_the_newest_card_at_the_correct_edge() {
        let frame = 800.;
        assert_eq!(expanded_card_top(0, 3, frame, Anchor::Top), 52.);
        assert_eq!(expanded_card_top(0, 3, frame, Anchor::Bottom), 220.);
        assert_eq!(expanded_card_top(2, 3, frame, Anchor::Bottom), 588.);
        assert_eq!(collapsed_card_top(0, frame, Anchor::Top, false), 52.);
        assert_eq!(collapsed_card_top(0, frame, Anchor::Bottom, false), 588.);
        assert_eq!(expanded_visual_index(2, 3, Anchor::Top), 0);
        assert_eq!(expanded_visual_index(2, 3, Anchor::Bottom), 2);
    }

    #[test]
    fn settle_is_continuous_mirrored_and_rebases_without_a_jump() {
        assert_eq!(
            settling_card_top(0, &[0., 0., 0.5], 800., Anchor::Bottom),
            312.
        );
        assert_eq!(
            settling_card_top(0, &[0., 0., 0.5], 800., Anchor::Top),
            328.
        );
        for anchor in [Anchor::Top, Anchor::Bottom] {
            assert_eq!(
                settling_card_top(0, &[0., 0., 1.], 800., anchor),
                settling_card_top(0, &[0., 0.], 800., anchor)
            );
            // Removing an older card must not move the newest one.
            assert_eq!(
                settling_card_top(2, &[0.5, 0., 0.], 800., anchor),
                settling_card_top(2, &[0., 0., 0.], 800., anchor)
            );
        }
        // With overflow, the stack height remains clamped until enough space opens.
        assert_eq!(
            settling_card_top(0, &[0., 0., 0.25], 400., Anchor::Bottom),
            28.
        );
    }

    #[test]
    fn reduced_motion_commits_both_transition_directions_immediately() {
        assert_eq!(collapse_progress(0., true, true), 1.);
        assert_eq!(collapse_progress(0., false, true), 0.);
        assert!(collapse_progress(0.26, true, false) > 0.5);
    }

    #[test]
    fn rejection_shake_returns_to_origin_and_is_asymmetric() {
        assert_eq!(rejection_x(0.), 0.);
        assert_eq!(rejection_x(0.42), 0.);
        assert!(rejection_x(0.042) < 0.);
        assert!(rejection_x(0.126) > 0.);
    }
}
