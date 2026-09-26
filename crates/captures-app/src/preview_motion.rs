//! Preview-stack motion both native hosts share: card exits (the Close streak
//! and the dust delete), survivors settling into the hole, the Clear all
//! stagger, the stack toolbar's entrances and exits, dust particles and the
//! collapsed pile's sparkles.
//!
//! Ported from `apps/desktop/ui/src/lib/thumbnailLayout.ts`,
//! `lib/thumbnailExit.ts`, the `ThumbnailCard` exit flow in `App.tsx` and
//! `styles/mini-preview.css`. Timings and curves live in [`crate::motion`];
//! this module owns the rules and geometry so AppKit and wgpu cannot drift.
//! Hosts own drawing and clocks: every function takes elapsed milliseconds
//! on any monotonic clock.

use serde::Serialize;

use crate::motion::{CubicBezier, Motion, MotionTokens, Pose, Transition, Tween};
use crate::preview::THUMBNAIL_CARD_SLOT;

/// `THUMBNAIL_DISMISS_STACK_MOTION_DELAY_MS`: survivors start sliding once the
/// Close streak has left the card.
pub const DISMISS_SETTLE_DELAY_MS: f64 = 450.0;
/// `THUMBNAIL_DELETE_STACK_MOTION_DELAY_MS`: survivors wait for the ash phase.
pub const DELETE_SETTLE_DELAY_MS: f64 = 1_800.0;
/// `thumbnail-delete 2.9s`: a dust delete holds its slot this long.
pub const DUST_HOLD_MS: f64 = 2_900.0;
/// `THUMBNAIL_CLEAR_STAGGER_MS` / `_MAX_MS`: Clear all starts the bottom card
/// first and each card above it 36 ms later, capped at 180 ms.
pub const CLEAR_STAGGER_MS: f64 = 36.0;
pub const CLEAR_STAGGER_MAX_MS: f64 = 180.0;
/// `THUMBNAIL_DISSOLVE_WAVE_MS`: the ash front travels from the trash control
/// to the farthest corner in this long.
pub const DISSOLVE_WAVE_MS: f64 = 720.0;
/// `thumbnail-dust-clip 2.55s`: the dust layer's clip opens, then it fades.
pub const DUST_CLIP_MS: f64 = 2_550.0;
/// `--dust-pad`: dust may fly this far outside the card.
pub const DUST_LAYER_PAD: f64 = 120.0;
/// `THUMBNAIL_DUST_TARGET_CELL_PX` and `THUMBNAIL_DUST_MAX_PARTICLES`.
pub const DUST_TARGET_CELL: f64 = 11.0;
pub const DUST_MAX_PARTICLES: usize = 220;
/// Delete control centres (`THUMBNAIL_DELETE_ORIGIN_*`): the first control
/// before a folder save, beside Close after one.
pub const DELETE_ORIGIN_FIRST_X: f64 = 22.5;
pub const DELETE_ORIGIN_AFTER_CLOSE_X: f64 = 57.5;
pub const DELETE_ORIGIN_Y: f64 = 22.5;

/// How a card leaves the stack.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExitKind {
    /// Close, Clear all and an unsaved Delete's dismissal:
    /// [`Motion::PreviewDismiss`] with the [`Motion::PreviewDismissStreak`]
    /// on the media.
    Dismiss,
    /// Delete: the card dissolves into dust from the trash control.
    Dust,
    /// Delete when dust cannot be built: [`Motion::PreviewDeleteFallback`].
    DeleteFallback,
}

impl ExitKind {
    /// How long the card keeps its slot (its exit animation's duration).
    pub fn hold_ms(self) -> f64 {
        match self {
            Self::Dismiss => 1_030.0,
            Self::Dust => DUST_HOLD_MS,
            Self::DeleteFallback => 680.0,
        }
    }

    /// When survivors begin sliding into the slot. The fallback collapses its
    /// slot from its 55 % keyframe, so survivors follow from there.
    pub fn settle_delay_ms(self) -> f64 {
        match self {
            Self::Dismiss => DISMISS_SETTLE_DELAY_MS,
            Self::Dust => DELETE_SETTLE_DELAY_MS,
            Self::DeleteFallback => 0.55 * 680.0,
        }
    }

    /// The card's own keyframes; dust is drawn from [`DustParticle`]s.
    pub fn motion(self) -> Option<Motion> {
        match self {
            Self::Dismiss => Some(Motion::PreviewDismiss),
            Self::Dust => None,
            Self::DeleteFallback => Some(Motion::PreviewDeleteFallback),
        }
    }
}

/// The survivor settle ([`Transition::PreviewStackSettle`]). Shipping writes
/// its timing literally, so it resolves without a token table.
pub fn settle_tween() -> Tween {
    Tween {
        duration_ms: 580.0,
        easing: CubicBezier {
            x1: 0.4,
            y1: 0.0,
            x2: 0.2,
            y2: 1.0,
        },
    }
}

/// Clear all's start delay for chronological `index` of `count` live cards:
/// the bottom card leaves first.
pub fn clear_delay_ms(count: usize, index: usize, top_anchor: bool) -> f64 {
    let from_bottom = if top_anchor {
        index
    } else {
        count.saturating_sub(index + 1)
    };
    (from_bottom as f64 * CLEAR_STAGGER_MS).min(CLEAR_STAGGER_MAX_MS)
}

/// One card leaving the stack while it keeps its slot.
#[derive(Clone, Debug, PartialEq)]
pub struct CardExit {
    pub kind: ExitKind,
    /// When the exit was requested.
    pub started_ms: f64,
    /// Clear all's stagger; the exit's animation starts this much later.
    pub delay_ms: f64,
    /// Whether older cards slide into the slot (not while Clear all empties
    /// the whole stack).
    pub settles: bool,
    /// Slots this card had already moved when it began exiting. An exiting
    /// card keeps that shift instead of sliding into later holes.
    pub frozen_shift: f64,
}

impl CardExit {
    /// Milliseconds into the exit's own animation.
    pub fn elapsed_ms(&self, now_ms: f64) -> f64 {
        now_ms - self.started_ms - self.delay_ms
    }

    pub fn finished(&self, now_ms: f64, reduced_motion: bool) -> bool {
        reduced_motion || self.elapsed_ms(now_ms) >= self.kind.hold_ms()
    }
}

/// Cards in display order, live and exiting, oldest first. Hosts lay out and
/// size the stack from [`Self::display_ids`] so an exiting card holds its slot
/// (shipping keeps `flex-basis: 160px` through every exit), slide live cards
/// by [`Self::shift_px`], and drop finished exits with [`Self::prune`], after
/// which the ordinary layout for the smaller stack lands where the slide
/// ended.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StackExits {
    display: Vec<String>,
    exits: Vec<(String, CardExit)>,
}

impl StackExits {
    /// Follow the stack's live membership (oldest first). Exiting cards keep
    /// their places; new cards join as the newest.
    pub fn sync(&mut self, live: &[String]) {
        self.display
            .retain(|id| live.contains(id) || self.exits.iter().any(|(exit, _)| exit == id));
        for id in live {
            if !self.display.contains(id) {
                self.display.push(id.clone());
            }
        }
    }

    /// Start `id`'s exit. `live` is the stack's membership that still includes
    /// `id`; remove it from the stack afterwards. Returns false for a card that
    /// is unknown or already leaving.
    #[allow(clippy::too_many_arguments)]
    pub fn begin(
        &mut self,
        live: &[String],
        id: &str,
        kind: ExitKind,
        now_ms: f64,
        delay_ms: f64,
        settles: bool,
        settle: &Tween,
    ) -> bool {
        self.sync(live);
        if !self.display.iter().any(|existing| existing == id) || self.exiting(id).is_some() {
            return false;
        }
        let frozen_shift = self.shift_slots(id, now_ms, false, settle);
        self.exits.push((
            id.to_owned(),
            CardExit {
                kind,
                started_ms: now_ms,
                delay_ms: delay_ms.max(0.0),
                settles,
                frozen_shift,
            },
        ));
        true
    }

    pub fn exiting(&self, id: &str) -> Option<&CardExit> {
        self.exits
            .iter()
            .find_map(|(exit, state)| (exit == id).then_some(state))
    }

    pub fn is_empty(&self) -> bool {
        self.exits.is_empty()
    }

    pub fn display_ids(&self) -> &[String] {
        &self.display
    }

    /// Number of cards that currently occupy slots.
    pub fn display_count(&self) -> usize {
        self.display.len()
    }

    /// Drop exits whose hold ended. Returns whether any card left.
    pub fn prune(&mut self, now_ms: f64, reduced_motion: bool) -> bool {
        let before = self.exits.len();
        let finished: Vec<String> = self
            .exits
            .iter()
            .filter(|(_, exit)| exit.finished(now_ms, reduced_motion))
            .map(|(id, _)| id.clone())
            .collect();
        self.exits.retain(|(id, _)| !finished.contains(id));
        self.display.retain(|id| !finished.contains(id));
        self.exits.len() != before
    }

    /// Forget every exit at once (the stack collapsed, closed or rebuilt).
    pub fn clear(&mut self) {
        let exiting: Vec<String> = self.exits.drain(..).map(|(id, _)| id).collect();
        self.display.retain(|id| !exiting.contains(id));
    }

    /// Milliseconds until the next exit ends, for hosts that schedule one
    /// wake-up instead of polling.
    pub fn next_finish_in_ms(&self, now_ms: f64) -> Option<f64> {
        self.exits
            .iter()
            .map(|(_, exit)| (exit.kind.hold_ms() - exit.elapsed_ms(now_ms)).max(0.0))
            .reduce(f64::min)
    }

    /// Whether any slot is still sliding or any exit animating.
    pub fn running(&self, now_ms: f64, reduced_motion: bool) -> bool {
        !reduced_motion
            && self
                .exits
                .iter()
                .any(|(_, exit)| !exit.finished(now_ms, false))
    }

    /// Card slots (0…n, fractional while sliding) `id` has moved toward the
    /// stack anchor. Older cards than an exiting one slide into its slot:
    /// bottom-anchored stacks move them down, top-anchored ones up.
    pub fn shift_slots(&self, id: &str, now_ms: f64, reduced_motion: bool, settle: &Tween) -> f64 {
        if let Some(exit) = self.exiting(id) {
            return exit.frozen_shift;
        }
        let Some(index) = self.display.iter().position(|existing| existing == id) else {
            return 0.0;
        };
        self.exits
            .iter()
            .filter(|(_, exit)| exit.settles)
            .filter(|(exit_id, _)| {
                self.display
                    .iter()
                    .position(|existing| existing == exit_id)
                    .is_some_and(|slot| slot > index)
            })
            .map(|(_, exit)| {
                let since = exit.elapsed_ms(now_ms) - exit.kind.settle_delay_ms();
                if since < 0.0 && !reduced_motion {
                    0.0
                } else {
                    settle.progress(since.max(0.0), reduced_motion)
                }
            })
            .sum()
    }

    /// [`Self::shift_slots`] in points, positive downward.
    pub fn shift_px(
        &self,
        id: &str,
        now_ms: f64,
        reduced_motion: bool,
        settle: &Tween,
        top_anchor: bool,
    ) -> f64 {
        let direction = if top_anchor { -1.0 } else { 1.0 };
        self.shift_slots(id, now_ms, reduced_motion, settle) * THUMBNAIL_CARD_SLOT * direction
    }
}

/// Why the stack toolbar (Clear all, Show less) appears or leaves.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolbarCause {
    /// The pile expanded: [`Motion::PreviewToolbarIn`].
    Expand,
    /// The stack collapsed: [`Motion::PreviewToolbarOut`].
    Collapse,
    /// A Close or Delete left fewer than two live cards:
    /// [`Motion::PreviewToolbarExit`].
    Exit,
    /// Clear all: [`Motion::PreviewToolbarClear`].
    Clear,
    /// Anything else (a new capture, settings, a rebuild): no motion.
    Other,
}

/// The stack toolbar's presence: shown or hidden, plus the motion playing.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StackToolbar {
    shown: bool,
    motion: Option<(Motion, f64)>,
}

impl StackToolbar {
    /// Report whether the toolbar belongs on screen now and why it changed.
    /// Returns whether its presence changed.
    pub fn set(&mut self, shown: bool, cause: ToolbarCause, now_ms: f64) -> bool {
        if shown == self.shown {
            return false;
        }
        self.shown = shown;
        self.motion = match (shown, cause) {
            (true, ToolbarCause::Expand) => Some((Motion::PreviewToolbarIn, now_ms)),
            (false, ToolbarCause::Collapse) => Some((Motion::PreviewToolbarOut, now_ms)),
            (false, ToolbarCause::Exit) => Some((Motion::PreviewToolbarExit, now_ms)),
            (false, ToolbarCause::Clear) => Some((Motion::PreviewToolbarClear, now_ms)),
            _ => None,
        };
        true
    }

    pub fn shown(&self) -> bool {
        self.shown
    }

    /// The motion playing, if any, and when it started.
    pub fn motion(&self) -> Option<(Motion, f64)> {
        self.motion
    }

    /// Pose to paint, or `None` when the toolbar is gone.
    pub fn pose(
        &self,
        tokens: &impl MotionTokens,
        now_ms: f64,
        reduced_motion: bool,
    ) -> Option<Pose> {
        let playing = self.motion.and_then(|(motion, started)| {
            let animation = motion.resolve(tokens)?;
            let elapsed = now_ms - started;
            animation
                .running(elapsed, reduced_motion)
                .then(|| animation.pose_at(elapsed, reduced_motion))
        });
        match (self.shown, playing) {
            (true, pose) => Some(pose.unwrap_or(Pose::REST)),
            (false, pose) => pose,
        }
    }

    /// Shipping disables pointer input while the toolbar enters or leaves.
    pub fn interactive(&self, tokens: &impl MotionTokens, now_ms: f64, reduced: bool) -> bool {
        self.shown && !self.running(tokens, now_ms, reduced)
    }

    pub fn running(&self, tokens: &impl MotionTokens, now_ms: f64, reduced: bool) -> bool {
        self.motion.is_some_and(|(motion, started)| {
            motion
                .resolve(tokens)
                .is_some_and(|animation| animation.running(now_ms - started, reduced))
        })
    }
}

/// Show less morph progress (0 = the 28 pt stack icon, 1 = the pill) for the
/// width tween and the icon/label crossfade tween.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MinimizeMorph {
    /// 0…1 along the width change.
    pub width: f64,
    /// 0…1 along the icon-to-label crossfade.
    pub swap: f64,
}

impl MinimizeMorph {
    /// Icon opacity, horizontal slide (points, toward the pile edge) and
    /// scale: `opacity: 0; translateX(-8px) scale(0.8)` at the pill.
    pub fn icon(self) -> (f64, f64, f64) {
        (1.0 - self.swap, -8.0 * self.swap, 1.0 - 0.2 * self.swap)
    }

    /// Label opacity and horizontal slide: from `translateX(4px)` to rest.
    pub fn label(self) -> (f64, f64) {
        (self.swap, 4.0 * (1.0 - self.swap))
    }

    /// The control's width between the icon and the hover pill.
    pub fn width_between(self, rest: f64, hover: f64) -> f64 {
        rest + (hover - rest) * self.width
    }
}

/// Centre of the Delete control, where the ash front starts, mirrored with the
/// controls on right-anchored stacks.
pub fn delete_origin_x(card_width: f64, saved: bool, right_anchor: bool) -> f64 {
    let left = if saved {
        DELETE_ORIGIN_AFTER_CLOSE_X
    } else {
        DELETE_ORIGIN_FIRST_X
    };
    if right_anchor {
        card_width - left
    } else {
        left
    }
}

/// One dust chip (`ThumbnailDustParticle`). Coordinates are points in the
/// padded dust layer (`left`/`top`) and the card (`source*`); the camelCase
/// field names are the AppKit host's decoding contract.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DustParticle {
    pub id: u32,
    pub left: f64,
    pub top: f64,
    pub width: f64,
    pub height: f64,
    pub card_width: f64,
    pub card_height: f64,
    pub source_left: f64,
    pub source_top: f64,
    pub surface_width: f64,
    pub surface_height: f64,
    pub surface_offset_x: f64,
    pub surface_offset_y: f64,
    pub dx: f64,
    pub dy: f64,
    pub rotate: f64,
    pub delay_ms: f64,
    pub duration_ms: f64,
}

/// A chip's pose `elapsed` after the delete (`thumbnailDustVisualAt`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct DustVisual {
    pub opacity: f64,
    pub dx: f64,
    pub dy: f64,
    /// Degrees.
    pub rotate: f64,
    pub scale: f64,
}

/// `mulberry32`: a small deterministic generator so a host can rebuild the
/// same dust for a card (and tests can pin it).
struct Mulberry32(u32);

impl Mulberry32 {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x6d2b_79f5);
        let mut t = self.0;
        t = (t ^ (t >> 15)).wrapping_mul(t | 1);
        t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61));
        f64::from(t ^ (t >> 14)) / 4_294_967_296.0
    }
}

fn grid_count(size: f64, target: f64, min: usize, max: usize) -> usize {
    ((size / target).round() as usize).clamp(min, max)
}

/// Slice a card into ash chips (`buildThumbnailDustParticles`): the delay
/// starts at the trash control and spreads radially with a ragged front;
/// chips drift up and outward, never down. `image` is the source media size
/// for the `object-fit: cover` crop.
pub fn dust_particles(
    card_width: f64,
    card_height: f64,
    image: (f64, f64),
    origin: (f64, f64),
    seed: u32,
) -> Vec<DustParticle> {
    let width = card_width.max(1.0);
    let height = card_height.max(1.0);
    let mut cols = grid_count(width, DUST_TARGET_CELL, 14, 24);
    let mut rows = grid_count(height, DUST_TARGET_CELL, 8, 16);
    if cols * rows > DUST_MAX_PARTICLES {
        let scale = (DUST_MAX_PARTICLES as f64 / (cols * rows) as f64).sqrt();
        cols = ((cols as f64 * scale).floor() as usize).max(10);
        rows = ((rows as f64 * scale).floor() as usize).max(6);
    }
    let mut random = Mulberry32(seed);
    let (origin_x, origin_y) = origin;
    let (image_width, image_height) = (image.0.max(1.0), image.1.max(1.0));
    let cover = (width / image_width).max(height / image_height);
    let (surface_width, surface_height) = (image_width * cover, image_height * cover);
    let (offset_x, offset_y) = (
        (width - surface_width) / 2.0,
        (height - surface_height) / 2.0,
    );
    let (cell_w, cell_h) = (width / cols as f64, height / rows as f64);
    let max_dist = (origin_x.max(width - origin_x))
        .hypot(origin_y.max(height - origin_y))
        .max(1.0);
    let mut particles = Vec::with_capacity(cols * rows);
    for row in 0..rows {
        for col in 0..cols {
            let left = col as f64 * cell_w;
            let top = row as f64 * cell_h;
            let (cx, cy) = (left + cell_w / 2.0, top + cell_h / 2.0);
            let wave = (cx - origin_x).hypot(cy - origin_y) / max_dist;
            let angle = (cy - origin_y).atan2(cx - origin_x);
            let wobble = (angle * 2.7 + wave * 5.5).sin() * 0.07 * wave;
            let scatter = (random.next() - 0.5) * 0.34 * wave * wave;
            let delay_norm = (wave + wobble + scatter).clamp(0.0, 1.12);
            let jitter = random.next() * (18.0 + wave * 140.0);
            let away_x = (cx - origin_x) / max_dist;
            let dx = away_x * (12.0 + random.next() * 26.0) + (random.next() - 0.5) * 22.0;
            let dy = -36.0 - random.next() * 58.0;
            let rotate = (random.next() - 0.5) * 120.0;
            let duration = 780.0 + (random.next() * 320.0 + wave * 80.0).floor();
            particles.push(DustParticle {
                id: particles.len() as u32,
                left: left + DUST_LAYER_PAD,
                top: top + DUST_LAYER_PAD,
                // Slight overlap hides sub-pixel gaps between chips.
                width: cell_w + 0.55,
                height: cell_h + 0.55,
                card_width: width,
                card_height: height,
                source_left: left,
                source_top: top,
                surface_width,
                surface_height,
                surface_offset_x: offset_x,
                surface_offset_y: offset_y,
                dx,
                dy,
                rotate,
                delay_ms: (delay_norm * DISSOLVE_WAVE_MS + jitter).floor(),
                duration_ms: duration,
            });
        }
    }
    particles
}

/// `THUMBNAIL_DUST_EASE` over each chip's flight.
const DUST_EASE: CubicBezier = CubicBezier {
    x1: 0.28,
    y1: 0.0,
    x2: 0.12,
    y2: 1.0,
};
const DUST_LIFT_AT: f64 = 0.14;

/// `thumbnailDustVisualAt`: the flight eases once, then the explicit keys mix
/// linearly (opacity 1 → .72 at half → 0 at 82 %).
pub fn dust_visual_at(particle: &DustParticle, elapsed_ms: f64) -> DustVisual {
    let local = elapsed_ms - particle.delay_ms;
    if local <= 0.0 || local.is_nan() {
        return DustVisual {
            opacity: 1.0,
            dx: 0.0,
            dy: 0.0,
            rotate: 0.0,
            scale: 1.0,
        };
    }
    let t = DUST_EASE.ease((local / particle.duration_ms.max(1.0)).min(1.0));
    let keys = [
        (0.0, 1.0),
        (DUST_LIFT_AT, 1.0),
        (0.5, 0.72),
        (0.82, 0.0),
        (1.0, 0.0),
    ];
    let opacity = keys
        .windows(2)
        .find(|pair| t <= pair[1].0)
        .map_or(0.0, |pair| {
            let (a, b) = (pair[0], pair[1]);
            a.1 + (b.1 - a.1) * (t - a.0) / (b.0 - a.0)
        });
    let mix = |a: f64, b: f64, f: f64| a + (b - a) * f;
    let (from, to, f) = if t <= DUST_LIFT_AT {
        (
            (0.0, 0.0, 0.0, 1.0),
            (0.06, 0.06, 0.08, 0.98),
            t / DUST_LIFT_AT,
        )
    } else {
        (
            (0.06, 0.06, 0.08, 0.98),
            (1.0, 1.0, 1.0, 0.18),
            (t - DUST_LIFT_AT) / (1.0 - DUST_LIFT_AT),
        )
    };
    DustVisual {
        opacity,
        dx: particle.dx * mix(from.0, to.0, f),
        dy: particle.dy * mix(from.1, to.1, f),
        rotate: particle.rotate * mix(from.2, to.2, f),
        scale: mix(from.3, to.3, f),
    }
}

/// `cubic-bezier(0.33, 0, 0.2, 1)` (dust clip) and `(0.22, 0.1, 0.25, 1)`
/// (source and chrome fades).
const CLIP_EASE: CubicBezier = CubicBezier {
    x1: 0.33,
    y1: 0.0,
    x2: 0.2,
    y2: 1.0,
};
const FADE_EASE: CubicBezier = CubicBezier {
    x1: 0.22,
    y1: 0.1,
    x2: 0.25,
    y2: 1.0,
};

/// A dissolving card's layers `elapsed` after Delete.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DustFrame {
    /// How far the dust clip has opened: 0 clips chips to the card's rounded
    /// rect, 1 lets them fly across the whole padded layer.
    pub clip_open: f64,
    /// `thumbnail-dust-clip` opacity of the whole dust layer.
    pub layer_opacity: f64,
    /// `thumbnail-delete-img-fade`: the frozen hover image over the chips.
    pub source_opacity: f64,
    /// `thumbnail-chrome-fade-*`: the card's controls.
    pub chrome_opacity: f64,
}

pub fn dust_frame(elapsed_ms: f64, reduced_motion: bool) -> DustFrame {
    if reduced_motion {
        return DustFrame {
            clip_open: 1.0,
            layer_opacity: 0.0,
            source_opacity: 0.0,
            chrome_opacity: 0.0,
        };
    }
    let clip = (elapsed_ms / DUST_CLIP_MS).clamp(0.0, 1.0);
    let clip_open = if clip <= 0.08 {
        0.0
    } else {
        CLIP_EASE.ease(((clip - 0.08) / 0.62).min(1.0))
    };
    let layer_opacity = if clip <= 0.7 {
        1.0
    } else {
        1.0 - CLIP_EASE.ease(((clip - 0.7) / 0.2).min(1.0))
    };
    let hold_then_fade = |duration: f64, hold: f64| {
        let linear = (elapsed_ms / duration).clamp(0.0, 1.0);
        if linear <= hold {
            1.0
        } else {
            1.0 - FADE_EASE.ease((linear - hold) / (1.0 - hold))
        }
    };
    DustFrame {
        clip_open,
        layer_opacity,
        source_opacity: hold_then_fade(550.0, 0.2),
        chrome_opacity: hold_then_fade(850.0, 0.16),
    }
}

/// The dissolving card's shadow and outline opacity
/// ([`Transition::PreviewDeleteFrameFade`]).
pub fn delete_frame_opacity(tokens: &impl MotionTokens, elapsed_ms: f64, reduced: bool) -> f64 {
    Transition::PreviewDeleteFrameFade
        .resolve(tokens)
        .map_or(0.0, |tween| tween.value(1.0, 0.0, elapsed_ms, reduced))
}

/// One sparkle dot of `.thumbnail-collapsed-hit-target::before/::after`: a
/// radial gradient in a 6 pt tile placed at `x`/`y` percent of the layer.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct SparkleDot {
    /// 0…1 of the layer's width and height minus the 6 pt tile.
    pub x: f64,
    pub y: f64,
    /// Solid core radius and fade-out radius, points.
    pub core: f64,
    pub fade: f64,
    /// Accent (`--theme-accent`) or white.
    pub accent: bool,
    pub alpha: f64,
}

const fn dot(x: f64, y: f64, core: f64, fade: f64, accent: bool, alpha: f64) -> SparkleDot {
    SparkleDot {
        x,
        y,
        core,
        fade,
        accent,
        alpha,
    }
}

/// `::before`, animated by [`Motion::PreviewPileSparkle`].
pub const SPARKLES_EARLY: [SparkleDot; 6] = [
    dot(0.12, 0.62, 1.2, 2.0, true, 0.9),
    dot(0.38, 0.18, 0.9, 1.6, false, 0.85),
    dot(0.66, 0.40, 1.0, 1.8, true, 0.8),
    dot(0.90, 0.72, 0.8, 1.5, false, 0.7),
    dot(0.26, 0.06, 1.0, 1.8, true, 0.85),
    dot(0.78, 0.24, 0.9, 1.6, false, 0.8),
];
/// `::after`, animated by [`Motion::PreviewPileSparkleLate`].
pub const SPARKLES_LATE: [SparkleDot; 5] = [
    dot(0.24, 0.84, 1.0, 1.8, false, 0.8),
    dot(0.55, 0.66, 1.2, 2.0, true, 0.85),
    dot(0.80, 0.88, 0.9, 1.6, true, 0.7),
    dot(0.46, 0.12, 1.0, 1.8, false, 0.75),
    dot(0.08, 0.30, 1.1, 1.9, true, 0.8),
];
/// The sparkle layer reaches this far over the fanned pile, past the sides
/// and past the near edge (`inset: -96px -10px -6px`, flipped when top-anchored).
pub const SPARKLE_REACH: f64 = 96.0;
pub const SPARKLE_SIDE: f64 = 10.0;
pub const SPARKLE_NEAR: f64 = 6.0;
const SPARKLE_TILE: f64 = 6.0;

/// The sparkle layer around the pile's hit target `(x, y, width, height)`,
/// y down.
pub fn sparkle_layer(target: (f64, f64, f64, f64), top_anchor: bool) -> (f64, f64, f64, f64) {
    let (x, y, width, height) = target;
    let (above, below) = if top_anchor {
        (SPARKLE_NEAR, SPARKLE_REACH)
    } else {
        (SPARKLE_REACH, SPARKLE_NEAR)
    };
    (
        x - SPARKLE_SIDE,
        y - above,
        width + SPARKLE_SIDE * 2.0,
        height + above + below,
    )
}

/// A dot's centre in the layer `(x, y, width, height)`, y down.
pub fn sparkle_center(dot: &SparkleDot, layer: (f64, f64, f64, f64)) -> (f64, f64) {
    let (x, y, width, height) = layer;
    (
        x + (width - SPARKLE_TILE) * dot.x + SPARKLE_TILE / 2.0,
        y + (height - SPARKLE_TILE) * dot.y + SPARKLE_TILE / 2.0,
    )
}

/// The sparkle tables for hosts that read them over the settings ABI.
pub fn sparkle_catalog() -> serde_json::Value {
    serde_json::json!({
        "reach": SPARKLE_REACH,
        "side": SPARKLE_SIDE,
        "near": SPARKLE_NEAR,
        "early": SPARKLES_EARLY,
        "late": SPARKLES_LATE,
    })
}

/// Every exit and settle timing for hosts that read them over the ABI.
pub fn exit_catalog() -> serde_json::Value {
    let kind = |kind: ExitKind| {
        serde_json::json!({
            "hold_ms": kind.hold_ms(),
            "settle_delay_ms": kind.settle_delay_ms(),
        })
    };
    serde_json::json!({
        "dismiss": kind(ExitKind::Dismiss),
        "dust": kind(ExitKind::Dust),
        "delete_fallback": kind(ExitKind::DeleteFallback),
        "clear_stagger_ms": CLEAR_STAGGER_MS,
        "clear_stagger_max_ms": CLEAR_STAGGER_MAX_MS,
        "dust_pad": DUST_LAYER_PAD,
        "delete_origin": {
            "first_x": DELETE_ORIGIN_FIRST_X,
            "after_close_x": DELETE_ORIGIN_AFTER_CLOSE_X,
            "y": DELETE_ORIGIN_Y,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Shipping;
    impl MotionTokens for Shipping {
        fn duration_ms(&self, token: &str) -> Option<f64> {
            Some(match token {
                "dur-1" => 90.,
                "dur-2" => 140.,
                "dur-3" => 200.,
                "dur-4" => 280.,
                _ => return None,
            })
        }
        fn easing(&self, token: &str) -> Option<[f64; 4]> {
            Some(match token {
                "ease-out" => [0.16, 1., 0.3, 1.],
                "ease-standard" => [0.2, 0.8, 0.2, 1.],
                "ease-in" => [0.4, 0., 1., 1.],
                "ease-in-out" => [0.45, 0., 0.55, 1.],
                _ => return None,
            })
        }
    }

    fn ids(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    fn settle() -> Tween {
        let settle = Transition::PreviewStackSettle.resolve(&Shipping).unwrap();
        assert_eq!(settle, settle_tween());
        settle
    }

    #[test]
    fn exits_hold_their_slot_while_older_cards_settle_after_the_delay() {
        let settle = settle();
        let mut exits = StackExits::default();
        let mut live = ids(&["a", "b", "c"]);
        exits.sync(&live);
        assert!(exits.begin(&live, "b", ExitKind::Dismiss, 1_000., 0., true, &settle));
        assert!(!exits.begin(&live, "b", ExitKind::Dismiss, 1_000., 0., true, &settle));
        live.retain(|id| id != "b");
        exits.sync(&live);
        assert_eq!(
            exits.display_ids(),
            ["a", "b", "c"],
            "the exiting card keeps its slot"
        );
        assert_eq!(exits.display_count(), 3);
        // Before the 450 ms streak finishes nothing moves.
        assert_eq!(exits.shift_slots("a", 1_449., false, &settle), 0.);
        let mid = exits.shift_slots("a", 1_450. + 290., false, &settle);
        assert!(mid > 0. && mid < 1., "{mid}");
        assert_eq!(exits.shift_slots("a", 1_450. + 580., false, &settle), 1.);
        // Newer cards and the exiting card itself stay put.
        assert_eq!(exits.shift_slots("c", 2_000., false, &settle), 0.);
        assert_eq!(exits.shift_slots("b", 2_000., false, &settle), 0.);
        assert_eq!(
            exits.shift_px("a", 2_100., false, &settle, false),
            THUMBNAIL_CARD_SLOT
        );
        assert_eq!(
            exits.shift_px("a", 2_100., false, &settle, true),
            -THUMBNAIL_CARD_SLOT
        );
        assert!(exits.running(2_000., false));
        assert_eq!(exits.next_finish_in_ms(2_000.), Some(30.));
        assert!(!exits.prune(2_029., false));
        assert!(exits.prune(2_030., false));
        assert_eq!(exits.display_ids(), ["a", "c"]);
        assert!(exits.is_empty() && !exits.running(2_030., false));
        assert_eq!(exits.shift_slots("a", 2_030., false, &settle), 0.);
    }

    #[test]
    fn concurrent_exits_stack_shifts_and_freeze_an_exiting_cards_offset() {
        let settle = settle();
        let mut exits = StackExits::default();
        let live = ids(&["a", "b", "c", "d"]);
        exits.sync(&live);
        exits.begin(&live, "d", ExitKind::Dismiss, 0., 0., true, &settle);
        // "b" has already slid one slot for "d" when it starts to dissolve.
        exits.begin(&live, "b", ExitKind::Dust, 1_100., 0., true, &settle);
        assert_eq!(exits.exiting("b").unwrap().frozen_shift, 1.);
        assert_eq!(exits.shift_slots("b", 5_000., false, &settle), 1.);
        // "a" slides for both holes: dust waits 1.8 s before moving it.
        assert_eq!(exits.shift_slots("a", 2_000., false, &settle), 1.);
        assert_eq!(
            exits.shift_slots("a", 1_100. + 1_800. + 580., false, &settle),
            2.
        );
        // New captures join as the newest card.
        let live = ids(&["a", "c", "e"]);
        exits.sync(&live);
        assert_eq!(exits.display_ids(), ["a", "b", "c", "d", "e"]);
        exits.clear();
        assert_eq!(exits.display_ids(), ["a", "c", "e"]);
    }

    #[test]
    fn reduced_motion_and_clear_all_skip_the_settle() {
        let settle = settle();
        let mut exits = StackExits::default();
        let live = ids(&["a", "b"]);
        exits.begin(&live, "b", ExitKind::Dust, 0., 0., true, &settle);
        assert_eq!(exits.shift_slots("a", 0., true, &settle), 1.);
        assert!(!exits.running(0., true));
        assert!(exits.prune(0., true));

        let mut clearing = StackExits::default();
        for (index, id) in live.iter().enumerate() {
            let delay = clear_delay_ms(2, index, false);
            clearing.begin(&live, id, ExitKind::Dismiss, 0., delay, false, &settle);
        }
        assert_eq!(clearing.shift_slots("a", 900., false, &settle), 0.);
        assert_eq!(clearing.exiting("a").unwrap().delay_ms, 36.);
        assert_eq!(clearing.exiting("b").unwrap().delay_ms, 0.);
        assert!(clearing.prune(1_030., false));
        assert_eq!(
            clearing.display_ids(),
            ["a"],
            "the staggered card holds 36 ms longer"
        );
        assert!(clearing.prune(1_066., false));
    }

    #[test]
    fn clear_stagger_starts_at_the_bottom_and_caps() {
        assert_eq!(clear_delay_ms(3, 2, false), 0.);
        assert_eq!(clear_delay_ms(3, 0, false), 72.);
        assert_eq!(clear_delay_ms(3, 0, true), 0.);
        assert_eq!(clear_delay_ms(3, 2, true), 72.);
        assert_eq!(clear_delay_ms(12, 0, false), CLEAR_STAGGER_MAX_MS);
    }

    #[test]
    fn exit_kinds_match_shipping_holds_and_delays() {
        assert_eq!(ExitKind::Dismiss.hold_ms(), 1_030.);
        assert_eq!(
            Motion::PreviewDismiss
                .resolve(&Shipping)
                .unwrap()
                .duration_ms,
            ExitKind::Dismiss.hold_ms()
        );
        assert_eq!(ExitKind::Dust.settle_delay_ms(), 1_800.);
        assert_eq!(ExitKind::Dust.motion(), None);
        assert_eq!(
            Motion::PreviewDeleteFallback
                .resolve(&Shipping)
                .unwrap()
                .duration_ms,
            ExitKind::DeleteFallback.hold_ms()
        );
        // The dismiss hold is the streak plus the shared settle.
        assert_eq!(
            DISMISS_SETTLE_DELAY_MS + settle().duration_ms,
            ExitKind::Dismiss.hold_ms()
        );
    }

    #[test]
    fn toolbar_enters_on_expand_and_leaves_by_cause() {
        let mut toolbar = StackToolbar::default();
        assert_eq!(toolbar.pose(&Shipping, 0., false), None);
        assert!(toolbar.set(true, ToolbarCause::Expand, 0.));
        let start = toolbar.pose(&Shipping, 100., false).unwrap();
        assert_eq!(start.opacity, 0., "hidden for the first 35 %");
        assert!(!toolbar.interactive(&Shipping, 100., false));
        assert_eq!(toolbar.pose(&Shipping, 520., false), Some(Pose::REST));
        assert!(toolbar.interactive(&Shipping, 520., false));
        assert!(!toolbar.set(true, ToolbarCause::Other, 600.));

        assert!(toolbar.set(false, ToolbarCause::Collapse, 1_000.));
        assert_eq!(toolbar.motion().unwrap().0, Motion::PreviewToolbarOut);
        let leaving = toolbar.pose(&Shipping, 1_140., false).unwrap();
        assert!(leaving.opacity > 0. && leaving.opacity < 1.);
        assert_eq!(toolbar.pose(&Shipping, 1_280., false), None);

        toolbar.set(true, ToolbarCause::Other, 2_000.);
        assert_eq!(toolbar.pose(&Shipping, 2_000., false), Some(Pose::REST));
        toolbar.set(false, ToolbarCause::Exit, 3_000.);
        assert_eq!(
            toolbar.pose(&Shipping, 3_400., false),
            Some(Pose::REST),
            "the exit holds for 42 %"
        );
        toolbar.set(true, ToolbarCause::Other, 5_000.);
        toolbar.set(false, ToolbarCause::Clear, 6_000.);
        assert_eq!(toolbar.motion().unwrap().0, Motion::PreviewToolbarClear);
        assert_eq!(
            toolbar.pose(&Shipping, 6_000., true),
            None,
            "reduced motion"
        );
    }

    #[test]
    fn minimize_morph_crossfades_icon_and_label() {
        let rest = MinimizeMorph {
            width: 0.,
            swap: 0.,
        };
        assert_eq!(rest.icon(), (1., 0., 1.));
        assert_eq!(rest.label(), (0., 4.));
        assert_eq!(rest.width_between(28., 92.), 28.);
        let pill = MinimizeMorph {
            width: 1.,
            swap: 1.,
        };
        assert_eq!(pill.icon(), (0., -8., 0.8));
        assert_eq!(pill.label(), (1., 0.));
        assert_eq!(pill.width_between(28., 92.), 92.);
    }

    #[test]
    fn dust_matches_the_shipping_grid_wave_and_flight() {
        let origin = (delete_origin_x(284., false, false), DELETE_ORIGIN_Y);
        let particles = dust_particles(284., 160., (1440., 900.), origin, 7);
        // 284 / 11 → 26 → capped at 24 columns; 160 / 11 → 15 rows; 360 > 220
        // scales to 18 × 11, the 198 chips of the shipping fixture.
        assert_eq!(particles.len(), 18 * 11);
        assert_eq!(
            particles,
            dust_particles(284., 160., (1440., 900.), origin, 7)
        );
        assert_ne!(
            particles,
            dust_particles(284., 160., (1440., 900.), origin, 8)
        );
        let first = particles[0];
        assert_eq!((first.left, first.top), (DUST_LAYER_PAD, DUST_LAYER_PAD));
        assert!((first.surface_width - 284.).abs() < 1e-9);
        assert!(first.surface_height > 160., "cover crop");
        let near = particles
            .iter()
            .min_by(|a, b| a.delay_ms.total_cmp(&b.delay_ms))
            .unwrap();
        assert!(
            near.source_left < 60. && near.source_top < 40.,
            "front starts at trash"
        );
        for particle in &particles {
            assert!(particle.dy < 0., "ash never falls");
            assert!(particle.delay_ms + particle.duration_ms < DUST_HOLD_MS);
        }
        let rest = dust_visual_at(&first, first.delay_ms);
        assert_eq!((rest.opacity, rest.scale), (1., 1.));
        let end = dust_visual_at(&first, first.delay_ms + first.duration_ms);
        assert_eq!(end.opacity, 0.);
        assert!((end.scale - 0.18).abs() < 1e-9);
        assert!((end.dx - first.dx).abs() < 1e-9 && (end.dy - first.dy).abs() < 1e-9);
        assert_eq!(delete_origin_x(284., true, true), 284. - 57.5);
    }

    #[test]
    fn dust_frame_opens_the_clip_then_fades_the_layer() {
        let start = dust_frame(0., false);
        assert_eq!(
            (
                start.clip_open,
                start.layer_opacity,
                start.source_opacity,
                start.chrome_opacity
            ),
            (0., 1., 1., 1.)
        );
        assert_eq!(dust_frame(100., false).source_opacity, 1., "20 % hold");
        let open = dust_frame(0.7 * DUST_CLIP_MS, false);
        assert!((open.clip_open - 1.).abs() < 1e-6 && (open.layer_opacity - 1.).abs() < 1e-6);
        assert_eq!(open.source_opacity, 0.);
        assert!(dust_frame(0.9 * DUST_CLIP_MS, false).layer_opacity.abs() < 1e-6);
        assert_eq!(dust_frame(DUST_CLIP_MS, false).layer_opacity, 0.);
        assert_eq!(dust_frame(0., true).layer_opacity, 0.);
        assert_eq!(delete_frame_opacity(&Shipping, 0., false), 1.);
        assert_eq!(delete_frame_opacity(&Shipping, 500., false), 0.);
    }

    #[test]
    fn sparkles_cover_the_fanned_pile_and_loop() {
        let layer = sparkle_layer((28., 100., 284., 160.), false);
        assert_eq!(layer, (18., 4., 304., 262.));
        assert_eq!(
            sparkle_layer((28., 100., 284., 160.), true),
            (18., 94., 304., 262.)
        );
        let (x, y) = sparkle_center(&SPARKLES_EARLY[0], layer);
        assert!((x - (18. + 298. * 0.12 + 3.)).abs() < 1e-9);
        assert!((y - (4. + 256. * 0.62 + 3.)).abs() < 1e-9);
        let early = Motion::PreviewPileSparkle.resolve(&Shipping).unwrap();
        assert_eq!(early.pose_repeating(0., false).opacity, 0.);
        let peak = early.pose_repeating(540., false);
        assert!((peak.opacity - 0.9).abs() < 1e-9);
        assert_eq!(early.pose_repeating(1_800. + 540., false), peak);
        assert_eq!(early.pose_repeating(540., true).opacity, 0.);
        let late = Motion::PreviewPileSparkleLate.resolve(&Shipping).unwrap();
        assert_eq!(late.pose_repeating(400., false).opacity, 0., "0.5 s delay");
        let catalog = sparkle_catalog();
        assert_eq!(catalog["early"].as_array().unwrap().len(), 6);
        assert_eq!(catalog["late"][1]["accent"], true);
        assert_eq!(exit_catalog()["dust"]["settle_delay_ms"], 1_800.);
        assert_eq!(exit_catalog()["delete_origin"]["after_close_x"], 57.5);
    }
}
