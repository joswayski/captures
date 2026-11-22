//! Tooltips on the recording editor: the `title` text shipping's
//! `RecordingEditor` gave its controls, drawn with the host's shared tooltip
//! (`crate::tooltip`, its fade honoring reduced motion) on one retained
//! surface above the page. Like a `title`, a tooltip waits the system hover
//! delay before it appears (comctl32's `TTDT_INITIAL`, the double-click
//! time), moves at once between anchors while one shows, and hides when the
//! pointer leaves, presses or a key goes down.
use super::*;
use crate::tooltip::{self, Align, Fade};

pub(super) const TIP_TIMER: usize = 44;

/// Room a tooltip needs beside its anchor: the gap, its 3 px slide, a box
/// of up to four wrapped lines (a long destination path) and spare.
const ROOM: f32 = tooltip::GAP + 3. + 80.;
/// Half the widest tooltip (`TOOLTIP_MAX_WIDTH`) plus spare.
const HALF: f32 = 100.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tip {
    Play,
    TrimStart,
    TrimEnd,
    /// "Est. size" under Maximum file size.
    Estimate,
    /// The footer's "Saving to" folder.
    Destination,
    Copy,
    Save,
}

#[derive(Default)]
pub(super) struct Tips {
    pub fade: Fade<Tip>,
    /// The tooltip showing (or fading in), as `fade` was last set.
    shown: Option<Tip>,
    /// Waiting out the hover delay.
    pending: Option<Tip>,
    /// Pressed or typed over: stays hidden until the pointer leaves it.
    suppressed: Option<Tip>,
}

impl Recording {
    /// Shipping's `title` copy.
    pub fn tip_label(&self, tip: Tip) -> String {
        match tip {
            Tip::Play if self.preview.playing => "Pause (Space)".into(),
            Tip::Play => "Play (Space)".into(),
            Tip::TrimStart => "Trim start · I sets it at the playhead".into(),
            Tip::TrimEnd => "Trim end · O sets it at the playhead".into(),
            Tip::Estimate => "Estimated saved file size for the current edits and settings".into(),
            Tip::Destination => self.directory_label(),
            Tip::Copy if self.format_requires_copy() => {
                "Changing formats always creates a new file".into()
            }
            Tip::Copy => "Save as a new file and leave the original untouched".into(),
            Tip::Save => "Save (Command/Ctrl+S)".into(),
        }
    }

    /// The save folder as people write it: the session's canonical source
    /// folder drops its `\\?\` verbatim prefix.
    pub(super) fn directory_label(&self) -> String {
        let path = self.directory.display().to_string();
        if let Some(share) = path.strip_prefix(r"\\?\UNC\") {
            format!(r"\\{share}")
        } else {
            path.strip_prefix(r"\\?\").unwrap_or(&path).to_owned()
        }
    }

    /// The element a tooltip belongs to, in window DIPs.
    fn tip_anchor(&self, tip: Tip) -> Option<Rect> {
        let target = match tip {
            Tip::Play => Target::Play,
            Tip::TrimStart => Target::TrimStart,
            Tip::TrimEnd => Target::TrimEnd,
            Tip::Copy => Target::Copy,
            Tip::Save => Target::Save,
            Tip::Estimate => {
                if !self.shown(Target::MaximumSize) {
                    return None;
                }
                // The `<strong>` value line, as drawn by `draw_output`.
                let r = self.maximum_estimate_rect();
                let line = Rect::new(r.x, r.y + 24., r.w, 20.);
                return Some(self.page_visible_rect(line)).filter(|r| r.h > 0.);
            }
            Tip::Destination => {
                // The `<output>` as `draw` lays it out in the footer.
                let (footer, field) = (self.rects[FOOTER], self.target_rect(Target::Filename));
                let change = self.target_rect(Target::Change);
                return Some(Rect::new(
                    field.x + 65.,
                    footer.y + self.t.n("s-4"),
                    (change.x - field.x - 69.).max(1.),
                    16.,
                ));
            }
        };
        if !self.shown(target) {
            return None;
        }
        let r = self.visible_target_rect(target);
        (r.w > 0. && r.h > 0.).then_some(r)
    }

    /// The tooltip whose anchor is under a window point.
    pub(super) fn tip_at(&self, x: f32, y: f32) -> Option<Tip> {
        [
            Tip::Play,
            Tip::TrimStart,
            Tip::TrimEnd,
            Tip::Estimate,
            Tip::Destination,
            Tip::Copy,
            Tip::Save,
        ]
        .into_iter()
        .find(|&tip| self.tip_anchor(tip).is_some_and(|r| r.contains(x, y)))
    }

    /// The pointer moved onto `tip` (or off every anchor).
    pub(super) fn hover_tip(&mut self, tip: Option<Tip>) {
        if tip != self.tips.suppressed {
            self.tips.suppressed = None;
        }
        let tip = tip.filter(|&t| Some(t) != self.tips.suppressed);
        if tip == self.tips.shown.or(self.tips.pending) {
            return;
        }
        self.tips.pending = None;
        if !self.hwnd.is_invalid() {
            unsafe {
                let _ = KillTimer(Some(self.hwnd), TIP_TIMER);
            }
        }
        match tip {
            // Moving between anchors while one shows switches at once.
            Some(_) if self.tips.shown.is_some() => self.show_tip(tip),
            Some(_) => {
                self.tips.pending = tip;
                if !self.hwnd.is_invalid() {
                    unsafe {
                        SetTimer(Some(self.hwnd), TIP_TIMER, GetDoubleClickTime(), None);
                    }
                }
            }
            None => self.show_tip(None),
        }
    }

    /// Show a tooltip at rest for an offscreen render (`--select tip-NAME`).
    pub(super) fn settle_tip(&mut self, name: &str) {
        let tip = match name {
            "play" => Tip::Play,
            "trim-start" => Tip::TrimStart,
            "trim-end" => Tip::TrimEnd,
            "estimate" => Tip::Estimate,
            "destination" => Tip::Destination,
            "copy" => Tip::Copy,
            "save" => Tip::Save,
            _ => return,
        };
        self.tips.shown = Some(tip);
        self.tips.fade = Fade::settled(Some(tip));
        self.place_tips();
    }

    /// The hover delay ran out.
    pub(super) fn tip_due(&mut self) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIP_TIMER);
        }
        if let Some(tip) = self.tips.pending.take() {
            self.show_tip(Some(tip));
        }
    }

    fn show_tip(&mut self, tip: Option<Tip>) {
        self.tips.shown = tip;
        if self.tips.fade.set(tip, &self.t, tooltip::now_ms()) {
            self.place_tips();
        }
    }

    /// Hide without waiting (a press or a key, like a `title` tooltip).
    pub(super) fn dismiss_tip(&mut self) {
        self.tips.suppressed = self.tips.shown.or(self.tips.pending);
        self.tips.pending = None;
        if self.tips.shown.is_some() {
            self.show_tip(None);
        }
    }

    /// Where a tooltip may sit: under its anchor, or above it when the
    /// window has no room below (the footer's Save).
    fn tip_bounds(&self, anchor: Rect) -> Rect {
        // Full width, slid inside the window near an edge, so `paint` moves
        // the box over rather than clipping it (the trim handles).
        let left = (anchor.x + anchor.w / 2. - HALF).clamp(0., (self.width - HALF * 2.).max(0.));
        let right = (left + HALF * 2.).min(self.width);
        let below = anchor.y + anchor.h;
        let (top, bottom) = if below + ROOM <= self.height {
            (below, below + ROOM)
        } else {
            // `tooltip::paint` keeps the box inside its bounds, so bounds
            // ending a gap above the anchor place it there.
            ((anchor.y - ROOM).max(0.), anchor.y - tooltip::GAP)
        };
        Rect::new(left, top, (right - left).max(0.), (bottom - top).max(0.))
    }

    /// Size the tooltip surface to the tooltips it draws.
    pub(super) fn place_tips(&mut self) {
        let mut area: Option<Rect> = None;
        for (tip, _) in self.tips.fade.frames(&self.t, tooltip::now_ms()) {
            let Some(anchor) = self.tip_anchor(tip) else {
                continue;
            };
            let b = self.tip_bounds(anchor);
            area = Some(match area {
                None => b,
                Some(a) => {
                    let (x, y) = (a.x.min(b.x), a.y.min(b.y));
                    let r = (a.x + a.w).max(b.x + b.w);
                    let bottom = (a.y + a.h).max(b.y + b.h);
                    Rect::new(x, y, r - x, bottom - y)
                }
            });
        }
        if let Some(area) = area {
            self.rects[TOOLTIP] = area;
        }
        self.dirty[TOOLTIP] = true;
    }

    pub(super) fn tips_visible(&self) -> bool {
        !self.tips.fade.frames(&self.t, tooltip::now_ms()).is_empty()
    }

    /// Keep painting while a tooltip fades.
    pub(super) fn tips_animating(&self) -> bool {
        self.tips.fade.animating(&self.t, tooltip::now_ms())
    }

    /// Draw the tooltips in the surface's coordinates.
    pub(super) fn draw_tips(&self, dc: &ID2D1DeviceContext) -> Result<()> {
        let origin = self.rects[TOOLTIP];
        let local = |r: Rect| {
            gfx::rect(
                r.x - origin.x,
                r.y - origin.y,
                r.x - origin.x + r.w,
                r.y - origin.y + r.h,
            )
        };
        for (tip, progress) in self.tips.fade.frames(&self.t, tooltip::now_ms()) {
            let Some(anchor) = self.tip_anchor(tip) else {
                continue;
            };
            tooltip::paint(
                dc,
                &self.g,
                &self.t,
                &self.tip_label(tip),
                local(anchor),
                Align::Center,
                tooltip::GAP,
                local(self.tip_bounds(anchor)),
                progress,
            )?;
        }
        Ok(())
    }
}
