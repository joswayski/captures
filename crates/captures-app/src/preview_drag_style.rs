//! How a mini preview moves while it is dragged: its file out of the stack
//! into another app ([`DragOut`], in one of three [`DragStyle`]s), or up and
//! down within the stack to reorder it ([`Reorder`]). Preferences shows each
//! style as a looping animation ([`demo`]).
//!
//! The rules, curves and geometry live here so AppKit, Win32 and GTK move
//! the same way. Hosts own the clock (any monotonic milliseconds), the
//! pointer and drawing, and call these once per frame.
//!
//! Both drags respect the stack's orientation: a stack on a top corner lists
//! newest first from its top edge, a bottom stack lists oldest first and
//! keeps its bottom edge still. A slot that closes pulls the older cards
//! toward the anchored edge (see [`closing_shift`]), so the window can drop
//! the slot afterwards without anything jumping.

pub use captures_settings::MiniPreviewDragStyle as DragStyle;

use crate::motion::CubicBezier;
use crate::preview::{
    THUMBNAIL_CARD_HEIGHT, THUMBNAIL_CONTROL_GUTTER, THUMBNAIL_GAP, THUMBNAIL_PADDING,
};

/// One card's slot pitch: its height plus the gap below it.
pub const SLOT_PITCH: f64 = THUMBNAIL_CARD_HEIGHT + THUMBNAIL_GAP;

/// `--ease-out`-like curve for everything that settles.
const SETTLE: CubicBezier = CubicBezier {
    x1: 0.2,
    y1: 0.8,
    x2: 0.2,
    y2: 1.0,
};
/// A gentle in-out for slots opening and closing.
const SLOT: CubicBezier = CubicBezier {
    x1: 0.4,
    y1: 0.0,
    x2: 0.2,
    y2: 1.0,
};

/// Frosted: blur in, sharpen back.
const FROST_IN_MS: f64 = 260.;
/// The media's Gaussian radius in points at full frost.
pub const FROST_BLUR: f64 = 14.;
const FROST_MEDIA_OPACITY: f64 = 0.5;
const FROST_MEDIA_SCALE: f64 = 1.08;
const FROST_CARD_SCALE: f64 = 0.97;
/// Dock: a short lift, a brief beat, then the card leaves and its slot
/// eases shut, all on the symmetric [`DOCK_EASE`].
const DOCK_LIFT_MS: f64 = 150.;
const DOCK_LIFT_SCALE: f64 = 1.03;
const DOCK_BEAT_MS: f64 = 50.;
const DOCK_LEAVE_MS: f64 = 260.;
const DOCK_AWAY_SCALE: f64 = 0.94;
/// The slot starts closing a moment after the card starts leaving.
const DOCK_CLOSE_DELAY_MS: f64 = 60.;
const DOCK_CLOSE_MS: f64 = 460.;
/// Returning, the slot eases open over this long.
const DOCK_OPEN_MS: f64 = 400.;
/// CSS `ease-in-out`: the dock's gap opens and closes without a lurch.
const DOCK_EASE: CubicBezier = CubicBezier {
    x1: 0.42,
    y1: 0.0,
    x2: 0.58,
    y2: 1.0,
};
/// Well: the card empties into a recessed tile.
const WELL_IN_MS: f64 = 220.;
/// Returning reverses the style; the dock reopens its slot first, and its
/// card re-forms once the slot is [`DOCK_REFORM_AT`] open.
const RETURN_MS: f64 = 300.;
const DOCK_REFORM_AT: f64 = 0.45;
/// An accepted drop: the imprint shrinks away, then its slot closes.
const HANDOFF_MS: f64 = 220.;
const HANDOFF_SCALE: f64 = 0.92;
const SLOT_CLOSE_DELAY_MS: f64 = 140.;
pub const SLOT_CLOSE_MS: f64 = 420.;

/// The hairline drawn inside the imprint's edge, in the glass palette's
/// light text colour at `hairline` opacity.
pub const HAIRLINE_WIDTH: f64 = 1.;
pub const HAIRLINE_OPACITY: f64 = 0.28;
/// The well's `--glass-veil` (rgb 10, 10, 14) fill, its inner shadow and
/// the glyph's disc.
pub const WELL_FILL_OPACITY: f64 = 0.18;
pub const WELL_INNER_SHADOW_BLUR: f64 = 3.;
pub const WELL_INNER_SHADOW_OPACITY: f64 = 0.28;
pub const GLYPH_DIAMETER: f64 = 30.;
pub const GLYPH_DISC_OPACITY: f64 = 0.08;
/// The glyph: an arrow out of the tile, in a 24-unit box drawn at 14 points.
pub const GLYPH_PATH: &str = "M7 17 17 7M9 7h8v8";
pub const GLYPH_SIZE: f64 = 14.;
pub const GLYPH_STROKE: f64 = 2.2;
pub const GLYPH_OPACITY: f64 = 0.75;

/// How the dragged card draws in its slot at one instant. Opacities and
/// scales multiply what the card already draws; `1` everywhere (with zero
/// blur) is the card at rest.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CardPose {
    /// The whole card: media, chrome, shadow and outline.
    pub opacity: f64,
    /// About the card's centre.
    pub scale: f64,
    /// The media's Gaussian blur radius in points.
    pub media_blur: f64,
    pub media_opacity: f64,
    /// About the media's centre, inside the card's rounded clip.
    pub media_scale: f64,
    /// The card's opaque glass fill behind the media.
    pub fill_opacity: f64,
    /// Chips, buttons, rings and the drop shadow.
    pub chrome_opacity: f64,
    /// See [`HAIRLINE_OPACITY`]; this is its multiplier.
    pub hairline: f64,
    /// The well's fill and inner shadow, as a multiplier.
    pub well: f64,
    /// The glyph's opacity multiplier and its scale about the card's centre.
    pub glyph: f64,
    pub glyph_scale: f64,
    /// How much of its slot the card still takes (1 open, 0 closed).
    pub slot: f64,
}

impl CardPose {
    pub const REST: Self = Self {
        opacity: 1.,
        scale: 1.,
        media_blur: 0.,
        media_opacity: 1.,
        media_scale: 1.,
        fill_opacity: 1.,
        chrome_opacity: 1.,
        hairline: 0.,
        well: 0.,
        glyph: 0.,
        glyph_scale: 1.,
        slot: 1.,
    };

    fn lerp(a: Self, b: Self, t: f64) -> Self {
        let l = |x: f64, y: f64| x + (y - x) * t;
        Self {
            opacity: l(a.opacity, b.opacity),
            scale: l(a.scale, b.scale),
            media_blur: l(a.media_blur, b.media_blur),
            media_opacity: l(a.media_opacity, b.media_opacity),
            media_scale: l(a.media_scale, b.media_scale),
            fill_opacity: l(a.fill_opacity, b.fill_opacity),
            chrome_opacity: l(a.chrome_opacity, b.chrome_opacity),
            hairline: l(a.hairline, b.hairline),
            well: l(a.well, b.well),
            glyph: l(a.glyph, b.glyph),
            glyph_scale: l(a.glyph_scale, b.glyph_scale),
            slot: l(a.slot, b.slot),
        }
    }

    /// The style's look while the file is out of the stack.
    fn away(style: DragStyle) -> Self {
        match style {
            DragStyle::Frosted => Self {
                scale: FROST_CARD_SCALE,
                media_blur: FROST_BLUR,
                media_opacity: FROST_MEDIA_OPACITY,
                media_scale: FROST_MEDIA_SCALE,
                fill_opacity: 0.,
                chrome_opacity: 0.,
                hairline: 1.,
                ..Self::REST
            },
            DragStyle::Dock => Self {
                opacity: 0.,
                scale: DOCK_AWAY_SCALE,
                slot: 0.,
                ..Self::REST
            },
            DragStyle::Well => Self {
                media_opacity: 0.,
                fill_opacity: 0.,
                chrome_opacity: 0.,
                hairline: 1.,
                well: 1.,
                glyph: 1.,
                ..Self::REST
            },
        }
    }
}

/// How a file drag ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DragEnd {
    /// Another app accepted the file: the card leaves and its slot closes.
    Dropped,
    /// Cancelled, refused, or dropped back on the stack: the card returns.
    Returned,
}

/// One card's file drag out of the stack, from the moment the drag begins
/// until its end has finished playing. While the file is back over its own
/// stack ([`DragOut::hover`]) the card shows again, ready to land.
#[derive(Clone, Debug)]
pub struct DragOut {
    style: DragStyle,
    began_ms: f64,
    /// Whether the file is over its own stack, since when, and the pose it
    /// eased from.
    hover: Option<(bool, f64, CardPose)>,
    end: Option<(DragEnd, f64, CardPose)>,
    reduced: bool,
}

impl DragOut {
    pub fn new(style: DragStyle, now_ms: f64, reduced_motion: bool) -> Self {
        Self {
            style,
            began_ms: now_ms,
            hover: None,
            end: None,
            reduced: reduced_motion,
        }
    }

    pub fn style(&self) -> DragStyle {
        self.style
    }

    /// The drag ended; only the first end counts.
    pub fn end(&mut self, end: DragEnd, now_ms: f64) {
        if self.end.is_none() {
            let from = self.away_pose(now_ms);
            self.end = Some((end, now_ms, from));
        }
    }

    pub fn ended(&self) -> Option<DragEnd> {
        self.end.map(|(end, ..)| end)
    }

    /// The file came back over its own stack (`over`) or left it again:
    /// the card eases back into view, slot open, as it would return (hosts
    /// open its slot where it would land with [`Reorder::hovering`]), or
    /// back into the style's imprint. Ignored after an end or when nothing
    /// changes.
    pub fn hover(&mut self, over: bool, now_ms: f64) {
        let hovering = self.hover.is_some_and(|(over, ..)| over);
        if self.end.is_none() && over != hovering {
            let from = self.away_pose(now_ms);
            self.hover = Some((over, now_ms, from));
        }
    }

    /// Whether the card's slot is mostly closed at `now_ms`, as the dock
    /// leaves it. A file dropped back on the stack without hovering it
    /// first then moves its card to the new spot at once ([`drop_index`])
    /// and the slot reopens there; otherwise the stack glides around it
    /// ([`Reorder::landing`]) while it returns.
    pub fn slot_closed(&self, now_ms: f64) -> bool {
        self.pose(now_ms).slot < 0.5
    }

    /// Milliseconds an end takes to play out in this style.
    pub fn end_ms(style: DragStyle, end: DragEnd) -> f64 {
        match (style, end) {
            (DragStyle::Dock, DragEnd::Returned) => {
                (DOCK_OPEN_MS * DOCK_REFORM_AT + RETURN_MS).max(DOCK_OPEN_MS)
            }
            (_, DragEnd::Returned) => RETURN_MS,
            (DragStyle::Dock, DragEnd::Dropped) => DOCK_CLOSE_MS,
            (_, DragEnd::Dropped) => SLOT_CLOSE_DELAY_MS + SLOT_CLOSE_MS,
        }
    }

    /// Whether the pose still changes at `now_ms`: easing into the style,
    /// or playing an end that has not finished. Hosts may stop redrawing
    /// the card while this is false.
    pub fn moving(&self, now_ms: f64) -> bool {
        match self.end {
            Some(_) => !self.finished(now_ms),
            None if self.hover.is_some() => self.hover.is_some_and(|(over, at, _)| {
                let ms = if over {
                    Self::end_ms(self.style, DragEnd::Returned)
                } else {
                    RETURN_MS
                };
                !self.reduced && now_ms - at < ms
            }),
            None => {
                let away_ms = match self.style {
                    DragStyle::Frosted => FROST_IN_MS,
                    // The glyph settles 80 ms after the well.
                    DragStyle::Well => WELL_IN_MS + 80.,
                    DragStyle::Dock => {
                        DOCK_LIFT_MS
                            + DOCK_BEAT_MS
                            + DOCK_LEAVE_MS.max(DOCK_CLOSE_DELAY_MS + DOCK_CLOSE_MS)
                    }
                };
                !self.reduced && now_ms - self.began_ms < away_ms
            }
        }
    }

    /// Whether the end has finished: the card is back at rest, or gone with
    /// its slot closed. Hosts then drop this and commit the outcome.
    pub fn finished(&self, now_ms: f64) -> bool {
        self.end.is_some_and(|(end, at, _)| {
            self.reduced || now_ms - at >= Self::end_ms(self.style, end)
        })
    }

    fn progress(&self, elapsed: f64, ms: f64, curve: CubicBezier) -> f64 {
        if self.reduced {
            return 1.;
        }
        curve.ease((elapsed / ms).clamp(0., 1.))
    }

    /// The pose before any end: the style's while the file is out, the
    /// card's own while the file is back over the stack.
    fn away_pose(&self, now_ms: f64) -> CardPose {
        match self.hover {
            None => self.style_pose(now_ms),
            Some((true, at, from)) => self.returning(from, (now_ms - at).max(0.)),
            Some((false, at, from)) => CardPose::lerp(
                from,
                CardPose::away(self.style),
                self.progress(now_ms - at, RETURN_MS, SETTLE),
            ),
        }
    }

    /// Returning to rest from `from`, `t` milliseconds in. The dock opens
    /// its slot first and the card re-forms as it does.
    fn returning(&self, from: CardPose, t: f64) -> CardPose {
        if self.style == DragStyle::Dock && !self.reduced {
            let open = self.progress(t, DOCK_OPEN_MS, DOCK_EASE);
            let reform = self.progress(t - DOCK_OPEN_MS * DOCK_REFORM_AT, RETURN_MS, DOCK_EASE);
            let mut pose = CardPose::lerp(from, CardPose::REST, reform);
            pose.slot = from.slot + (1. - from.slot) * open;
            pose
        } else {
            CardPose::lerp(from, CardPose::REST, self.progress(t, RETURN_MS, SETTLE))
        }
    }

    /// The style's pose while the file is out.
    fn style_pose(&self, now_ms: f64) -> CardPose {
        let t = (now_ms - self.began_ms).max(0.);
        let away = CardPose::away(self.style);
        match self.style {
            DragStyle::Frosted => {
                CardPose::lerp(CardPose::REST, away, self.progress(t, FROST_IN_MS, SETTLE))
            }
            DragStyle::Well => {
                let mut pose =
                    CardPose::lerp(CardPose::REST, away, self.progress(t, WELL_IN_MS, SETTLE));
                // The glyph settles in from a little smaller.
                pose.glyph_scale = 0.8 + 0.2 * self.progress(t, WELL_IN_MS + 80., SETTLE);
                pose
            }
            DragStyle::Dock => {
                if self.reduced {
                    return away;
                }
                let lift = self.progress(t, DOCK_LIFT_MS, DOCK_EASE);
                let leaving = t - DOCK_LIFT_MS - DOCK_BEAT_MS;
                let leave = self.progress(leaving, DOCK_LEAVE_MS, DOCK_EASE);
                let close = self.progress(leaving - DOCK_CLOSE_DELAY_MS, DOCK_CLOSE_MS, DOCK_EASE);
                CardPose {
                    opacity: 1. - leave,
                    scale: 1. + (DOCK_LIFT_SCALE - 1.) * lift * (1. - leave)
                        - (1. - DOCK_AWAY_SCALE) * leave,
                    slot: 1. - close,
                    ..CardPose::REST
                }
            }
        }
    }

    /// The dragged card's pose at `now_ms`.
    pub fn pose(&self, now_ms: f64) -> CardPose {
        let Some((end, at, from)) = self.end else {
            return self.away_pose(now_ms);
        };
        let t = (now_ms - at).max(0.);
        match end {
            DragEnd::Returned => self.returning(from, t),
            DragEnd::Dropped => {
                let gone = self.progress(t, HANDOFF_MS, SETTLE);
                let close = if self.style == DragStyle::Dock {
                    self.progress(t, DOCK_CLOSE_MS, DOCK_EASE)
                } else {
                    self.progress(t - SLOT_CLOSE_DELAY_MS, SLOT_CLOSE_MS, SLOT)
                };
                CardPose {
                    opacity: from.opacity * (1. - gone),
                    scale: from.scale + (HANDOFF_SCALE - from.scale) * gone,
                    slot: from.slot * (1. - close),
                    ..from
                }
            }
        }
    }
}

/// How far a card older than a closing slot moves toward the stack's
/// anchored edge, in points, y down: down for a bottom stack, up for a top
/// one. `slot` is the closing card's [`CardPose::slot`]. Newer cards stay.
pub fn closing_shift(slot: f64, top_anchor: bool) -> f64 {
    let lost = (1. - slot.clamp(0., 1.)) * SLOT_PITCH;
    if top_anchor { -lost } else { lost }
}

/// The first slot's top in stack content (top stacks leave the control
/// gutter above their newest card).
fn first_slot(top_anchor: bool) -> f64 {
    if top_anchor {
        THUMBNAIL_CONTROL_GUTTER
    } else {
        THUMBNAIL_PADDING
    }
}

/// The visual slot (0 at the top) of chronological `index`.
fn visual_slot(count: usize, index: usize, top_anchor: bool) -> usize {
    if top_anchor {
        count.saturating_sub(index + 1)
    } else {
        index
    }
}

/// The chronological index whose slot is nearest a card whose top is at
/// content y `card_top`.
pub fn reorder_index(count: usize, card_top: f64, top_anchor: bool) -> usize {
    if count == 0 {
        return 0;
    }
    let slot = ((card_top - first_slot(top_anchor)) / SLOT_PITCH)
        .round()
        .clamp(0., (count - 1) as f64) as usize;
    if top_anchor { count - 1 - slot } else { slot }
}

/// The chronological index a card whose file is dropped back on its own
/// stack lands at: the slot nearest the pointer, `pointer_y` in stack
/// content (the coordinates of [`Reorder::slot_top`]).
pub fn drop_index(count: usize, pointer_y: f64, top_anchor: bool) -> usize {
    reorder_index(count, pointer_y - THUMBNAIL_CARD_HEIGHT / 2., top_anchor)
}

/// Lift: the carried card grows slightly as it leaves its slot.
pub const REORDER_LIFT_SCALE: f64 = 1.03;
const REORDER_LIFT_MS: f64 = 140.;
/// Neighbours glide toward their new slots with this time constant.
const REORDER_GLIDE_TAU_MS: f64 = 60.;
const REORDER_SETTLE_EPSILON: f64 = 0.25;
/// A press that moves this far more vertically than horizontally reorders;
/// a mostly horizontal one drags the file out at once.
pub const REORDER_THRESHOLD: f64 = 4.;
/// During a reorder, the pointer this far outside the card's column (or past
/// the stack's first or last slot) hands the gesture to a file drag.
pub const REORDER_ESCAPE: f64 = 24.;

/// Whether a press that moved `(dx, dy)` points is a reorder (mostly
/// vertical, in a stack of two or more), a file drag, or neither yet.
pub fn press_intent(dx: f64, dy: f64, count: usize) -> Option<PressIntent> {
    if dx.hypot(dy) < REORDER_THRESHOLD {
        return None;
    }
    Some(if count >= 2 && dy.abs() > dx.abs() {
        PressIntent::Reorder
    } else {
        PressIntent::FileDrag
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PressIntent {
    Reorder,
    FileDrag,
}

/// Whether the pointer, `dx` points outside the card's column horizontally
/// (0 inside) and `beyond` points past the stack's outermost slot edge (0 or
/// negative inside), should hand a reorder to a file drag.
pub fn reorder_escapes(dx_outside: f64, beyond: f64) -> bool {
    dx_outside > REORDER_ESCAPE || beyond > REORDER_ESCAPE
}

/// A card carried up or down the stack to a new position.
#[derive(Clone, Debug)]
pub struct Reorder {
    count: usize,
    from: usize,
    to: usize,
    top_anchor: bool,
    began_ms: f64,
    last_ms: f64,
    /// The carried card's top, in content y.
    carried_top: f64,
    /// Visual y offsets from each card's resting slot, chronological.
    offsets: Vec<f64>,
    /// A file hovering its own stack ([`Reorder::hovering`]): the card
    /// waits in the slot it was aimed at instead of following the pointer.
    aiming: bool,
    dropped: Option<f64>,
    reduced: bool,
}

impl Reorder {
    /// Pick up chronological `from` in a stack of `count` cards.
    pub fn new(count: usize, from: usize, top_anchor: bool, now_ms: f64, reduced: bool) -> Self {
        let from = from.min(count.saturating_sub(1));
        Self {
            count,
            from,
            to: from,
            top_anchor,
            began_ms: now_ms,
            last_ms: now_ms,
            carried_top: Self::slot_top(count, from, top_anchor),
            offsets: vec![0.; count],
            aiming: false,
            dropped: None,
            reduced,
        }
    }

    /// A card whose file is back over its own stack: it and the cards it
    /// would pass glide to open the slot it is [aimed](Self::aim) at,
    /// without the carry's lift, and hold there until [`Self::drop`] lands
    /// it or [`Self::cancel`] sends everything home.
    pub fn hovering(
        count: usize,
        from: usize,
        top_anchor: bool,
        now_ms: f64,
        reduced: bool,
    ) -> Self {
        Self {
            aiming: true,
            ..Self::new(count, from, top_anchor, now_ms, reduced)
        }
    }

    /// A card whose file came back onto the stack, already let go at `to`:
    /// it and the cards it passes glide to their new slots, without the
    /// carry's lift.
    pub fn landing(
        count: usize,
        from: usize,
        to: usize,
        top_anchor: bool,
        now_ms: f64,
        reduced: bool,
    ) -> Self {
        let mut reorder = Self::hovering(count, from, top_anchor, now_ms, reduced);
        reorder.aim(to);
        reorder.drop(now_ms);
        reorder
    }

    /// Where a hovering card would land now (see [`drop_index`]); its home
    /// slot when the file has left the stack. Ignored once let go.
    pub fn aim(&mut self, to: usize) {
        if self.aiming && self.dropped.is_none() {
            self.to = to.min(self.count.saturating_sub(1));
        }
    }

    /// The top of chronological `index`'s resting slot in stack content.
    pub fn slot_top(count: usize, index: usize, top_anchor: bool) -> f64 {
        first_slot(top_anchor) + visual_slot(count, index, top_anchor) as f64 * SLOT_PITCH
    }

    pub fn from(&self) -> usize {
        self.from
    }

    /// Where the card would land now.
    pub fn to(&self) -> usize {
        self.to
    }

    /// The pointer moved the carried card by `dy` points since the press.
    /// It follows the pointer, held within the stack's first and last slot.
    pub fn pointer(&mut self, dy: f64) {
        if self.aiming || self.dropped.is_some() {
            return;
        }
        let home = Self::slot_top(self.count, self.from, self.top_anchor);
        let first = first_slot(self.top_anchor);
        let last = first + (self.count.saturating_sub(1)) as f64 * SLOT_PITCH;
        self.carried_top = (home + dy).clamp(first, last);
        self.to = reorder_index(self.count, self.carried_top, self.top_anchor);
    }

    /// Where each card should sit now, as visual offsets from its slot.
    fn targets(&self) -> Vec<f64> {
        let (from_slot, to_slot) = (
            visual_slot(self.count, self.from, self.top_anchor),
            visual_slot(self.count, self.to, self.top_anchor),
        );
        (0..self.count)
            .map(|index| {
                let slot = visual_slot(self.count, index, self.top_anchor);
                if index == self.from {
                    if self.aiming || self.dropped.is_some() {
                        (to_slot as f64 - from_slot as f64) * SLOT_PITCH
                    } else {
                        self.carried_top - Self::slot_top(self.count, self.from, self.top_anchor)
                    }
                } else if from_slot < to_slot && slot > from_slot && slot <= to_slot {
                    -SLOT_PITCH
                } else if to_slot < from_slot && slot >= to_slot && slot < from_slot {
                    SLOT_PITCH
                } else {
                    0.
                }
            })
            .collect()
    }

    /// Advance to `now_ms`. Returns each card's visual y offset from its
    /// resting slot (chronological), and the carried card's lift (0…1).
    pub fn frame(&mut self, now_ms: f64) -> (Vec<f64>, f64) {
        let dt = (now_ms - self.last_ms).clamp(0., 64.);
        self.last_ms = now_ms.max(self.last_ms);
        let k = if self.reduced {
            1.
        } else {
            1. - (-dt / REORDER_GLIDE_TAU_MS).exp()
        };
        for (index, target) in self.targets().into_iter().enumerate() {
            let offset = &mut self.offsets[index];
            // The carried card follows the pointer exactly until it is let go.
            if index == self.from && !self.aiming && self.dropped.is_none() {
                *offset = target;
            } else {
                *offset += (target - *offset) * k;
                if (target - *offset).abs() < REORDER_SETTLE_EPSILON {
                    *offset = target;
                }
            }
        }
        let lift = if self.aiming {
            0.
        } else if self.reduced {
            if self.dropped.is_some() { 0. } else { 1. }
        } else if let Some(at) = self.dropped {
            1. - SETTLE.ease(((now_ms - at) / REORDER_LIFT_MS).clamp(0., 1.))
        } else {
            SETTLE.ease(((now_ms - self.began_ms) / REORDER_LIFT_MS).clamp(0., 1.))
        };
        (self.offsets.clone(), lift)
    }

    /// Let go: the carried card glides into its new slot.
    pub fn drop(&mut self, now_ms: f64) {
        if self.dropped.is_none() {
            // A hovering card has no lift to settle from.
            let lift = if self.aiming { REORDER_LIFT_MS } else { 0. };
            self.dropped = Some(now_ms - lift);
        }
    }

    /// Give up (the gesture became a file drag): everything glides home.
    pub fn cancel(&mut self, now_ms: f64) {
        self.to = self.from;
        self.drop(now_ms);
    }

    /// Whether a frame would still move anything. Only a hovering card
    /// whose stack has opened its aimed slot holds still, until it is aimed
    /// elsewhere, let go or cancelled.
    pub fn moving(&self) -> bool {
        !self.aiming || self.dropped.is_some() || self.offsets != self.targets()
    }

    /// Let go and every card has reached its slot: commit `from → to` to the
    /// stack (`PreviewStack::move_card`) and lay it out without offsets.
    pub fn settled(&self, now_ms: f64) -> bool {
        let Some(at) = self.dropped else {
            return false;
        };
        let targets = self.targets();
        (self.reduced || now_ms - at >= REORDER_LIFT_MS)
            && self.offsets.iter().zip(&targets).all(|(o, t)| o == t)
    }
}

/// One frame of Preferences' looping preview of a style: three small cards
/// in a column, the middle one dragged out and back.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Demo {
    /// Each card's top, in tile units (the tile is [`DEMO_SIZE`]), oldest
    /// first; cards are [`DEMO_CARD`] large and centred horizontally.
    pub card_tops: [f64; 3],
    /// The middle card's pose (scale and blur are in tile units).
    pub pose: CardPose,
    /// The drag image's top-left and opacity (it is [`DEMO_CARD`] large).
    pub ghost: (f64, f64),
    pub ghost_opacity: f64,
}

/// The preview tile and its cards, in points.
pub const DEMO_SIZE: (f64, f64) = (132., 96.);
pub const DEMO_CARD: (f64, f64) = (52., 26.);
const DEMO_GAP: f64 = 4.;
/// Out, hold, back, rest.
const DEMO_OUT_MS: f64 = 520.;
const DEMO_HOLD_MS: f64 = 900.;
const DEMO_BACK_MS: f64 = 420.;
pub const DEMO_LOOP_MS: f64 = 3_000.;

/// The demo of `style` at `elapsed_ms` (looping). Reduced motion shows the
/// held mid-drag frame.
pub fn demo(style: DragStyle, elapsed_ms: f64, reduced_motion: bool) -> Demo {
    let t = if reduced_motion {
        DEMO_OUT_MS + DEMO_HOLD_MS / 2.
    } else {
        elapsed_ms.rem_euclid(DEMO_LOOP_MS)
    };
    let x = (DEMO_SIZE.0 - DEMO_CARD.0) / 2. + 22.;
    let top = (DEMO_SIZE.1 - 3. * DEMO_CARD.1 - 2. * DEMO_GAP) / 2.;
    let slot = |i: usize| top + i as f64 * (DEMO_CARD.1 + DEMO_GAP);
    let home = (x, slot(1));
    let away = (10., 8.);
    // The drag out, held, then back home (a cancel shows every style's return).
    let back_at = DEMO_OUT_MS + DEMO_HOLD_MS;
    let (ghost, ghost_opacity) = if t < 60. {
        (home, 0.)
    } else if t < back_at {
        let p = SETTLE.ease(((t - 60.) / DEMO_OUT_MS).clamp(0., 1.));
        (lerp2(home, away, p), 0.92)
    } else if t < back_at + DEMO_BACK_MS {
        let p = SETTLE.ease((t - back_at) / DEMO_BACK_MS);
        (lerp2(away, home, p), 0.92)
    } else {
        (home, 0.)
    };
    let mut drag = DragOut::new(style, 60., reduced_motion);
    if t >= back_at + DEMO_BACK_MS * 0.6 {
        drag.end(DragEnd::Returned, back_at + DEMO_BACK_MS * 0.6);
    }
    let mut pose = if t < 60. {
        CardPose::REST
    } else {
        drag.pose(t)
    };
    pose.media_blur *= DEMO_CARD.0 / 284.;
    let shift = (1. - pose.slot) * (DEMO_CARD.1 + DEMO_GAP);
    Demo {
        // The demo is a bottom stack: the oldest card moves down.
        card_tops: [slot(0) + shift, slot(1), slot(2)],
        pose,
        ghost,
        ghost_opacity,
    }
}

/// The demo card's left edge in the tile.
pub fn demo_card_left() -> f64 {
    (DEMO_SIZE.0 - DEMO_CARD.0) / 2. + 22.
}

fn lerp2(a: (f64, f64), b: (f64, f64), t: f64) -> (f64, f64) {
    (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frosted_blurs_into_an_imprint_and_sharpens_back_on_return() {
        let mut drag = DragOut::new(DragStyle::Frosted, 0., false);
        assert_eq!(drag.pose(0.), CardPose::REST);
        let away = drag.pose(1_000.);
        assert_eq!(away.media_blur, FROST_BLUR);
        assert_eq!(
            (away.chrome_opacity, away.fill_opacity, away.hairline),
            (0., 0., 1.)
        );
        assert_eq!((away.opacity, away.slot), (1., 1.));
        // Ended halfway in: the return starts from where it was, not from
        // the full frost, so nothing jumps.
        let mut early = DragOut::new(DragStyle::Frosted, 0., false);
        let mid = early.pose(80.);
        early.end(DragEnd::Returned, 80.);
        assert_eq!(early.pose(80.), mid);
        drag.end(DragEnd::Returned, 1_000.);
        assert!(!drag.finished(1_000. + RETURN_MS - 1.));
        assert!(drag.finished(1_000. + RETURN_MS));
        assert_eq!(drag.pose(1_000. + RETURN_MS), CardPose::REST);
    }

    #[test]
    fn a_drop_fades_the_imprint_then_closes_its_slot_smoothly() {
        let mut drag = DragOut::new(DragStyle::Frosted, 0., false);
        drag.end(DragEnd::Dropped, 500.);
        let start = drag.pose(500.);
        assert_eq!((start.slot, start.opacity), (1., 1.));
        // The slot waits for the imprint to fade, then closes without a
        // step: no 60 Hz frame moves the stack by more than 24 points (a
        // snap would move the whole 176-point slot at once).
        assert_eq!(drag.pose(500. + SLOT_CLOSE_DELAY_MS).slot, 1.);
        let mut last = 1.;
        for frame in 0..=45 {
            let slot = drag.pose(500. + frame as f64 * 16.).slot;
            assert!(
                slot <= last && (last - slot) * SLOT_PITCH < 24.,
                "{frame}: {slot}"
            );
            last = slot;
        }
        let done = 500. + DragOut::end_ms(DragStyle::Frosted, DragEnd::Dropped);
        assert_eq!(drag.pose(done).slot, 0.);
        assert_eq!(drag.pose(done).opacity, 0.);
        assert!(drag.finished(done));
    }

    #[test]
    fn the_dock_closes_its_gap_while_away_and_reopens_it_on_return() {
        let mut drag = DragOut::new(DragStyle::Dock, 0., false);
        // The card lifts, holds a beat, and only then leaves.
        assert!(drag.pose(DOCK_LIFT_MS).scale > 1.02);
        let beat = drag.pose(DOCK_LIFT_MS + DOCK_BEAT_MS);
        assert_eq!((beat.opacity, beat.slot), (1., 1.));
        assert!(!drag.slot_closed(DOCK_LIFT_MS + DOCK_BEAT_MS));
        // The gap eases shut: slow at both ends, never a step over 24 pt
        // at 60 Hz.
        let mut last = 1.;
        let mut steps = vec![];
        for frame in 0..60 {
            let slot = drag.pose(frame as f64 * 16.).slot;
            assert!(slot <= last && (last - slot) * SLOT_PITCH < 24.);
            steps.push(last - slot);
            last = slot;
        }
        let moving: Vec<f64> = steps.into_iter().filter(|step| *step > 0.).collect();
        let peak = moving.iter().cloned().fold(0., f64::max);
        assert!(moving[0] < peak / 4. && moving[moving.len() - 1] < peak / 4.);
        let away = drag.pose(1_000.);
        assert_eq!((away.opacity, away.slot), (0., 0.));
        assert!(drag.slot_closed(1_000.));
        drag.end(DragEnd::Returned, 1_000.);
        // The slot opens before the card shows again.
        let opening = drag.pose(1_000. + DOCK_OPEN_MS * 0.4);
        assert!(opening.slot > 0.3 && opening.opacity < 0.01);
        let done = 1_000. + DragOut::end_ms(DragStyle::Dock, DragEnd::Returned);
        assert!(drag.finished(done));
        assert_eq!(drag.pose(done), CardPose::REST);
    }

    #[test]
    fn hosts_may_stop_redrawing_once_the_pose_stops_moving() {
        for style in [DragStyle::Frosted, DragStyle::Dock, DragStyle::Well] {
            let mut drag = DragOut::new(style, 0., false);
            assert!(drag.moving(10.), "{style:?} eases out");
            // Step until it stops: the pose must already be the final one.
            let still = (0..200)
                .map(|i| i as f64 * 10.)
                .find(|t| !drag.moving(*t))
                .unwrap();
            assert_eq!(
                drag.pose(still),
                drag.pose(60_000.),
                "{style:?} stopped early"
            );
            assert_ne!(
                drag.pose(still - 20.),
                drag.pose(60_000.),
                "{style:?} stopped late"
            );
            drag.end(DragEnd::Returned, 5_000.);
            assert!(drag.moving(5_010.), "{style:?} plays its return");
            let back = 5_000. + DragOut::end_ms(style, DragEnd::Returned);
            assert!(!drag.moving(back));
            assert!(
                !DragOut::new(style, 0., true).moving(0.),
                "reduced motion holds still"
            );
        }
    }

    #[test]
    fn the_well_empties_into_a_tile_with_a_glyph() {
        let drag = DragOut::new(DragStyle::Well, 0., false);
        let away = drag.pose(1_000.);
        assert_eq!(
            (away.media_opacity, away.well, away.glyph, away.glyph_scale),
            (0., 1., 1., 1.)
        );
        assert_eq!(away.slot, 1.);
    }

    #[test]
    fn reduced_motion_jumps_to_each_state_and_ends_at_once() {
        for style in [DragStyle::Frosted, DragStyle::Dock, DragStyle::Well] {
            let mut drag = DragOut::new(style, 0., true);
            assert_eq!(drag.pose(0.), CardPose::away(style), "{style:?}");
            drag.end(DragEnd::Returned, 10.);
            assert!(drag.finished(10.));
            assert_eq!(drag.pose(10.), CardPose::REST);
        }
    }

    #[test]
    fn closing_slots_pull_older_cards_toward_the_anchored_edge() {
        assert_eq!(closing_shift(1., false), 0.);
        assert_eq!(closing_shift(0., false), SLOT_PITCH);
        assert_eq!(closing_shift(0.5, true), -SLOT_PITCH / 2.);
    }

    #[test]
    fn reorder_targets_follow_the_stacks_orientation() {
        // Bottom stack: oldest at the top. Top stack: newest at the top.
        let bottom = |i| Reorder::slot_top(4, i, false);
        let top = |i| Reorder::slot_top(4, i, true);
        assert!(bottom(0) < bottom(3));
        assert!(top(3) < top(0));
        for i in 0..4 {
            assert_eq!(reorder_index(4, bottom(i) + 20., false), i);
            assert_eq!(reorder_index(4, top(i) - 20., true), i);
        }
        // Past either end clamps to the end slots.
        assert_eq!(reorder_index(4, -1_000., false), 0);
        assert_eq!(reorder_index(4, 9_000., true), 0);
    }

    #[test]
    fn carrying_a_card_down_a_bottom_stack_slides_the_cards_it_passes_up() {
        let mut reorder = Reorder::new(4, 0, false, 0., false);
        reorder.pointer(2. * SLOT_PITCH + 10.);
        assert_eq!(reorder.to(), 2);
        let mut offsets = vec![];
        for frame in 1..=40 {
            offsets = reorder.frame(frame as f64 * 16.).0;
        }
        assert_eq!(
            offsets,
            vec![2. * SLOT_PITCH + 10., -SLOT_PITCH, -SLOT_PITCH, 0.]
        );
        // Neighbours glide rather than jump: the first frame after a target
        // change moves them only part of the way.
        reorder.pointer(0.);
        let (glide, _) = reorder.frame(40. * 16. + 16.);
        assert!(glide[1] < -SLOT_PITCH / 3. && glide[1] > -SLOT_PITCH);
        reorder.pointer(3. * SLOT_PITCH);
        reorder.drop(700.);
        assert!(!reorder.settled(700.));
        let mut now = 700.;
        while !reorder.settled(now) {
            now += 16.;
            reorder.frame(now);
            assert!(now < 2_000.);
        }
        assert_eq!(reorder.to(), 3);
    }

    #[test]
    fn carrying_up_a_top_stack_and_cancelling_glides_everything_home() {
        let mut reorder = Reorder::new(3, 0, true, 0., false);
        // Index 0 is the oldest, at the bottom of a top stack: carry it up.
        reorder.pointer(-2. * SLOT_PITCH);
        assert_eq!(reorder.to(), 2);
        for frame in 1..=30 {
            reorder.frame(frame as f64 * 16.);
        }
        let (offsets, lift) = reorder.frame(500.);
        assert_eq!(offsets, vec![-2. * SLOT_PITCH, SLOT_PITCH, SLOT_PITCH]);
        assert_eq!(lift, 1.);
        reorder.cancel(500.);
        assert_eq!(reorder.to(), 0);
        let mut now = 500.;
        while !reorder.settled(now) {
            now += 16.;
            reorder.frame(now);
        }
        assert_eq!(reorder.frame(now).0, vec![0.; 3]);
    }

    #[test]
    fn a_file_dropped_back_on_the_stack_lands_in_the_slot_under_the_pointer() {
        // The pointer anywhere over a slot picks that slot, in either
        // orientation; past the ends clamps.
        for top in [false, true] {
            for index in 0..4 {
                let middle = Reorder::slot_top(4, index, top) + THUMBNAIL_CARD_HEIGHT / 2.;
                for dy in [
                    -THUMBNAIL_CARD_HEIGHT / 2. + 1.,
                    0.,
                    THUMBNAIL_CARD_HEIGHT / 2. - 1.,
                ] {
                    assert_eq!(drop_index(4, middle + dy, top), index, "{top} {index}");
                }
            }
        }
        assert_eq!(drop_index(4, -500., false), 0);
        assert_eq!(drop_index(4, -500., true), 3);
        // Only a dock whose gap has closed moves the card before it returns.
        assert!(!DragOut::new(DragStyle::Frosted, 0., false).slot_closed(1_000.));
        assert!(!DragOut::new(DragStyle::Well, 0., false).slot_closed(1_000.));
        // A landing glides without a lift and commits where it was dropped.
        let mut landing = Reorder::landing(4, 3, 1, false, 100., false);
        let (offsets, lift) = landing.frame(116.);
        assert_eq!(lift, 0.);
        assert!(offsets[3] < 0. && offsets[3] > -2. * SLOT_PITCH);
        assert!(offsets[1] > 0. && offsets[1] < SLOT_PITCH);
        let mut now = 116.;
        while !landing.settled(now) {
            now += 16.;
            assert_eq!(landing.frame(now).1, 0.);
            assert!(now < 1_000.);
        }
        assert_eq!((landing.from(), landing.to()), (3, 1));
        assert_eq!(
            landing.frame(now).0,
            vec![0., SLOT_PITCH, SLOT_PITCH, -2. * SLOT_PITCH]
        );
    }

    #[test]
    fn a_file_back_over_its_stack_shows_its_card_in_the_slot_it_would_land_in() {
        // The dock's card has left and its gap closed when the file comes
        // back over the stack: the card re-forms with its slot open.
        let mut drag = DragOut::new(DragStyle::Dock, 0., false);
        assert!(drag.slot_closed(1_000.));
        drag.hover(true, 1_000.);
        assert!(drag.moving(1_100.));
        let back = drag.pose(2_000.);
        assert_eq!((back.opacity, back.slot), (1., 1.));
        assert!(!drag.moving(2_000.));
        // Leaving again eases back into the imprint; returning again re-forms.
        drag.hover(false, 2_000.);
        assert_eq!(drag.pose(3_000.), CardPose::away(DragStyle::Dock));
        drag.hover(true, 3_000.);
        // Dropped while showing, it returns from where it is.
        drag.end(DragEnd::Returned, 4_000.);
        assert!(!drag.slot_closed(4_000.));
        assert_eq!(drag.pose(4_000.).opacity, 1.);

        // A bottom stack of four: card 3 hovers over slot 1, the cards it
        // would pass make room and it waits there, without a lift.
        let mut hover = Reorder::hovering(4, 3, false, 0., false);
        hover.pointer(-500.);
        hover.aim(1);
        let mut now = 0.;
        while hover.moving() {
            now += 16.;
            assert_eq!(hover.frame(now).1, 0.);
            assert!(now < 1_000.);
        }
        let open = vec![0., SLOT_PITCH, SLOT_PITCH, -2. * SLOT_PITCH];
        assert_eq!(hover.frame(now).0, open);
        assert!(!hover.settled(now + 1_000.));
        // Off the stack it aims home and everything glides back; aimed again
        // and let go, it lands there.
        hover.aim(3);
        assert!(hover.moving());
        hover.aim(1);
        hover.drop(now);
        while !hover.settled(now) {
            now += 16.;
            hover.frame(now);
            assert!(now < 2_000.);
        }
        assert_eq!((hover.from(), hover.to()), (3, 1));
        assert_eq!(hover.frame(now).0, open);
    }

    #[test]
    fn mostly_vertical_presses_reorder_and_others_drag_the_file() {
        assert_eq!(press_intent(1., 2., 3), None);
        assert_eq!(press_intent(1., 6., 3), Some(PressIntent::Reorder));
        assert_eq!(press_intent(6., 1., 3), Some(PressIntent::FileDrag));
        // A lone card has nowhere to go.
        assert_eq!(press_intent(0., 8., 1), Some(PressIntent::FileDrag));
        assert!(!reorder_escapes(10., -5.));
        assert!(reorder_escapes(30., 0.));
        assert!(reorder_escapes(0., 30.));
    }

    #[test]
    fn the_demo_loops_through_a_drag_out_and_back() {
        for style in [DragStyle::Frosted, DragStyle::Dock, DragStyle::Well] {
            let rest = demo(style, 0., false);
            assert_eq!(rest.pose, CardPose::REST);
            assert_eq!(rest.ghost_opacity, 0.);
            let out = demo(style, DEMO_OUT_MS + 300., false);
            assert_eq!(out.ghost_opacity, 0.92);
            assert_eq!(out.pose.slot, CardPose::away(style).slot);
            assert_eq!(demo(style, DEMO_LOOP_MS - 1., false).pose, CardPose::REST);
            assert_eq!(demo(style, DEMO_LOOP_MS, false), rest);
            let held = demo(style, 0., true);
            assert_eq!(held.ghost_opacity, 0.92);
        }
        // The dock demo closes its gap toward the bottom.
        let dock = demo(DragStyle::Dock, DEMO_OUT_MS + 600., false);
        assert!(dock.card_tops[0] > demo(DragStyle::Dock, 0., false).card_tops[0]);
    }
}
