//! The host's shared glass tooltip: shipping `.recording-tooltip >
//! [role="tooltip"]` (`styles/primitives.css`). A `--glass-strong` pill with
//! a `--glass-border` hairline, `--r-sm` corners, 5 × 8 px padding and
//! `--text-xs` medium copy, at most 180 px wide, centered under its anchor (or
//! right-aligned to it). It fades in while sliding 3 px down over `--dur-1`
//! `--ease-standard` and fades back out the same way
//! ([`Transition::Tooltip`]); under Windows' reduced motion it appears and
//! leaves at once.
//!
//! A surface keeps one [`Fade`] keyed by its own control type, calls
//! [`Fade::set`] with the control under the pointer, draws every entry of
//! [`Fade::frames`] with [`paint`], and keeps repainting while
//! [`Fade::animating`]. [`Fade::settled`] is the resting state for offscreen
//! renders. The recording controls (`hud.rs`) and the capture bar
//! (`capture.rs`) use it; mini preview cards (`previews.rs`) keep its fade
//! with their own timings ([`Fade::timed`]) and draw their own chip.

use captures_app::motion::{Transition, Tween};
use captures_app::recording_hud as hud;
use captures_app::tray_notice::LogicalRect;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::core::*;

use crate::gfx::{self, Gfx, rect};
use crate::tokens::Tokens;

type Rect = D2D_RECT_F;

/// Shipping's default `top: calc(100% + 8px)`; the recording controls'
/// actions use [`hud::TOOLTIP_GAP`] (6 px).
#[allow(dead_code)] // The capture bar is the first user; it lands next.
pub const GAP: f32 = 8.;

/// Milliseconds on one monotonic clock shared by every tooltip.
pub fn now_ms() -> f64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_secs_f64()
        * 1000.
}

/// `transition` (`Transition::Tooltip` unless [`Fade::timed`]) from the
/// tokens, or shipping's 90 ms standard curve.
fn tween(transition: Transition, t: &Tokens) -> Tween {
    transition.resolve(t).unwrap_or(Tween {
        duration_ms: 90.,
        easing: captures_app::motion::CubicBezier::LINEAR,
    })
}

/// Which anchor's tooltip shows, and the one fading out after the pointer
/// left it. Like a CSS transition, a change mid-fade continues from the
/// current opacity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fade<K> {
    shown: Option<K>,
    /// When `shown` last changed.
    at: f64,
    /// `shown`'s progress at `at` (a tooltip coming back mid-fade).
    from: f64,
    /// The previous tooltip and its progress at `at`.
    leaving: Option<(K, f64)>,
    /// How it fades: `Transition::Tooltip` unless [`Self::timed`].
    transition: Transition,
}

impl<K: Copy + PartialEq> Default for Fade<K> {
    fn default() -> Self {
        Self::settled(None)
    }
}

impl<K: Copy + PartialEq> Fade<K> {
    /// At rest on `key`: shown fully, nothing leaving.
    pub const fn settled(key: Option<K>) -> Self {
        Self {
            shown: key,
            at: f64::NEG_INFINITY,
            from: 1.,
            leaving: None,
            transition: Transition::Tooltip,
        }
    }

    /// Hidden, fading with `transition` instead (the mini previews' own
    /// tooltip timings).
    pub const fn timed(transition: Transition) -> Self {
        Self {
            transition,
            ..Self::settled(None)
        }
    }

    /// Show `key`'s tooltip (or none); true when that changed.
    pub fn set(&mut self, key: Option<K>, t: &Tokens, now: f64) -> bool {
        if key == self.shown {
            return false;
        }
        let (tween, reduced) = (
            tween(self.transition, t),
            crate::appearance::reduced_motion(),
        );
        let p = tween.progress(now - self.at, reduced);
        let shown = self
            .shown
            .map(|k| (k, self.from + (1. - self.from) * p))
            .filter(|(_, p)| *p > 0.);
        let leaving = self
            .leaving
            .map(|(k, from)| (k, from * (1. - p)))
            .filter(|(_, p)| *p > 0.);
        // A tooltip the pointer comes back to resumes where its fade-out is.
        self.from = match leaving {
            Some((k, p)) if Some(k) == key => p,
            _ => 0.,
        };
        self.leaving = shown;
        self.shown = key;
        self.at = now;
        true
    }

    /// Hide at once (the surface closed or hid).
    pub fn clear(&mut self) {
        *self = Self::settled(None);
    }

    /// The tooltips to draw now with their fade progress (0 hidden, 1 at
    /// rest), the leaving one first.
    pub fn frames(&self, t: &Tokens, now: f64) -> Vec<(K, f32)> {
        let p =
            tween(self.transition, t).progress(now - self.at, crate::appearance::reduced_motion());
        let mut frames = Vec::new();
        if let Some((key, from)) = self.leaving {
            frames.push((key, (from * (1. - p)) as f32));
        }
        if let Some(key) = self.shown {
            frames.push((key, (self.from + (1. - self.from) * p) as f32));
        }
        frames.retain(|(_, p)| *p > 0.);
        frames
    }

    /// True while a fade runs and the surface must keep painting.
    pub fn animating(&self, t: &Tokens, now: f64) -> bool {
        (self.shown.is_some() || self.leaving.is_some())
            && tween(self.transition, t).running(now - self.at, crate::appearance::reduced_motion())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    /// `left: 50%; translate(-50%)`.
    Center,
    /// The recording controls' last three: `right: 0`.
    Right,
}

/// Draw `label` as a tooltip `gap` DIPs under `anchor`, kept inside
/// `bounds`, at fade `progress` (opacity, and the 3 px slide still to go).
#[allow(clippy::too_many_arguments)]
pub fn paint(
    dc: &ID2D1DeviceContext,
    g: &Gfx,
    t: &Tokens,
    label: &str,
    anchor: Rect,
    align: Align,
    gap: f32,
    bounds: Rect,
    progress: f32,
) -> Result<()> {
    if progress <= 0. {
        return Ok(());
    }
    let pad = (hud::TOOLTIP_PADDING_X as f32, hud::TOOLTIP_PADDING_Y as f32);
    let text = g.text_sized(t, label, t.n("text-xs"), "weight-medium", None)?;
    let inner = hud::TOOLTIP_MAX_WIDTH as f32 - pad.0 * 2. - 2.;
    if text.width > inner {
        g.ellipsis(&text, inner)?;
    }
    let frame = frame(
        anchor,
        (text.width.min(inner), text.height),
        align,
        gap,
        bounds,
        progress,
    );
    let opacity = progress.clamp(0., 1.);
    let radius = t.n("r-sm");
    gfx::fill_round(dc, frame, radius, t.color_alpha("glass-strong", opacity))?;
    gfx::border(
        dc,
        frame,
        radius,
        1.,
        t.color_alpha("glass-border", opacity),
    )?;
    gfx::text(
        dc,
        &text,
        frame.left + 1. + pad.0,
        frame.top + 1. + pad.1,
        t.color_alpha("glass-text", opacity),
    )
}

/// The tooltip's box for a measured one-line label, through the shared
/// `recording_hud::tooltip_frame` geometry (its gap is the HUD's 6 px, so the
/// anchor grows by the difference).
fn frame(
    anchor: Rect,
    (width, height): (f32, f32),
    align: Align,
    gap: f32,
    bounds: Rect,
    progress: f32,
) -> Rect {
    let extra = f64::from(gap) - hud::TOOLTIP_GAP;
    let logical = |r: Rect, grow: f64| {
        LogicalRect::new(
            r.left.into(),
            r.top.into(),
            (r.right - r.left).into(),
            f64::from(r.bottom - r.top) + grow,
        )
    };
    let f = hud::tooltip_frame(
        logical(anchor, extra),
        width.into(),
        height.into(),
        align == Align::Right,
        progress.into(),
        logical(bounds, 0.),
    );
    rect(
        f.x as f32,
        f.y as f32,
        (f.x + f.width) as f32,
        (f.y + f.height) as f32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens() -> Tokens {
        Tokens::variant("dark", "mustard")
    }

    #[test]
    fn fade_in_out_and_resume_from_mid_flight() {
        if crate::appearance::reduced_motion() {
            return;
        }
        let t = tokens();
        let d = tween(Transition::Tooltip, &t).duration_ms;
        let mut fade = Fade::default();
        assert!(fade.frames(&t, 0.).is_empty());
        assert!(fade.set(Some(1), &t, 0.));
        assert!(
            !fade.set(Some(1), &t, 1.),
            "the same anchor changes nothing"
        );
        assert_eq!(fade.frames(&t, 0.), vec![]);
        let mid = fade.frames(&t, d / 2.);
        assert!(mid.len() == 1 && mid[0].1 > 0. && mid[0].1 < 1.);
        assert!(fade.animating(&t, d / 2.));
        assert_eq!(fade.frames(&t, d), vec![(1, 1.)]);
        assert!(!fade.animating(&t, d));
        // Moving to another button: the old one fades out as the new one in.
        fade.set(Some(2), &t, 1_000.);
        let both = fade.frames(&t, 1_000. + d / 2.);
        assert_eq!(both.iter().map(|f| f.0).collect::<Vec<_>>(), [1, 2]);
        assert!((both[0].1 + both[1].1 - 1.).abs() < 1e-5);
        // Leaving mid-fade and coming back resumes from the current opacity.
        fade.set(None, &t, 2_000.);
        let leaving = fade.frames(&t, 2_000. + d / 2.)[0].1;
        fade.set(Some(2), &t, 2_000. + d / 2.);
        let resumed = fade.frames(&t, 2_000. + d / 2.);
        assert_eq!(resumed.len(), 1);
        assert!((resumed[0].1 - leaving).abs() < 1e-5);
        assert_eq!(fade.frames(&t, 4_000.), vec![(2, 1.)]);
        fade.set(None, &t, 5_000.);
        assert!(fade.frames(&t, 5_000. + d).is_empty());
    }

    #[test]
    fn settled_draws_at_rest_and_frames_sit_under_their_anchor() {
        let t = tokens();
        assert_eq!(Fade::settled(Some(3)).frames(&t, 0.), vec![(3, 1.)]);
        assert!(!Fade::settled(Some(3)).animating(&t, 0.));
        let bounds = rect(0., 0., 400., 300.);
        let anchor = rect(100., 100., 140., 132.);
        let shown = frame(anchor, (60., 13.), Align::Center, GAP, bounds, 1.);
        // 8 px below, 1 px border and 8/5 px padding around the label.
        assert_eq!(
            (shown.left, shown.top, shown.right, shown.bottom),
            (120. - 39., 140., 120. + 39., 165.)
        );
        let hidden = frame(anchor, (60., 13.), Align::Center, GAP, bounds, 0.);
        assert_eq!(hidden.top, shown.top - 3.);
        let right = frame(anchor, (60., 13.), Align::Right, 6., bounds, 1.);
        assert_eq!((right.right, right.top), (140., 138.));
        // Near the bottom edge it stays inside the bounds.
        let low = frame(
            rect(100., 280., 140., 300.),
            (60., 13.),
            Align::Center,
            GAP,
            bounds,
            1.,
        );
        assert_eq!(low.bottom, 300.);
    }
}
