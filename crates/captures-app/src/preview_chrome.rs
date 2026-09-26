//! Mini-preview card chrome both native hosts share: editor presence, the
//! stale-pointer hover lock, the hover media treatment and icon tooltips.
//!
//! Ported from the shipping `ThumbnailCard` (`apps/desktop/ui/src/App.tsx`),
//! `lib/editorPresence.ts`, `lib/thumbnailHover.ts` and
//! `styles/mini-preview.css`. Hosts own drawing and timers; this module owns
//! the rules, copy and geometry so AppKit and wgpu cannot drift.

use crate::tray_notice::LogicalRect;

/// Compact editor control copy (`aria-label` and tooltip).
pub const EDIT_LABEL: &str = "Edit";
/// Resting label of the present pill (`.label-rest`).
pub const EDITOR_PRESENT_LABEL: &str = "In editor";
/// Hover/focus label and accessible name of the present pill (`.label-hover`).
pub const EDITOR_SHOW_LABEL: &str = "Show in editor";

/// `EDITOR_PRESENCE_LEAVE_MS`: after the editor closes, the pill shrinks and
/// the ring eases out for this long (`.thumbnail-editor-leaving`).
pub const EDITOR_PRESENCE_LEAVE_MS: f64 = 550.0;
/// `EDITOR_PRESENCE_LINGER_MS`: then the plain Edit icon stays visible this
/// long so an accidental close can be undone without re-hovering.
pub const EDITOR_PRESENCE_LINGER_MS: f64 = 3_000.0;

/// `.thumbnail-card:hover img`: `blur(2px) brightness(.5)` and `scale(1.015)`.
pub const HOVER_MEDIA_BLUR: f64 = 2.0;
pub const HOVER_MEDIA_BRIGHTNESS: f64 = 0.5;
pub const HOVER_MEDIA_SCALE: f64 = 1.015;

/// `THUMBNAIL_CARD_HOVER_LOCK_SLOP_PX`: a stationary pointer cannot unlock hover.
pub const CARD_HOVER_LOCK_SLOP: f64 = 4.0;

/// `.icon-button::after` / `.thumbnail-stack-control[data-tooltip]::after`.
pub const ICON_TOOLTIP_GAP: f64 = 6.0;
pub const ICON_TOOLTIP_PADDING_X: f64 = 7.0;
pub const ICON_TOOLTIP_PADDING_Y: f64 = 4.0;
/// `--thumbnail-tooltip-nudge`: the tip slides this far toward its icon's
/// opposite side while it fades in.
pub const ICON_TOOLTIP_NUDGE: f64 = 2.0;

/// Present pill metrics (`.thumbnail-editor-control.is-present`): padding
/// `3px 9px 3px 7px`, a 5 px gap, an 11 px icon and a 1 px border.
pub const EDITOR_PILL_PADDING_LEFT: f64 = 7.0;
pub const EDITOR_PILL_PADDING_RIGHT: f64 = 9.0;
pub const EDITOR_PILL_GAP: f64 = 5.0;
pub const EDITOR_PILL_ICON: f64 = 11.0;
/// `max-width: 11rem`.
pub const EDITOR_PILL_MAX_WIDTH: f64 = 176.0;
/// The compact control and the pill share this height (`28px`).
pub const EDITOR_CONTROL_SIZE: f64 = 28.0;

/// Where a card's editor control stands in its presence lifecycle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EditorPhase {
    /// No editor: the compact Edit icon appears only with hover chrome.
    #[default]
    Idle,
    /// An editor window shows this capture: "In editor" pill and accent ring.
    Present,
    /// The editor just closed: the pill shrinks back and the ring eases out.
    /// The control ignores the pointer (`.thumbnail-editor-control.leaving`).
    Leaving,
    /// After the leave, the plain Edit icon stays visible without hover.
    Lingering,
}

impl EditorPhase {
    /// `.is-present`: the pill shape, accent label and the card ring.
    pub fn present(self) -> bool {
        self == Self::Present
    }

    /// The control shows without hover chrome (`thumbnail-editor-active`,
    /// `-leaving` and `-lingering` all pin it visible).
    pub fn pinned(self) -> bool {
        self != Self::Idle
    }

    /// The control accepts clicks (not while leaving).
    pub fn interactive(self) -> bool {
        self != Self::Leaving
    }

    /// Accessible name: "Show in editor" while present, otherwise "Edit".
    pub fn accessible_label(self) -> &'static str {
        if self.present() {
            EDITOR_SHOW_LABEL
        } else {
            EDIT_LABEL
        }
    }

    /// The compact icon's tooltip; the pill and its leave carry none.
    pub fn tooltip(self) -> Option<&'static str> {
        matches!(self, Self::Idle | Self::Lingering).then_some(EDIT_LABEL)
    }
}

/// Pill label for a present control. Hover or keyboard focus shows the
/// action ("Show in editor") unless the control was just clicked to open the
/// editor and the pointer has not left it yet (`data-editor-just-opened`).
pub fn editor_pill_label(hovered_or_focused: bool, just_opened: bool) -> &'static str {
    if hovered_or_focused && !just_opened {
        EDITOR_SHOW_LABEL
    } else {
        EDITOR_PRESENT_LABEL
    }
}

/// Width of the present pill. Both labels share one grid cell in shipping, so
/// the pill is as wide as the wider label and never jumps on hover.
pub fn editor_pill_width(rest_label_width: f64, hover_label_width: f64) -> f64 {
    (EDITOR_PILL_PADDING_LEFT
        + EDITOR_PILL_ICON
        + EDITOR_PILL_GAP
        + rest_label_width.max(hover_label_width).max(0.0)
        + EDITOR_PILL_PADDING_RIGHT
        + 2.0)
        .clamp(EDITOR_CONTROL_SIZE, EDITOR_PILL_MAX_WIDTH)
}

/// Per-card editor presence (shipping `editorPresence` state in
/// `ThumbnailCard`). Hosts report whether an editor window currently shows the
/// capture and advance time; times are host milliseconds on any monotonic
/// clock.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EditorPresence {
    active: bool,
    phase: EditorPhase,
    /// When the current phase began.
    since_ms: f64,
}

impl EditorPresence {
    /// A card that appears while its editor is already open starts present.
    pub fn new(active: bool, now_ms: f64) -> Self {
        Self {
            active,
            phase: if active {
                EditorPhase::Present
            } else {
                EditorPhase::Idle
            },
            since_ms: now_ms,
        }
    }

    /// Rebuild a presence that crossed a C ABI as plain fields.
    pub fn from_parts(active: bool, phase: EditorPhase, since_ms: f64) -> Self {
        Self {
            active,
            phase,
            since_ms,
        }
    }

    pub fn active(&self) -> bool {
        self.active
    }

    /// When the current phase began, in host milliseconds.
    pub fn since_ms(&self) -> f64 {
        self.since_ms
    }

    pub fn phase(&self) -> EditorPhase {
        self.phase
    }

    /// Milliseconds since the current phase began, for host transitions.
    pub fn phase_elapsed_ms(&self, now_ms: f64) -> f64 {
        (now_ms - self.since_ms).max(0.0)
    }

    /// Report the host's editor state. Returns whether the phase changed.
    pub fn set_active(&mut self, active: bool, now_ms: f64, reduced_motion: bool) -> bool {
        if active == self.active {
            return self.advance(now_ms, reduced_motion);
        }
        self.active = active;
        let before = self.phase;
        if active {
            self.enter(EditorPhase::Present, now_ms);
        } else if self.phase == EditorPhase::Present {
            self.enter(EditorPhase::Leaving, now_ms);
            self.advance(now_ms, reduced_motion);
        }
        before != self.phase
    }

    /// Move through Leaving → Lingering → Idle as their times elapse. Reduced
    /// motion skips the leave ease (shipping uses a zero leave timer) but keeps
    /// the linger, which is a recovery affordance rather than motion.
    pub fn advance(&mut self, now_ms: f64, reduced_motion: bool) -> bool {
        let before = self.phase;
        loop {
            let elapsed = self.phase_elapsed_ms(now_ms);
            match self.phase {
                EditorPhase::Leaving if elapsed >= leave_ms(reduced_motion) => {
                    let at = self.since_ms + leave_ms(reduced_motion);
                    self.enter(EditorPhase::Lingering, at);
                }
                EditorPhase::Lingering if elapsed >= EDITOR_PRESENCE_LINGER_MS => {
                    let at = self.since_ms + EDITOR_PRESENCE_LINGER_MS;
                    self.enter(EditorPhase::Idle, at);
                }
                _ => break,
            }
        }
        before != self.phase
    }

    /// Milliseconds until [`advance`](Self::advance) would change the phase,
    /// so hosts schedule one wake instead of repainting continuously.
    pub fn next_change_in_ms(&self, now_ms: f64, reduced_motion: bool) -> Option<f64> {
        let elapsed = self.phase_elapsed_ms(now_ms);
        match self.phase {
            EditorPhase::Leaving => Some((leave_ms(reduced_motion) - elapsed).max(0.0)),
            EditorPhase::Lingering => Some((EDITOR_PRESENCE_LINGER_MS - elapsed).max(0.0)),
            EditorPhase::Idle | EditorPhase::Present => None,
        }
    }

    fn enter(&mut self, phase: EditorPhase, at_ms: f64) {
        self.phase = phase;
        self.since_ms = at_ms;
    }
}

fn leave_ms(reduced_motion: bool) -> f64 {
    if reduced_motion {
        0.0
    } else {
        EDITOR_PRESENCE_LEAVE_MS
    }
}

/// Shipping card-hover lock (`data-thumbnail-suppress-card-hover`). Expanding
/// the pile, or a new capture joining the stack, can leave the pointer resting
/// on a card that was never hovered as a live preview. Hover chrome and the
/// media blur stay idle until the pointer moves [`CARD_HOVER_LOCK_SLOP`] from
/// where it was first seen, or leaves. Keyboard focus still reveals chrome.
///
/// Native hosts see only real pointer samples, so both shipping lock kinds
/// (motion and appear) use the origin-and-slop rule of its native poll path.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CardHoverLock {
    locked: bool,
    origin: Option<(f64, f64)>,
}

impl CardHoverLock {
    /// Hold hover off until the pointer actually moves.
    pub fn lock(&mut self) {
        self.locked = true;
        self.origin = None;
    }

    pub fn unlock(&mut self) {
        self.locked = false;
        self.origin = None;
    }

    pub fn locked(&self) -> bool {
        self.locked
    }

    /// Rebuild a lock that crossed a C ABI as plain fields.
    pub fn from_parts(locked: bool, origin: Option<(f64, f64)>) -> Self {
        Self {
            locked,
            origin: origin.filter(|_| locked),
        }
    }

    /// The first pointer sample seen while locked.
    pub fn origin(&self) -> Option<(f64, f64)> {
        self.origin
    }

    /// Feed a pointer sample in window coordinates (`None` when the pointer
    /// is outside the stack window). Returns whether hover stays locked.
    pub fn pointer(&mut self, position: Option<(f64, f64)>) -> bool {
        if !self.locked {
            return false;
        }
        let Some((x, y)) = position.filter(|(x, y)| x.is_finite() && y.is_finite()) else {
            self.unlock();
            return false;
        };
        match self.origin {
            None => self.origin = Some((x, y)),
            Some((ox, oy)) if (x - ox).hypot(y - oy) >= CARD_HOVER_LOCK_SLOP => self.unlock(),
            Some(_) => {}
        }
        self.locked
    }

    /// The window moved or resized under a stationary pointer (a new card
    /// grew the stack): window-relative samples jump without real movement,
    /// so sample the origin again rather than unlocking.
    pub fn resample_origin(&mut self) {
        if self.locked {
            self.origin = None;
        }
    }
}

/// Which side of its icon a tooltip opens on. Card corner icons and the stack
/// toolbar open above in bottom-anchored stacks (there is room over each
/// card) and below in top-anchored stacks, where the gutter above the first
/// card holds the toolbar.
pub fn icon_tooltip_above(top_anchor: bool) -> bool {
    !top_anchor
}

/// Frame of an icon tooltip in y-down coordinates: centered on `anchor`,
/// [`ICON_TOOLTIP_GAP`] away, sliding [`ICON_TOOLTIP_NUDGE`] toward the icon's
/// far side while `progress` runs 0→1.
pub fn icon_tooltip_frame(
    anchor: LogicalRect,
    text_width: f64,
    text_height: f64,
    above: bool,
    progress: f64,
) -> LogicalRect {
    let width = text_width.max(0.0) + ICON_TOOLTIP_PADDING_X * 2.0;
    let height = text_height.max(0.0) + ICON_TOOLTIP_PADDING_Y * 2.0;
    let x = anchor.x + (anchor.width - width) / 2.0;
    let nudge = ICON_TOOLTIP_NUDGE * (1.0 - progress.clamp(0.0, 1.0));
    let y = if above {
        anchor.y - ICON_TOOLTIP_GAP - height + nudge
    } else {
        anchor.y + anchor.height + ICON_TOOLTIP_GAP - nudge
    };
    LogicalRect::new(x, y, width, height)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presence_matches_shipping_leave_and_linger() {
        let mut presence = EditorPresence::new(false, 0.0);
        assert_eq!(presence.phase(), EditorPhase::Idle);
        assert!(presence.set_active(true, 100.0, false));
        assert_eq!(presence.phase(), EditorPhase::Present);
        assert!(presence.phase().present() && presence.phase().pinned());
        assert_eq!(presence.phase().accessible_label(), "Show in editor");
        assert_eq!(presence.phase().tooltip(), None);
        assert_eq!(presence.next_change_in_ms(200.0, false), None);

        assert!(presence.set_active(false, 1_000.0, false));
        assert_eq!(presence.phase(), EditorPhase::Leaving);
        assert!(!presence.phase().interactive() && !presence.phase().present());
        assert_eq!(presence.next_change_in_ms(1_200.0, false), Some(350.0));
        assert!(!presence.advance(1_549.0, false));
        assert!(presence.advance(1_550.0, false));
        assert_eq!(presence.phase(), EditorPhase::Lingering);
        assert_eq!(presence.phase().tooltip(), Some("Edit"));
        assert_eq!(presence.phase().accessible_label(), "Edit");
        assert!(presence.phase().pinned());
        assert_eq!(presence.next_change_in_ms(1_550.0, false), Some(3_000.0));
        // A late tick still lands on the right phase and anchors to the
        // scheduled time, not the tick.
        assert!(presence.advance(10_000.0, false));
        assert_eq!(presence.phase(), EditorPhase::Idle);
        assert!(!presence.phase().pinned());
    }

    #[test]
    fn reopening_during_leave_or_linger_returns_to_present() {
        for reopen_at in [1_100.0, 2_000.0] {
            let mut presence = EditorPresence::new(true, 0.0);
            presence.set_active(false, 1_000.0, false);
            presence.set_active(true, reopen_at, false);
            assert_eq!(presence.phase(), EditorPhase::Present);
        }
    }

    #[test]
    fn reduced_motion_skips_the_leave_ease_but_keeps_the_linger() {
        let mut presence = EditorPresence::new(true, 0.0);
        presence.set_active(false, 500.0, true);
        assert_eq!(presence.phase(), EditorPhase::Lingering);
        assert_eq!(presence.next_change_in_ms(500.0, true), Some(3_000.0));
    }

    #[test]
    fn closing_an_idle_card_does_not_play_a_leave() {
        let mut presence = EditorPresence::new(false, 0.0);
        assert!(!presence.set_active(false, 10.0, false));
        assert_eq!(presence.phase(), EditorPhase::Idle);
    }

    #[test]
    fn pill_label_holds_in_editor_until_the_pointer_leaves_after_opening() {
        assert_eq!(editor_pill_label(false, false), "In editor");
        assert_eq!(editor_pill_label(true, false), "Show in editor");
        assert_eq!(editor_pill_label(true, true), "In editor");
        assert_eq!(
            editor_pill_width(40.0, 70.0),
            7.0 + 11.0 + 5.0 + 70.0 + 9.0 + 2.0
        );
        assert_eq!(editor_pill_width(0.0, 500.0), EDITOR_PILL_MAX_WIDTH);
    }

    #[test]
    fn hover_lock_waits_for_real_movement_or_leave() {
        let mut lock = CardHoverLock::default();
        assert!(!lock.pointer(Some((10.0, 10.0))));
        lock.lock();
        assert!(
            lock.pointer(Some((10.0, 10.0))),
            "first sample sets the origin"
        );
        assert!(
            lock.pointer(Some((12.0, 12.0))),
            "sub-slop jitter stays locked"
        );
        assert!(!lock.pointer(Some((10.0, 14.0))), "4 pt of travel unlocks");
        lock.lock();
        assert!(lock.pointer(Some((50.0, 50.0))));
        assert!(!lock.pointer(None), "leaving the window unlocks");
        lock.lock();
        lock.pointer(Some((50.0, 50.0)));
        lock.resample_origin();
        assert!(
            lock.pointer(Some((50.0, 234.0))),
            "a window jump re-samples"
        );
        assert!(!lock.pointer(Some((50.0, 240.0))));
    }

    #[test]
    fn icon_tooltips_center_and_open_away_from_the_stack_anchor() {
        assert!(icon_tooltip_above(false));
        assert!(!icon_tooltip_above(true));
        let icon = LogicalRect::new(8.0, 8.0, 28.0, 28.0);
        let above = icon_tooltip_frame(icon, 30.0, 12.0, true, 1.0);
        assert_eq!(above, LogicalRect::new(0.0, -18.0, 44.0, 20.0));
        let entering = icon_tooltip_frame(icon, 30.0, 12.0, true, 0.0);
        assert_eq!(entering.y, above.y + ICON_TOOLTIP_NUDGE);
        let below = icon_tooltip_frame(icon, 30.0, 12.0, false, 1.0);
        assert_eq!(below.y, 42.0);
        assert_eq!(
            icon_tooltip_frame(icon, 30.0, 12.0, false, 0.0).y,
            42.0 - ICON_TOOLTIP_NUDGE
        );
    }
}
