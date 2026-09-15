//! Geometry shared by the GPUI preview renderer and its X11 input shape.

pub const FRAME_WIDTH: f32 = 340.0;
pub const FRAME_HEIGHT: f32 = 760.0;
pub const CARD_X: f32 = 28.0;
pub const CARD_WIDTH: f32 = 284.0;
pub const CARD_HEIGHT: f32 = 160.0;
pub const CARD_GAP: f32 = 24.0;
pub const CARD_SLOT: f32 = CARD_HEIGHT + CARD_GAP;
pub const STACK_GUTTER: f32 = 52.0;
pub const STACK_PADDING: f32 = 28.0;
pub const VISIBLE_CARDS: usize = 3;

pub const ARRIVE_MS: f32 = 520.0;
pub const STACK_MOTION_MS: f32 = 520.0;
pub const DISMISS_MS: f32 = 1_030.0;
pub const DELETE_MS: f32 = 2_900.0;
pub const DROP_REJECT_MS: f32 = 420.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub opacity: f32,
    pub rotation: f32,
}

pub fn ease_out_cubic(value: f32) -> f32 {
    let t = value.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

pub fn smooth(value: f32) -> f32 {
    let t = value.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

pub fn pose_depth(depth: f32) -> f32 {
    let n = depth.max(0.0);
    n * (24.0 + 0.55 * n) / (n + 24.0)
}

pub fn peek_jitter(depth: usize) -> f32 {
    if depth == 0 {
        return 0.0;
    }
    let hashed = ((depth as u32).wrapping_mul(0x9e37_79b1) ^ 0x7f4a_7c15).wrapping_mul(0x85eb_ca6b);
    (hashed as f64 / 2_f64.powi(32) * 2.0 - 1.0) as f32 * 0.4 * 0.58_f32.powi(depth as i32 - 1)
}

pub fn layer_rotation(id: u64, depth: usize) -> f32 {
    if depth == 0 {
        return 0.0;
    }
    let hash = id.wrapping_mul(0x0100_0193) as u32;
    let sign = if hash & 1 == 0 { -1.0 } else { 1.0 };
    sign * (2.7 + (hash >> 1) as f32 / (u32::MAX >> 1) as f32 * 0.3)
}

pub fn collapsed_padding(count: usize) -> f32 {
    let depth = count.saturating_sub(1) as f32;
    STACK_GUTTER.max(pose_depth(depth) * 16.0 + STACK_PADDING)
}

/// Signed gravity is +1 at the bottom edge, 0 in the work-area center and -1
/// at the top. The card itself never jumps when gravity crosses an anchor.
pub fn collapsed_pose(id: u64, depth: usize, gravity: f32, hover: f32, sway: (f32, f32)) -> Pose {
    let gravity = gravity.clamp(-1.0, 1.0);
    let proximity = 1.0 - gravity.abs();
    let pile = pose_depth(depth as f32);
    let peek = 13.0 + 3.0 * hover;
    let scale_step = 0.025 - hover * 0.005;
    let scale = (1.0 - pile * scale_step).max(0.72);
    let front_y =
        STACK_GUTTER + (FRAME_HEIGHT - CARD_HEIGHT - 2.0 * STACK_GUTTER) * ((gravity + 1.0) * 0.5);
    let signed_peek = -pile * peek * gravity + peek_jitter(depth) * gravity;
    let trail = (pile / pose_depth(3.0).max(1.0)).clamp(0.0, 1.0);
    Pose {
        x: CARD_X + pile * (-0.8 + hover * 0.2) + sway.0 * trail,
        y: front_y + signed_peek + sway.1 * trail,
        width: CARD_WIDTH * scale,
        height: CARD_HEIGHT * scale,
        opacity: (1.0 - (pile - 2.1).max(0.0) * 0.15).max(0.28),
        rotation: layer_rotation(id, depth) * proximity * (1.0 - 0.28 * hover),
    }
}

pub fn expanded_pose(index: usize, count: usize, top: bool, scroll: usize) -> Pose {
    let visual = if top { count - 1 - index } else { index };
    let visible = visual >= scroll && visual < scroll + VISIBLE_CARDS;
    let local = visual as isize - scroll as isize;
    Pose {
        x: CARD_X,
        y: STACK_PADDING + local as f32 * CARD_SLOT,
        width: CARD_WIDTH,
        height: CARD_HEIGHT,
        opacity: if visible { 1.0 } else { 0.0 },
        rotation: 0.0,
    }
}

pub fn interpolate(from: Pose, to: Pose, progress: f32) -> Pose {
    let t = smooth(progress);
    let lerp = |a: f32, b: f32| a + (b - a) * t;
    Pose {
        x: lerp(from.x, to.x),
        y: lerp(from.y, to.y),
        width: lerp(from.width, to.width),
        height: lerp(from.height, to.height),
        opacity: lerp(from.opacity, to.opacity),
        rotation: lerp(from.rotation, to.rotation),
    }
}

pub fn gravity_from_y(card_y: f32, work_height: f32) -> f32 {
    let travel = (work_height - CARD_HEIGHT - 2.0 * STACK_GUTTER).max(1.0);
    (2.0 * ((card_y - STACK_GUTTER) / travel) - 1.0).clamp(-1.0, 1.0)
}

pub fn side_from_x(frame_x: f32, work_width: f32, was_right: bool) -> bool {
    let travel = (work_width - FRAME_WIDTH).max(1.0);
    let bias = (2.0 * frame_x / travel - 1.0).clamp(-1.0, 1.0);
    if was_right { bias > -0.2 } else { bias >= 0.2 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deep_pile_keeps_receding_without_cliff() {
        let steps: Vec<_> = (1..9)
            .map(|n| pose_depth(n as f32) - pose_depth((n - 1) as f32))
            .collect();
        assert!(steps.windows(2).all(|pair| pair[1] <= pair[0]));
        assert!(steps[7] > 0.55);
    }

    #[test]
    fn gravity_reverses_peek_while_center_adds_rotation() {
        let top = collapsed_pose(9, 2, -1.0, 0.0, (0.0, 0.0));
        let middle = collapsed_pose(9, 2, 0.0, 0.0, (0.0, 0.0));
        let bottom = collapsed_pose(9, 2, 1.0, 0.0, (0.0, 0.0));
        assert!(top.y > STACK_GUTTER);
        assert!(bottom.y < FRAME_HEIGHT - STACK_GUTTER - CARD_HEIGHT);
        assert_eq!(top.rotation, 0.0);
        assert_ne!(middle.rotation, 0.0);
    }

    #[test]
    fn sway_only_moves_rear_layers() {
        let front = collapsed_pose(1, 0, 1.0, 0.0, (3.0, 2.0));
        let rear = collapsed_pose(2, 3, 1.0, 0.0, (3.0, 2.0));
        assert_eq!(front.x, CARD_X);
        assert!(rear.x > collapsed_pose(2, 3, 1.0, 0.0, (0.0, 0.0)).x);
    }

    #[test]
    fn corner_hysteresis_does_not_flap_at_center() {
        assert!(!side_from_x(520.0, 1_400.0, false));
        assert!(side_from_x(520.0, 1_400.0, true));
    }

    #[test]
    fn expanded_stack_clips_both_sides_of_recent_window() {
        assert_eq!(expanded_pose(0, 6, false, 2).opacity, 0.0);
        assert_eq!(expanded_pose(2, 6, false, 2).opacity, 1.0);
        assert_eq!(expanded_pose(4, 6, false, 2).opacity, 1.0);
        assert_eq!(expanded_pose(5, 6, false, 2).opacity, 0.0);

        // Top anchoring reverses visual order but retains the same 3-card window.
        assert_eq!(expanded_pose(5, 6, true, 2).opacity, 0.0);
        assert_eq!(expanded_pose(3, 6, true, 2).opacity, 1.0);
    }
}
