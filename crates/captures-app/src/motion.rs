//! Shipping motion as data both native hosts play.
//!
//! Ported from the Tauri UI's CSS (`apps/desktop/ui/src/styles/*.css`): each
//! [`Motion`] is one `@keyframes` rule with its `animation` shorthand, and each
//! [`Transition`] is one `transition` declaration. Durations and easings name
//! the `--dur-*` / `--ease-*` design tokens where shipping does; hosts resolve
//! them against their build-generated token table, so a token change moves
//! every native host with the web UI. Literal values appear only where the
//! shipping CSS itself hard-codes them.
//!
//! Reduced motion follows the shipping global rule in `base.css`
//! (`animation-duration: 0.01ms; transition-duration: 0.01ms`): an animation
//! keeps its delay, then lands on its final keyframe; a transition lands on its
//! target. Hosts paint nothing in between, so they schedule no repaints.

use serde::Serialize;

/// Transform and opacity of one keyframe. `translate_y` is in logical points,
/// positive downward as in CSS; `scale` is about the element's centre; `blur` is
/// a CSS `filter: blur()` radius that hosts may approximate or omit.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Pose {
    pub opacity: f64,
    pub translate_y: f64,
    pub scale: f64,
    pub blur: f64,
}

impl Pose {
    /// The element's own style: fully visible, untransformed.
    pub const REST: Self = Self {
        opacity: 1.,
        translate_y: 0.,
        scale: 1.,
        blur: 0.,
    };

    const fn hidden(translate_y: f64, scale: f64) -> Self {
        Self {
            opacity: 0.,
            translate_y,
            scale,
            blur: 0.,
        }
    }

    fn lerp(self, other: Self, t: f64) -> Self {
        let mix = |a: f64, b: f64| a + (b - a) * t;
        Self {
            opacity: mix(self.opacity, other.opacity),
            translate_y: mix(self.translate_y, other.translate_y),
            scale: mix(self.scale, other.scale),
            blur: mix(self.blur, other.blur),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Keyframe {
    /// 0...1 of the animation duration, like a CSS keyframe selector.
    pub offset: f64,
    #[serde(flatten)]
    pub pose: Pose,
}

const fn frame(offset: f64, pose: Pose) -> Keyframe {
    Keyframe { offset, pose }
}

/// A duration as shipping writes it: a `--dur-*` token or a literal.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Timing {
    Token(&'static str),
    Millis(f64),
}

/// A timing function as shipping writes it: an `--ease-*` token or a literal
/// `cubic-bezier()` (CSS `linear` is `[0, 0, 1, 1]`, `ease-out` keyword
/// `[0, 0, 0.58, 1]`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Easing {
    Token(&'static str),
    Bezier([f64; 4]),
}

/// One shipping `@keyframes` rule plus its `animation` timing.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Keyframes {
    pub duration: Timing,
    pub delay_ms: f64,
    /// Applied to every segment between keyframes, as CSS does.
    pub easing: Easing,
    pub frames: &'static [Keyframe],
}

/// One shipping `transition` declaration.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct TransitionSpec {
    pub duration: Timing,
    pub easing: Easing,
}

const POP_IN: &[Keyframe] = &[
    frame(
        0.,
        Pose {
            opacity: 0.,
            translate_y: -4.,
            scale: 0.985,
            blur: 0.,
        },
    ),
    frame(1., Pose::REST),
];

const RESTART_EXIT: &[Keyframe] = &[frame(0., Pose::REST), frame(1., Pose::hidden(-6., 1.))];
const STARTUP_ARRIVE: &[Keyframe] = &[frame(0., Pose::hidden(-8., 0.97)), frame(1., Pose::REST)];
const STARTUP_ARRIVE_FROM_BELOW: &[Keyframe] =
    &[frame(0., Pose::hidden(8., 0.97)), frame(1., Pose::REST)];
const SAVED_LIFECYCLE: &[Keyframe] = &[
    frame(0., Pose::hidden(-8., 0.97)),
    frame(0.025, Pose::REST),
    frame(0.86, Pose::REST),
    frame(1., Pose::hidden(-5., 0.985)),
];
const HIDDEN_LIFECYCLE: &[Keyframe] = &[
    frame(0., Pose::hidden(7., 0.97)),
    frame(0.05, Pose::REST),
    frame(0.8, Pose::REST),
    frame(1., Pose::hidden(5., 0.985)),
];
const THUMBNAIL_ARRIVE: &[Keyframe] = &[
    frame(
        0.,
        Pose {
            opacity: 0.,
            translate_y: 24.,
            scale: 0.975,
            blur: 3.,
        },
    ),
    frame(1., Pose::REST),
];
const OPTIONS_ARRIVE: &[Keyframe] = &[frame(0., Pose::hidden(-5., 1.)), frame(1., Pose::REST)];
const FADE_IN: &[Keyframe] = &[frame(0., Pose::hidden(0., 1.)), frame(1., Pose::REST)];
const FADE_OUT: &[Keyframe] = &[frame(0., Pose::REST), frame(1., Pose::hidden(0., 1.))];
const CONTENT_IN: &[Keyframe] = &[frame(0., Pose::hidden(0., 0.94)), frame(1., Pose::REST)];
const CONTENT_OUT: &[Keyframe] = &[frame(0., Pose::REST), frame(1., Pose::hidden(0., 1.035))];

/// Shipping entrance, exit and lifecycle animations the native hosts play.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Motion {
    /// `.update-notice`: `ui-pop-in var(--dur-4) var(--ease-out)`.
    UpdateNoticeIn,
    /// `.update-notice:has(.update-restarting)`: `update-restart-exit
    /// var(--dur-4) var(--ease-in) 3s forwards`, timed from the restart state.
    UpdateNoticeRestartExit,
    /// `.startup-notice`: `startup-arrive var(--dur-4) var(--ease-out)`.
    StartupNoticeIn,
    /// `.startup-notice[data-caret="bottom"]`: `startup-arrive-from-below`.
    StartupNoticeInFromBelow,
    /// `.recording-saved-notice`: `recording-saved-lifecycle 15s
    /// var(--ease-standard) forwards`.
    RecordingSavedLifecycle,
    /// `.recording-controls-hidden-notice`: `recording-controls-hidden-lifecycle
    /// 6s var(--ease-standard) forwards`.
    RecordingControlsHiddenLifecycle,
    /// `.thumbnail-card.thumbnail-ready`: `thumbnail-arrive 0.52s
    /// cubic-bezier(0.22, 0.65, 0.28, 1)`.
    PreviewCardArrive,
    /// `.recording-options-row`: `recording-options-arrive var(--dur-3) 40ms
    /// var(--ease-standard) both`, the capture menu's Record options panel.
    CaptureMenuOptionsArrive,
    /// `.recording-countdown`: `recording-countdown-fade-in var(--dur-3)
    /// var(--ease-out) both` (the scrim).
    CountdownIn,
    /// `.recording-countdown-content`: `recording-countdown-content-in
    /// var(--dur-4) var(--ease-out) both`.
    CountdownContentIn,
    /// `.recording-countdown.exiting`: `recording-countdown-fade-out
    /// var(--dur-2) var(--ease-in) both`.
    CountdownOut,
    /// `.recording-countdown.exiting .recording-countdown-content`:
    /// `recording-countdown-content-out var(--dur-2) var(--ease-in) both`.
    CountdownContentOut,
    /// `.custom-select-listbox` and `.preferences-save-status`:
    /// `ui-pop-in var(--dur-2) var(--ease-out)`.
    PopoverIn,
}

impl Motion {
    pub const ALL: [Self; 13] = [
        Self::UpdateNoticeIn,
        Self::UpdateNoticeRestartExit,
        Self::StartupNoticeIn,
        Self::StartupNoticeInFromBelow,
        Self::RecordingSavedLifecycle,
        Self::RecordingControlsHiddenLifecycle,
        Self::PreviewCardArrive,
        Self::CaptureMenuOptionsArrive,
        Self::CountdownIn,
        Self::CountdownContentIn,
        Self::CountdownOut,
        Self::CountdownContentOut,
        Self::PopoverIn,
    ];

    /// Stable name used by the settings ABI.
    pub fn name(self) -> &'static str {
        match self {
            Self::UpdateNoticeIn => "update_notice_in",
            Self::UpdateNoticeRestartExit => "update_notice_restart_exit",
            Self::StartupNoticeIn => "startup_notice_in",
            Self::StartupNoticeInFromBelow => "startup_notice_in_from_below",
            Self::RecordingSavedLifecycle => "recording_saved_lifecycle",
            Self::RecordingControlsHiddenLifecycle => "recording_controls_hidden_lifecycle",
            Self::PreviewCardArrive => "preview_card_arrive",
            Self::CaptureMenuOptionsArrive => "capture_menu_options_arrive",
            Self::CountdownIn => "countdown_in",
            Self::CountdownContentIn => "countdown_content_in",
            Self::CountdownOut => "countdown_out",
            Self::CountdownContentOut => "countdown_content_out",
            Self::PopoverIn => "popover_in",
        }
    }

    pub fn keyframes(self) -> Keyframes {
        let spec = |duration, delay_ms, easing, frames| Keyframes {
            duration,
            delay_ms,
            easing,
            frames,
        };
        use Easing::{Bezier, Token as Ease};
        use Timing::{Millis, Token as Dur};
        match self {
            Self::UpdateNoticeIn => spec(Dur("dur-4"), 0., Ease("ease-out"), POP_IN),
            Self::UpdateNoticeRestartExit => {
                spec(Dur("dur-4"), 3_000., Ease("ease-in"), RESTART_EXIT)
            }
            Self::StartupNoticeIn => spec(Dur("dur-4"), 0., Ease("ease-out"), STARTUP_ARRIVE),
            Self::StartupNoticeInFromBelow => spec(
                Dur("dur-4"),
                0.,
                Ease("ease-out"),
                STARTUP_ARRIVE_FROM_BELOW,
            ),
            Self::RecordingSavedLifecycle => {
                spec(Millis(15_000.), 0., Ease("ease-standard"), SAVED_LIFECYCLE)
            }
            Self::RecordingControlsHiddenLifecycle => {
                spec(Millis(6_000.), 0., Ease("ease-standard"), HIDDEN_LIFECYCLE)
            }
            Self::PreviewCardArrive => spec(
                Millis(520.),
                0.,
                Bezier([0.22, 0.65, 0.28, 1.]),
                THUMBNAIL_ARRIVE,
            ),
            Self::CaptureMenuOptionsArrive => {
                spec(Dur("dur-3"), 40., Ease("ease-standard"), OPTIONS_ARRIVE)
            }
            Self::CountdownIn => spec(Dur("dur-3"), 0., Ease("ease-out"), FADE_IN),
            Self::CountdownContentIn => spec(Dur("dur-4"), 0., Ease("ease-out"), CONTENT_IN),
            Self::CountdownOut => spec(Dur("dur-2"), 0., Ease("ease-in"), FADE_OUT),
            Self::CountdownContentOut => spec(Dur("dur-2"), 0., Ease("ease-in"), CONTENT_OUT),
            Self::PopoverIn => spec(Dur("dur-2"), 0., Ease("ease-out"), POP_IN),
        }
    }

    /// Resolve against a host's token table. `None` names a missing token.
    pub fn resolve(self, tokens: &impl MotionTokens) -> Option<Animation> {
        let spec = self.keyframes();
        Some(Animation {
            duration_ms: tokens.resolve_timing(spec.duration)?,
            delay_ms: spec.delay_ms,
            easing: tokens.resolve_easing(spec.easing)?,
            frames: spec.frames,
            lifecycle: matches!(
                self,
                Self::RecordingSavedLifecycle | Self::RecordingControlsHiddenLifecycle
            ),
        })
    }
}

/// Shipping state-change transitions the native hosts play.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Transition {
    /// `.capture-segmented-indicator.ready`: width and transform over
    /// `var(--dur-4) var(--ease-standard)` (Preferences and the capture menu).
    SegmentedIndicator,
    /// `.history-card`: border, `translateY(-2px)` and shadow over
    /// `var(--dur-3) var(--ease-standard)` on hover.
    HistoryCardHover,
    /// `.recording-tooltip > [role="tooltip"]`: opacity and a 3 pt slide over
    /// `var(--dur-1)`, `var(--ease-standard)`.
    Tooltip,
    /// `.thumbnail-card img`: the hover `filter` (blur and brightness) over
    /// `0.18s ease`.
    PreviewMediaFilter,
    /// `.thumbnail-card img`: the hover `transform: scale()` over `0.22s ease`.
    PreviewMediaScale,
    /// `.icon-button::after`: card icon tooltips fade and nudge over `0.12s ease`.
    PreviewIconTooltip,
    /// `.thumbnail-stack-control[data-tooltip]::after`: the stack toolbar tip
    /// over `var(--dur-1) var(--ease-out)`.
    PreviewStackTooltip,
    /// `.thumbnail-card`: the editor ring's `box-shadow` arrives over
    /// `0.22s ease`.
    PreviewEditorRing,
    /// `.thumbnail-editor-leaving`: the ring eases out over
    /// `0.55s var(--ease-standard)`.
    PreviewEditorRingLeave,
    /// `.thumbnail-editor-control`: Edit ↔ "In editor" width, padding and
    /// colour morph over `0.28s cubic-bezier(0.2, 0.8, 0.2, 1)`.
    PreviewEditorMorph,
}

/// CSS `ease` keyword.
const CSS_EASE: Easing = Easing::Bezier([0.25, 0.1, 0.25, 1.]);

impl Transition {
    pub const ALL: [Self; 10] = [
        Self::SegmentedIndicator,
        Self::HistoryCardHover,
        Self::Tooltip,
        Self::PreviewMediaFilter,
        Self::PreviewMediaScale,
        Self::PreviewIconTooltip,
        Self::PreviewStackTooltip,
        Self::PreviewEditorRing,
        Self::PreviewEditorRingLeave,
        Self::PreviewEditorMorph,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::SegmentedIndicator => "segmented_indicator",
            Self::HistoryCardHover => "history_card_hover",
            Self::Tooltip => "tooltip",
            Self::PreviewMediaFilter => "preview_media_filter",
            Self::PreviewMediaScale => "preview_media_scale",
            Self::PreviewIconTooltip => "preview_icon_tooltip",
            Self::PreviewStackTooltip => "preview_stack_tooltip",
            Self::PreviewEditorRing => "preview_editor_ring",
            Self::PreviewEditorRingLeave => "preview_editor_ring_leave",
            Self::PreviewEditorMorph => "preview_editor_morph",
        }
    }

    pub fn spec(self) -> TransitionSpec {
        let token = |duration, easing| (Timing::Token(duration), Easing::Token(easing));
        let (duration, easing) = match self {
            Self::SegmentedIndicator => token("dur-4", "ease-standard"),
            Self::HistoryCardHover => token("dur-3", "ease-standard"),
            Self::Tooltip => token("dur-1", "ease-standard"),
            Self::PreviewMediaFilter => (Timing::Millis(180.), CSS_EASE),
            Self::PreviewMediaScale | Self::PreviewEditorRing => (Timing::Millis(220.), CSS_EASE),
            Self::PreviewIconTooltip => (Timing::Millis(120.), CSS_EASE),
            Self::PreviewStackTooltip => token("dur-1", "ease-out"),
            Self::PreviewEditorRingLeave => (Timing::Millis(550.), Easing::Token("ease-standard")),
            Self::PreviewEditorMorph => (Timing::Millis(280.), Easing::Bezier([0.2, 0.8, 0.2, 1.])),
        };
        TransitionSpec { duration, easing }
    }

    pub fn resolve(self, tokens: &impl MotionTokens) -> Option<Tween> {
        let spec = self.spec();
        Some(Tween {
            duration_ms: tokens.resolve_timing(spec.duration)?,
            easing: tokens.resolve_easing(spec.easing)?,
        })
    }
}

/// A host's view of its token table: `--dur-*` in milliseconds and
/// `--ease-*` as cubic-bezier control points.
pub trait MotionTokens {
    fn duration_ms(&self, token: &str) -> Option<f64>;
    fn easing(&self, token: &str) -> Option<[f64; 4]>;

    fn resolve_timing(&self, timing: Timing) -> Option<f64> {
        match timing {
            Timing::Token(name) => self.duration_ms(name),
            Timing::Millis(ms) => Some(ms),
        }
    }

    fn resolve_easing(&self, easing: Easing) -> Option<CubicBezier> {
        let [x1, y1, x2, y2] = match easing {
            Easing::Token(name) => self.easing(name)?,
            Easing::Bezier(points) => points,
        };
        CubicBezier::new(x1, y1, x2, y2)
    }
}

/// A resolved [`Motion`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Animation {
    pub duration_ms: f64,
    pub delay_ms: f64,
    pub easing: CubicBezier,
    pub frames: &'static [Keyframe],
    lifecycle: bool,
}

impl Animation {
    /// When the last keyframe is reached, including the delay.
    pub fn total_ms(&self, reduced_motion: bool) -> f64 {
        self.delay_ms + if reduced_motion { 0. } else { self.duration_ms }
    }

    /// True while a host must keep painting frames.
    pub fn running(&self, elapsed_ms: f64, reduced_motion: bool) -> bool {
        if reduced_motion {
            return false;
        }
        elapsed_ms < self.total_ms(false)
    }

    /// Pose `elapsed_ms` after the animation starts. Before the delay it holds
    /// the first keyframe (`fill-mode: both`); afterwards the last.
    pub fn pose_at(&self, elapsed_ms: f64, reduced_motion: bool) -> Pose {
        let first = self.frames[0].pose;
        let last = self.frames[self.frames.len() - 1].pose;
        if elapsed_ms.is_nan() || elapsed_ms < self.delay_ms {
            return first;
        }
        if reduced_motion || self.duration_ms <= 0. {
            return last;
        }
        let progress = ((elapsed_ms - self.delay_ms) / self.duration_ms).clamp(0., 1.);
        for pair in self.frames.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            if progress <= b.offset {
                let span = b.offset - a.offset;
                let local = if span <= 0. {
                    1.
                } else {
                    (progress - a.offset) / span
                };
                return a.pose.lerp(b.pose, self.easing.ease(local));
            }
        }
        last
    }

    /// Pose of a lifecycle notice whose window lives `since_start_ms` after it
    /// appeared and closes in `until_end_ms` (`None` while it is held open,
    /// such as during a save). The entrance plays from the start and the exit
    /// ends at the close, so a host that extends a notice's life keeps it
    /// steady in between. Under reduced motion the notice stays still and
    /// visible until its window closes: shipping's global 0.01 ms rule would
    /// jump straight to the invisible final keyframe, which this does not copy.
    pub fn lifecycle_pose(
        &self,
        since_start_ms: f64,
        until_end_ms: Option<f64>,
        reduced_motion: bool,
    ) -> Pose {
        if reduced_motion || !self.lifecycle {
            return if self.lifecycle {
                Pose::REST
            } else {
                self.pose_at(since_start_ms, reduced_motion)
            };
        }
        let hold = self.hold_ms();
        if let Some(until) = until_end_ms {
            let from_end = self.duration_ms - until.max(0.);
            if from_end >= hold.1 {
                return self.pose_at(from_end, false);
            }
        }
        self.pose_at(since_start_ms.min(hold.0), false)
    }

    /// Whether a lifecycle notice is between keyframes.
    pub fn lifecycle_running(
        &self,
        since_start_ms: f64,
        until_end_ms: Option<f64>,
        reduced_motion: bool,
    ) -> bool {
        if reduced_motion {
            return false;
        }
        let hold = self.hold_ms();
        since_start_ms < hold.0
            || until_end_ms.is_some_and(|until| self.duration_ms - until >= hold.1)
    }

    /// Milliseconds until a lifecycle notice's exit begins, for scheduling one
    /// wake-up instead of painting the steady middle.
    pub fn lifecycle_exit_in(&self, until_end_ms: f64) -> f64 {
        (until_end_ms - (self.duration_ms - self.hold_ms().1)).max(0.)
    }

    /// Start and end of the steady middle of a lifecycle, in milliseconds.
    fn hold_ms(&self) -> (f64, f64) {
        let rest = |frame: &&Keyframe| frame.pose == Pose::REST;
        let start = self.frames.iter().find(rest).map_or(0., |f| f.offset);
        let end = self.frames.iter().rev().find(rest).map_or(1., |f| f.offset);
        (start * self.duration_ms, end * self.duration_ms)
    }
}

/// A resolved [`Transition`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tween {
    pub duration_ms: f64,
    pub easing: CubicBezier,
}

impl Tween {
    /// Eased 0...1 progress `elapsed_ms` after the change.
    pub fn progress(&self, elapsed_ms: f64, reduced_motion: bool) -> f64 {
        if reduced_motion || self.duration_ms <= 0. || elapsed_ms >= self.duration_ms {
            return 1.;
        }
        self.easing.ease((elapsed_ms / self.duration_ms).max(0.))
    }

    pub fn running(&self, elapsed_ms: f64, reduced_motion: bool) -> bool {
        !reduced_motion && elapsed_ms < self.duration_ms
    }

    /// Value between `from` and `to`.
    pub fn value(&self, from: f64, to: f64, elapsed_ms: f64, reduced_motion: bool) -> f64 {
        from + (to - from) * self.progress(elapsed_ms, reduced_motion)
    }
}

/// CSS `cubic-bezier(x1, y1, x2, y2)`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct CubicBezier {
    pub x1: f64,
    pub y1: f64,
    pub x2: f64,
    pub y2: f64,
}

impl CubicBezier {
    pub const LINEAR: Self = Self {
        x1: 0.,
        y1: 0.,
        x2: 1.,
        y2: 1.,
    };

    /// CSS requires x in 0...1 so the curve is a function of time.
    pub fn new(x1: f64, y1: f64, x2: f64, y2: f64) -> Option<Self> {
        let valid = [x1, y1, x2, y2].iter().all(|v| v.is_finite())
            && (0. ..=1.).contains(&x1)
            && (0. ..=1.).contains(&x2);
        valid.then_some(Self { x1, y1, x2, y2 })
    }

    /// Parse a shipping token value such as `cubic-bezier(0.16, 1, 0.3, 1)`.
    pub fn parse(css: &str) -> Option<Self> {
        let css = css.trim();
        if css == "linear" {
            return Some(Self::LINEAR);
        }
        let inner = css.strip_prefix("cubic-bezier(")?.strip_suffix(')')?;
        let values: Vec<f64> = inner
            .split(',')
            .map(|v| v.trim().parse().ok())
            .collect::<Option<_>>()?;
        let [x1, y1, x2, y2] = values.as_slice() else {
            return None;
        };
        Self::new(*x1, *y1, *x2, *y2)
    }

    fn sample(a1: f64, a2: f64, t: f64) -> f64 {
        // Bernstein form with P0 = 0 and P3 = 1.
        let u = 1. - t;
        3. * u * u * t * a1 + 3. * u * t * t * a2 + t * t * t
    }

    fn slope(a1: f64, a2: f64, t: f64) -> f64 {
        let u = 1. - t;
        3. * u * u * a1 + 6. * u * t * (a2 - a1) + 3. * t * t * (1. - a2)
    }

    /// Eased output for input progress `x` in 0...1.
    pub fn ease(&self, x: f64) -> f64 {
        if x <= 0. {
            return 0.;
        }
        if x >= 1. {
            return 1.;
        }
        // Newton's method, falling back to bisection, as browsers do.
        let mut t = x;
        for _ in 0..8 {
            let error = Self::sample(self.x1, self.x2, t) - x;
            if error.abs() < 1e-7 {
                return Self::sample(self.y1, self.y2, t);
            }
            let slope = Self::slope(self.x1, self.x2, t);
            if slope.abs() < 1e-6 {
                break;
            }
            t -= error / slope;
        }
        let (mut low, mut high) = (0., 1.);
        t = x;
        for _ in 0..60 {
            let value = Self::sample(self.x1, self.x2, t);
            if (value - x).abs() < 1e-7 {
                break;
            }
            if value < x {
                low = t;
            } else {
                high = t;
            }
            t = (low + high) / 2.;
        }
        Self::sample(self.y1, self.y2, t)
    }
}

/// The settings ABI's `motion` payload: every shipping animation and
/// transition with its token names, for hosts that hand keyframes to a system
/// animator (Core Animation) instead of sampling poses.
pub fn catalog() -> serde_json::Value {
    let keyframes: serde_json::Map<_, _> = Motion::ALL
        .iter()
        .map(|m| (m.name().into(), serde_json::json!(m.keyframes())))
        .collect();
    let transitions: serde_json::Map<_, _> = Transition::ALL
        .iter()
        .map(|t| (t.name().into(), serde_json::json!(t.spec())))
        .collect();
    serde_json::json!({ "keyframes": keyframes, "transitions": transitions })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shipping `shared/design.css` values.
    struct Shipping;
    impl MotionTokens for Shipping {
        fn duration_ms(&self, token: &str) -> Option<f64> {
            Some(match token {
                "dur-1" => 90.,
                "dur-2" => 140.,
                "dur-3" => 200.,
                "dur-4" => 280.,
                "dur-5" => 420.,
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

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn bezier_matches_css_reference_points() {
        let linear = CubicBezier::LINEAR;
        assert!(close(linear.ease(0.3), 0.3));
        // CSS `ease` at 50% ≈ 0.8024 (WebKit UnitBezier).
        let ease = CubicBezier::new(0.25, 0.1, 0.25, 1.).unwrap();
        assert!((ease.ease(0.5) - 0.8024).abs() < 1e-3);
        let out = CubicBezier::parse("cubic-bezier(0.16, 1, 0.3, 1)").unwrap();
        assert!(out.ease(0.25) > 0.7, "ease-out front-loads");
        assert_eq!(out.ease(0.), 0.);
        assert_eq!(out.ease(1.), 1.);
        assert_eq!(CubicBezier::parse("linear"), Some(CubicBezier::LINEAR));
        assert_eq!(CubicBezier::parse("cubic-bezier(1.2, 0, 0, 1)"), None);
        assert_eq!(CubicBezier::parse("ease"), None);
        assert_eq!(CubicBezier::parse("cubic-bezier(0, 0, 1)"), None);
        let mut previous = 0.;
        for step in 0..=100 {
            let value = out.ease(f64::from(step) / 100.);
            assert!(value + 1e-9 >= previous, "monotonic");
            previous = value;
        }
    }

    #[test]
    fn every_motion_and_transition_resolves_against_shipping_tokens() {
        for motion in Motion::ALL {
            let animation = motion
                .resolve(&Shipping)
                .unwrap_or_else(|| panic!("{}", motion.name()));
            assert!(animation.duration_ms > 0.);
            assert_eq!(animation.frames.first().unwrap().offset, 0.);
            assert_eq!(animation.frames.last().unwrap().offset, 1.);
        }
        for transition in Transition::ALL {
            assert!(transition.resolve(&Shipping).is_some());
        }
        let catalog = catalog();
        assert_eq!(
            catalog["keyframes"]["update_notice_in"]["duration"]["token"],
            "dur-4"
        );
        assert_eq!(
            catalog["keyframes"]["preview_card_arrive"]["duration"]["millis"],
            520.
        );
        assert_eq!(
            catalog["keyframes"]["preview_card_arrive"]["frames"][0]["translate_y"],
            24.
        );
        assert_eq!(
            catalog["transitions"]["segmented_indicator"]["easing"]["token"],
            "ease-standard"
        );
        assert_eq!(
            catalog["transitions"]["preview_media_filter"]["duration"]["millis"],
            180.
        );
        assert_eq!(
            catalog["transitions"]["preview_media_filter"]["easing"]["bezier"][1],
            0.1
        );
        // The ring leave matches the shipping presence leave timer.
        assert_eq!(
            Transition::PreviewEditorRingLeave
                .resolve(&Shipping)
                .unwrap()
                .duration_ms,
            crate::preview_chrome::EDITOR_PRESENCE_LEAVE_MS
        );
    }

    #[test]
    fn pop_in_starts_hidden_and_settles_at_rest() {
        let pop = Motion::UpdateNoticeIn.resolve(&Shipping).unwrap();
        assert_eq!(pop.duration_ms, 280.);
        let start = pop.pose_at(0., false);
        assert_eq!(start.opacity, 0.);
        assert_eq!(start.translate_y, -4.);
        let mid = pop.pose_at(140., false);
        assert!(mid.opacity > 0.5 && mid.opacity < 1.);
        assert_eq!(pop.pose_at(280., false), Pose::REST);
        assert!(pop.running(279., false));
        assert!(!pop.running(280., false));
    }

    #[test]
    fn reduced_motion_keeps_delay_then_lands_on_the_final_keyframe() {
        let exit = Motion::UpdateNoticeRestartExit.resolve(&Shipping).unwrap();
        assert_eq!(exit.pose_at(2_999., true), Pose::REST);
        assert_eq!(exit.pose_at(3_000., true).opacity, 0.);
        assert!(!exit.running(0., true));
        assert_eq!(exit.total_ms(true), 3_000.);
        let pop = Motion::PopoverIn.resolve(&Shipping).unwrap();
        assert_eq!(pop.pose_at(0., true), Pose::REST);
        let options = Motion::CaptureMenuOptionsArrive.resolve(&Shipping).unwrap();
        assert_eq!(options.pose_at(20., false).opacity, 0., "fill-mode both");
        assert_eq!(options.total_ms(false), 240.);
    }

    #[test]
    fn lifecycle_enters_holds_and_exits_at_the_window_close() {
        let saved = Motion::RecordingSavedLifecycle.resolve(&Shipping).unwrap();
        assert_eq!(saved.lifecycle_pose(0., Some(15_000.), false).opacity, 0.);
        assert_eq!(saved.lifecycle_pose(375., Some(14_625.), false), Pose::REST);
        assert_eq!(
            saved.lifecycle_pose(5_000., Some(10_000.), false),
            Pose::REST
        );
        assert!(!saved.lifecycle_running(5_000., Some(10_000.), false));
        // An extended life (a save reset the expiry) holds, then exits at close.
        assert_eq!(saved.lifecycle_pose(20_000., None, false), Pose::REST);
        assert_eq!(
            saved.lifecycle_pose(20_000., Some(3_000.), false),
            Pose::REST
        );
        let fading = saved.lifecycle_pose(20_000., Some(1_000.), false);
        assert!(fading.opacity > 0. && fading.opacity < 1.);
        assert!(saved.lifecycle_running(20_000., Some(1_000.), false));
        assert_eq!(saved.lifecycle_pose(20_000., Some(0.), false).opacity, 0.);
        assert!(close(saved.lifecycle_exit_in(10_000.), 7_900.));
        // Reduced motion: steady and visible, never the invisible end frame.
        assert_eq!(saved.lifecycle_pose(0., Some(0.), true), Pose::REST);
        assert!(!saved.lifecycle_running(0., Some(15_000.), true));
        let hidden = Motion::RecordingControlsHiddenLifecycle
            .resolve(&Shipping)
            .unwrap();
        assert_eq!(
            hidden.lifecycle_pose(0., Some(6_000.), false).translate_y,
            7.
        );
        assert_eq!(hidden.lifecycle_pose(300., Some(5_700.), false), Pose::REST);
    }

    #[test]
    fn tween_eases_and_snaps_under_reduced_motion() {
        let tween = Transition::SegmentedIndicator.resolve(&Shipping).unwrap();
        assert_eq!(tween.duration_ms, 280.);
        assert_eq!(tween.value(10., 110., 0., false), 10.);
        assert_eq!(tween.value(10., 110., 280., false), 110.);
        assert_eq!(tween.value(10., 110., 0., true), 110.);
        let mid = tween.value(10., 110., 140., false);
        assert!(mid > 60. && mid < 110.);
        assert!(!tween.running(10., true));
    }

    #[test]
    fn missing_tokens_do_not_resolve() {
        struct Empty;
        impl MotionTokens for Empty {
            fn duration_ms(&self, _: &str) -> Option<f64> {
                None
            }
            fn easing(&self, _: &str) -> Option<[f64; 4]> {
                None
            }
        }
        assert!(Motion::UpdateNoticeIn.resolve(&Empty).is_none());
        assert!(Motion::PreviewCardArrive.resolve(&Empty).is_some());
    }
}
