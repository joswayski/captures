//! The wake a mini preview leaves while its file is dragged out of the stack.
//!
//! The card's whole shell leaves its slot when the drag begins. The emptied
//! slot keeps a breathing hairline imprint with an accent glow, and a tapered
//! ribbon of light runs from the slot to the carried drag image, bowing
//! behind a fast carry and carrying motes of light toward it. An accepted
//! drop snaps the ribbon away toward where it landed with a soft bloom; a
//! cancelled drag or a drop back on the stack draws it back into the slot,
//! where the card re-forms.
//!
//! Hosts draw each [`WakeFrame`] in a click-through overlay over the display
//! (so the ribbon reaches wherever the pointer goes) in overlay points, y
//! down, and apply `card_opacity` and `card_scale` to the card itself. This
//! module owns the motion and geometry so AppKit, Win32 and GTK cannot
//! drift; hosts own the clock, the pointer and drawing.

/// A rounded rectangle in overlay points (y down).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WakeRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub radius: f64,
}

impl WakeRect {
    fn center(&self) -> (f64, f64) {
        (self.x + self.width / 2., self.y + self.height / 2.)
    }
}

/// How the drag ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WakeEnd {
    /// Another app accepted the file: the card is gone with it.
    Dropped,
    /// Cancelled, refused, or dropped back on the stack: the card returns.
    Returned,
}

/// The colour a ribbon pass or glow is drawn in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WakeInk {
    /// The theme accent.
    Accent,
    /// The glass palette's light text colour (`--glass-text`).
    Light,
}

/// One pass of the ribbon: a closed outline filled with a linear gradient
/// from `from` to `to` in `ink`, with `stops` of (offset 0…1, opacity).
#[derive(Clone, Debug, PartialEq)]
pub struct WakeRibbon {
    pub ink: WakeInk,
    pub outline: Vec<(f64, f64)>,
    pub from: (f64, f64),
    pub to: (f64, f64),
    pub stops: [(f64, f64); 3],
}

/// A mote of light: a `Light` core of `radius` at `opacity`, inside an
/// `Accent` radial halo [`MOTE_HALO_SCALE`] times wider fading from
/// `opacity × MOTE_HALO_OPACITY` to clear.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WakeMote {
    pub x: f64,
    pub y: f64,
    pub radius: f64,
    pub opacity: f64,
}

/// Everything to draw at one instant.
#[derive(Clone, Debug, PartialEq)]
pub struct WakeFrame {
    /// The emptied slot, drawn as a recessed pocket: a `--glass-veil`
    /// (rgb 10, 10, 14) fill at [`SLOT_SHADE_OPACITY`] under a `Light`
    /// hairline ([`SLOT_LINE_WIDTH`] inside the edge, [`SLOT_LINE_OPACITY`]),
    /// inside an `Accent` glow outside the edge ([`SLOT_GLOW_BLUR`],
    /// [`SLOT_GLOW_OPACITY`]), all times `slot_opacity`.
    pub slot: WakeRect,
    pub slot_opacity: f64,
    /// The card in its slot: hidden while it is away, re-formed on return.
    pub card_opacity: f64,
    pub card_scale: f64,
    /// Outermost glow first, core last. Empty while the tip is in the slot.
    pub ribbons: Vec<WakeRibbon>,
    pub motes: Vec<WakeMote>,
    /// An accepted drop's landing: an `Accent` radial glow of `radius`
    /// fading from `opacity` at the centre to clear (the core is not drawn).
    pub bloom: Option<WakeMote>,
    /// Everything drawn, as (x, y, width, height), for dirty rectangles.
    pub bounds: (f64, f64, f64, f64),
}

pub const SLOT_LINE_WIDTH: f64 = 1.25;
pub const SLOT_LINE_OPACITY: f64 = 0.6;
pub const SLOT_SHADE_OPACITY: f64 = 0.16;
/// The glow's blur radius (CSS `box-shadow` blur) around the slot.
pub const SLOT_GLOW_BLUR: f64 = 18.;
pub const SLOT_GLOW_OPACITY: f64 = 0.5;
pub const MOTE_HALO_SCALE: f64 = 5.;
pub const MOTE_HALO_OPACITY: f64 = 0.5;

/// (full width in points, ink, opacity at the slot, two fifths along, at the tip).
/// Nested accent passes stack into a soft falloff around a light core.
const RIBBON_PASSES: [(f64, WakeInk, [f64; 3]); 7] = [
    (46., WakeInk::Accent, [0., 0.025, 0.05]),
    (32., WakeInk::Accent, [0., 0.04, 0.07]),
    (21., WakeInk::Accent, [0., 0.06, 0.1]),
    (13., WakeInk::Accent, [0., 0.09, 0.15]),
    (7., WakeInk::Accent, [0., 0.16, 0.28]),
    (4., WakeInk::Light, [0., 0.12, 0.3]),
    (1.8, WakeInk::Light, [0., 0.7, 1.]),
];
const STOP_OFFSETS: [f64; 3] = [0., 0.45, 1.];
/// Outline samples along each side of the ribbon.
pub const RIBBON_SAMPLES: usize = 28;
/// The ribbon fades in as the tip leaves the slot's centre.
const VISIBLE_FROM: f64 = 6.;
const VISIBLE_AT: f64 = 56.;
/// The bend's spring: the curve's control trails the tip, critically damped.
const BEND_OMEGA: f64 = 15.;
const BEND_STEP_MS: f64 = 8.;
const BEND_MAX_FRAME_MS: f64 = 64.;
const MOTES: usize = 9;
const MOTE_PERIOD_MS: f64 = 1_500.;
/// Card fades out under the drag image.
const DEPART_MS: f64 = 160.;
const DEPART_SCALE: f64 = 0.95;
const SLOT_IN_DELAY_MS: f64 = 40.;
const SLOT_IN_MS: f64 = 240.;
const BREATH_MS: f64 = 2_600.;
const BREATH_LOW: f64 = 0.74;
/// Cancel: the ribbon draws back into the slot, then the card re-forms.
const RETURN_MS: f64 = 340.;
const REFORM_AT_MS: f64 = 170.;
const REFORM_MS: f64 = 280.;
const REFORM_SCALE: f64 = 0.94;
const SLOT_OUT_MS: f64 = 220.;
/// Accepted drop: the ribbon snaps toward the landing as the slot fades.
const DROP_MS: f64 = 300.;
const BLOOM_MS: f64 = 420.;
const BLOOM_RADIUS: f64 = 34.;
const BLOOM_OPACITY: f64 = 0.55;

/// One drag's wake, from the first pointer step past the drag threshold
/// until its end animation finishes.
#[derive(Clone, Debug)]
pub struct DragWake {
    slot: WakeRect,
    tip: (f64, f64),
    lag: (f64, f64),
    lag_velocity: (f64, f64),
    began_ms: f64,
    last_ms: f64,
    end: Option<(WakeEnd, f64)>,
    reduced: bool,
}

impl DragWake {
    /// Start with the card in `slot` and the drag image centred on `tip`.
    pub fn new(slot: WakeRect, tip: (f64, f64), now_ms: f64, reduced_motion: bool) -> Self {
        Self {
            slot,
            tip,
            lag: tip,
            lag_velocity: (0., 0.),
            began_ms: now_ms,
            last_ms: now_ms,
            end: None,
            reduced: reduced_motion,
        }
    }

    /// The slot moved (the stack re-laid out under the drag).
    pub fn set_slot(&mut self, slot: WakeRect) {
        self.slot = slot;
    }

    /// The drag image's centre moved. Ignored once the drag ended.
    pub fn pointer(&mut self, tip: (f64, f64)) {
        if self.end.is_none() {
            self.tip = tip;
        }
    }

    /// The drag ended; only the first end counts.
    pub fn end(&mut self, end: WakeEnd, now_ms: f64) {
        if self.end.is_none() {
            self.end = Some((end, now_ms));
        }
    }

    pub fn ended(&self) -> Option<WakeEnd> {
        self.end.map(|(end, _)| end)
    }

    /// The wake at `now_ms`, or None once its end has finished playing.
    pub fn frame(&mut self, now_ms: f64) -> Option<WakeFrame> {
        self.advance_bend(now_ms);
        let elapsed = (now_ms - self.began_ms).max(0.);
        let since_end = self.end.map(|(end, at)| (end, (now_ms - at).max(0.)));
        if let Some((end, t)) = since_end
            && (self.reduced
                || t >= match end {
                    WakeEnd::Dropped => DROP_MS.max(BLOOM_MS),
                    WakeEnd::Returned => REFORM_AT_MS + REFORM_MS,
                })
        {
            return None;
        }
        let reduced = self.reduced;
        let progress = |t: f64, ms: f64| if reduced { 1. } else { (t / ms).clamp(0., 1.) };

        // The card: away while dragged, re-formed on return.
        let depart = ease_out_cubic(progress(elapsed, DEPART_MS));
        let (mut card_opacity, mut card_scale) = (1. - depart, 1. - (1. - DEPART_SCALE) * depart);
        // The slot: fades in, breathes, fades out at the end.
        let breath = if reduced {
            1.
        } else {
            let phase = elapsed / BREATH_MS * std::f64::consts::TAU;
            BREATH_LOW + (1. - BREATH_LOW) * (0.5 - 0.5 * phase.cos())
        };
        let mut slot_opacity =
            ease_out_cubic(progress(elapsed - SLOT_IN_DELAY_MS, SLOT_IN_MS)) * breath;
        // The drawn part of the curve (0 at the slot, 1 at the tip) and the
        // ribbon's overall opacity.
        let (mut from, mut to, mut fade) = (0., 1., 1.);
        let mut bloom = None;
        match since_end {
            Some((WakeEnd::Returned, t)) => {
                to = 1. - ease_in_out_cubic(progress(t, RETURN_MS));
                fade = 1. - 0.25 * progress(t, RETURN_MS);
                let reform = progress(t - REFORM_AT_MS, REFORM_MS);
                card_opacity = ease_out_cubic(progress(t - REFORM_AT_MS, REFORM_MS * 0.7));
                card_scale = REFORM_SCALE + (1. - REFORM_SCALE) * ease_out_back(reform);
                slot_opacity *= 1. - ease_in_out_cubic(progress(t - REFORM_AT_MS, SLOT_OUT_MS));
            }
            Some((WakeEnd::Dropped, t)) => {
                let p = progress(t, DROP_MS);
                from = ease_in_cubic(p);
                fade = 1. - p;
                card_opacity = 0.;
                slot_opacity *= 1. - ease_out_cubic(p);
                let b = progress(t, BLOOM_MS);
                bloom = Some(WakeMote {
                    x: self.tip.0,
                    y: self.tip.1,
                    radius: BLOOM_RADIUS * (0.35 + 0.65 * ease_out_cubic(b)),
                    opacity: BLOOM_OPACITY * (1. - b) * (1. - b),
                });
            }
            None => {}
        }

        let start = self.slot.center();
        let chord = (self.tip.0 - start.0).hypot(self.tip.1 - start.1);
        let visible = smoothstep(VISIBLE_FROM, VISIBLE_AT, chord) * fade;
        let control = (
            start.0 + (self.lag.0 - start.0) / 2.,
            start.1 + (self.lag.1 - start.1) / 2.,
        );
        let curve = Quad {
            a: start,
            b: control,
            c: self.tip,
        };
        let mut ribbons = Vec::new();
        if visible > 0.001 && to - from > 0.001 {
            for (width, ink, opacity) in RIBBON_PASSES {
                ribbons.push(WakeRibbon {
                    ink,
                    outline: curve.outline(from, to, width),
                    from: curve.at(from),
                    to: curve.at(to),
                    stops: std::array::from_fn(|i| (STOP_OFFSETS[i], opacity[i] * visible)),
                });
            }
        }
        let mut motes = Vec::new();
        if !reduced && visible > 0.001 {
            for i in 0..MOTES {
                let seed = |k: usize| hash(i * 7 + k);
                let q =
                    (elapsed / MOTE_PERIOD_MS + i as f64 / MOTES as f64 + 0.06 * seed(1)).fract();
                let u = q.powf(1.6);
                if u < from || u > to {
                    continue;
                }
                let (x, y) = curve.at(u);
                let (nx, ny) = curve.normal(u);
                let shimmer = 1. + 0.25 * (elapsed / 260. + i as f64).sin();
                let lateral = (seed(2) - 0.5) * 10. * (std::f64::consts::PI * u).sin() * shimmer;
                motes.push(WakeMote {
                    x: x + nx * lateral,
                    y: y + ny * lateral,
                    radius: (1.3 + 1.1 * seed(3)) * (1. - 0.25 * u),
                    opacity: (std::f64::consts::PI * q).sin().powf(1.3) * 0.95 * visible,
                });
            }
        }

        let mut bounds = Bounds::around(self.slot, SLOT_GLOW_BLUR * 2.);
        let pad = RIBBON_PASSES[0].0;
        for &(x, y) in ribbons.first().map_or(&[][..], |r| &r.outline) {
            bounds.add(x, y, pad);
        }
        for m in motes.iter().chain(bloom.iter()) {
            bounds.add(m.x, m.y, m.radius * MOTE_HALO_SCALE + 2.);
        }
        Some(WakeFrame {
            slot: self.slot,
            slot_opacity,
            card_opacity,
            card_scale,
            ribbons,
            motes,
            bloom,
            bounds: bounds.rect(),
        })
    }

    fn advance_bend(&mut self, now_ms: f64) {
        let dt = (now_ms - self.last_ms).clamp(0., BEND_MAX_FRAME_MS);
        self.last_ms = now_ms.max(self.last_ms);
        if self.reduced {
            self.lag = self.tip;
            return;
        }
        let steps = (dt / BEND_STEP_MS).ceil().max(1.);
        let h = dt / steps / 1000.;
        for _ in 0..steps as usize {
            let spring = |lag: f64, velocity: f64, tip: f64| {
                let accel = BEND_OMEGA * BEND_OMEGA * (tip - lag) - 2. * BEND_OMEGA * velocity;
                let velocity = velocity + accel * h;
                (lag + velocity * h, velocity)
            };
            let x = spring(self.lag.0, self.lag_velocity.0, self.tip.0);
            let y = spring(self.lag.1, self.lag_velocity.1, self.tip.1);
            self.lag = (x.0, y.0);
            self.lag_velocity = (x.1, y.1);
        }
    }
}

struct Quad {
    a: (f64, f64),
    b: (f64, f64),
    c: (f64, f64),
}

impl Quad {
    fn at(&self, t: f64) -> (f64, f64) {
        let s = 1. - t;
        (
            s * s * self.a.0 + 2. * s * t * self.b.0 + t * t * self.c.0,
            s * s * self.a.1 + 2. * s * t * self.b.1 + t * t * self.c.1,
        )
    }

    /// The unit normal at `t` (the chord's when the curve is degenerate).
    fn normal(&self, t: f64) -> (f64, f64) {
        let s = 1. - t;
        let mut d = (
            2. * s * (self.b.0 - self.a.0) + 2. * t * (self.c.0 - self.b.0),
            2. * s * (self.b.1 - self.a.1) + 2. * t * (self.c.1 - self.b.1),
        );
        if d.0.hypot(d.1) < 1e-6 {
            d = (self.c.0 - self.a.0, self.c.1 - self.a.1);
        }
        let length = d.0.hypot(d.1).max(1e-6);
        (-d.1 / length, d.0 / length)
    }

    /// The ribbon between `from` and `to` along the curve: widest toward
    /// the tip, pinched to a point at both ends of the drawn part.
    fn outline(&self, from: f64, to: f64, width: f64) -> Vec<(f64, f64)> {
        let n = RIBBON_SAMPLES - 1;
        let side = |k: usize, sign: f64| {
            let s = k as f64 / n as f64;
            let t = from + (to - from) * s;
            let (x, y) = self.at(t);
            let (nx, ny) = self.normal(t);
            let pinch = smoothstep(0., 0.18, s) * smoothstep(0., 0.1, 1. - s);
            let half = width * (0.35 + 0.65 * t) * pinch / 2. * sign;
            (x + nx * half, y + ny * half)
        };
        (0..=n)
            .map(|k| side(k, 1.))
            .chain((0..=n).rev().map(|k| side(k, -1.)))
            .collect()
    }
}

struct Bounds(f64, f64, f64, f64);

impl Bounds {
    fn around(r: WakeRect, pad: f64) -> Self {
        Self(
            r.x - pad,
            r.y - pad,
            r.x + r.width + pad,
            r.y + r.height + pad,
        )
    }

    fn add(&mut self, x: f64, y: f64, pad: f64) {
        self.0 = self.0.min(x - pad);
        self.1 = self.1.min(y - pad);
        self.2 = self.2.max(x + pad);
        self.3 = self.3.max(y + pad);
    }

    fn rect(&self) -> (f64, f64, f64, f64) {
        (self.0, self.1, self.2 - self.0, self.3 - self.1)
    }
}

/// A stable 0…1 value per index.
fn hash(i: usize) -> f64 {
    ((i as f64 * 12.9898 + 4.1414).sin() * 43_758.545_3)
        .fract()
        .abs()
}

fn smoothstep(edge0: f64, edge1: f64, x: f64) -> f64 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0., 1.);
    t * t * (3. - 2. * t)
}

fn ease_out_cubic(t: f64) -> f64 {
    1. - (1. - t).powi(3)
}

fn ease_in_cubic(t: f64) -> f64 {
    t * t * t
}

fn ease_in_out_cubic(t: f64) -> f64 {
    if t < 0.5 {
        4. * t * t * t
    } else {
        1. - (-2. * t + 2.).powi(3) / 2.
    }
}

/// A gentle overshoot (`cubic-bezier`-free easeOutBack, c1 = 1.2).
fn ease_out_back(t: f64) -> f64 {
    let c1 = 1.2;
    let c3 = c1 + 1.;
    1. + c3 * (t - 1.).powi(3) + c1 * (t - 1.).powi(2)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SLOT: WakeRect = WakeRect {
        x: 100.,
        y: 400.,
        width: 240.,
        height: 135.,
        radius: 12.,
    };
    const CENTER: (f64, f64) = (220., 467.5);

    fn carried(reduced: bool) -> DragWake {
        let mut wake = DragWake::new(SLOT, CENTER, 0., reduced);
        // Carry up and to the left over a few frames, then hold.
        for (i, at) in [
            (16., (180., 400.)),
            (32., (100., 300.)),
            (48., (-20., 150.)),
        ] {
            wake.pointer(at);
            wake.frame(i);
        }
        wake
    }

    #[test]
    fn the_card_leaves_its_slot_and_a_ribbon_reaches_the_carried_image() {
        let mut wake = carried(false);
        let frame = wake.frame(400.).unwrap();
        assert_eq!(frame.card_opacity, 0.);
        assert!(frame.slot_opacity > 0.7);
        let core = frame.ribbons.last().unwrap();
        assert_eq!(core.ink, WakeInk::Light);
        // From the slot's centre to the drag image's centre, pinched to a
        // point at the slot and brightest at the tip.
        assert_eq!(core.from, CENTER);
        assert_eq!(core.to, (-20., 150.));
        assert_eq!(core.outline.first(), Some(&CENTER));
        assert_eq!(core.stops[0].1, 0.);
        assert!(core.stops[2].1 > 0.99);
        assert_eq!(core.outline.len(), RIBBON_SAMPLES * 2);
        // The drawing fits the reported bounds.
        let (x, y, w, h) = frame.bounds;
        for &(px, py) in frame.ribbons.iter().flat_map(|r| &r.outline) {
            assert!(px >= x && px <= x + w && py >= y && py <= y + h);
        }
        assert!(!frame.motes.is_empty());
    }

    #[test]
    fn a_fast_carry_bows_the_ribbon_behind_the_tip_then_it_straightens() {
        let mut wake = DragWake::new(SLOT, CENTER, 0., false);
        wake.frame(0.);
        // A sudden carry 400 points right: the midpoint trails below the chord's.
        wake.pointer((620., 467.5));
        let bowed = wake.frame(16.).unwrap();
        let core = bowed.ribbons.last().unwrap();
        let mid = core.outline[RIBBON_SAMPLES / 2];
        assert!(
            mid.0 < 420. - 40.,
            "midpoint {mid:?} should trail the chord's"
        );
        // Held still, the spring settles onto the straight chord within a
        // second of frames (the core is 2 points wide).
        let mut straight = None;
        for frame in 2..=64 {
            straight = wake.frame(frame as f64 * 16.);
        }
        let mid = straight.unwrap().ribbons.last().unwrap().outline[RIBBON_SAMPLES / 2];
        assert!(
            (mid.1 - 467.5).abs() < 2. && (mid.0 - 420.).abs() < 25.,
            "{mid:?}"
        );
    }

    #[test]
    fn a_tip_still_over_the_slot_draws_no_ribbon() {
        let mut wake = DragWake::new(SLOT, (CENTER.0 + 3., CENTER.1), 0., false);
        let frame = wake.frame(200.).unwrap();
        assert!(frame.ribbons.is_empty() && frame.motes.is_empty());
        assert!(frame.card_opacity < 0.01);
    }

    #[test]
    fn a_return_draws_the_ribbon_back_and_the_card_reforms() {
        let mut wake = carried(false);
        wake.end(WakeEnd::Returned, 1_000.);
        // A late pointer step or second end changes nothing.
        wake.pointer((900., 900.));
        wake.end(WakeEnd::Dropped, 1_010.);
        assert_eq!(wake.ended(), Some(WakeEnd::Returned));
        let mid = wake.frame(1_000. + RETURN_MS / 2.).unwrap();
        let core = mid.ribbons.last().unwrap();
        assert_eq!(core.from, CENTER);
        // Halfway back along the curve, nowhere near the late pointer.
        let back = core.to;
        assert!(back.0 > -20. && back.0 < 220. && back.1 > 150. && back.1 < 467.5);
        assert!(mid.card_opacity < 0.2);
        let reformed = wake.frame(1_000. + REFORM_AT_MS + REFORM_MS - 1.).unwrap();
        assert!(reformed.ribbons.is_empty());
        assert!(reformed.card_opacity > 0.99 && (reformed.card_scale - 1.).abs() < 0.01);
        assert!(wake.frame(1_000. + REFORM_AT_MS + REFORM_MS).is_none());
    }

    #[test]
    fn a_drop_snaps_the_ribbon_toward_the_landing_and_blooms_there() {
        let mut wake = carried(false);
        wake.end(WakeEnd::Dropped, 1_000.);
        let mid = wake.frame(1_000. + DROP_MS / 2.).unwrap();
        let core = mid.ribbons.last().unwrap();
        assert_eq!(core.to, (-20., 150.));
        assert_ne!(core.from, CENTER);
        assert_eq!(mid.card_opacity, 0.);
        let bloom = mid.bloom.unwrap();
        assert_eq!((bloom.x, bloom.y), (-20., 150.));
        assert!(mid.slot_opacity < 0.4);
        assert!(wake.frame(1_000. + DROP_MS.max(BLOOM_MS)).is_none());
    }

    #[test]
    fn reduced_motion_holds_still_and_ends_at_once() {
        let mut wake = carried(true);
        let frame = wake.frame(16.).unwrap();
        assert_eq!(frame.card_opacity, 0.);
        assert!(frame.motes.is_empty());
        let a = frame.ribbons.last().unwrap().outline[RIBBON_SAMPLES / 2];
        let b = wake.frame(5_000.).unwrap().ribbons.last().unwrap().outline[RIBBON_SAMPLES / 2];
        assert_eq!(a, b);
        assert_eq!(wake.frame(5_000.).unwrap().slot_opacity, 1.);
        wake.end(WakeEnd::Returned, 6_000.);
        assert!(wake.frame(6_000.).is_none());
    }
}
