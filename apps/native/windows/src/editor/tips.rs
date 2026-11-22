//! Tooltips on the screenshot editor: shipping `ScreenshotEditor`'s `title`
//! copy, drawn with the host's shared glass tooltip (`crate::tooltip::paint`)
//! on one node above the window, and the rail's own hover label
//! (`.screenshot-tool-rail span`). Like a `title`, a tooltip waits the system
//! hover delay before it appears (the double-click time, comctl32's
//! `TTDT_INITIAL`), moves at once between anchors while one shows, and hides
//! when the pointer leaves, presses or a key goes down. The rail label shows
//! at once on hover or keyboard focus, as its CSS does. Fades run on the
//! compositor over `--dur-1` and are instant under reduced motion; the
//! tooltip's 3 px slide is not drawn.
use captures_app::editor_chrome::{RAIL_TOOLS, SHAPES_LABEL};
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime;
use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};
use windows::core::*;

use super::{Editor, Target, shapes_index};
use crate::gfx::{self, Style, rect};
use crate::scene::Rect;
use crate::tooltip::{self, Align};

pub(super) const TIP_TIMER: usize = 5;

/// Room a tooltip needs beside its anchor: the gap, a box of up to four
/// wrapped lines (Maximum file size's help) and spare.
const ROOM: f32 = tooltip::GAP + 80.;
/// Half the widest tooltip (`TOOLTIP_MAX_WIDTH`) plus spare.
const HALF: f32 = 100.;
/// `.screenshot-tool-rail span { left: calc(100% + 10px) }`.
const RAIL_GAP: f32 = 10.;
/// Room for the rail label's `shadow-md`.
pub(super) const TIP_BLEED: f32 = 24.;

/// What a tooltip belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tip {
    /// A control's `title`.
    Control(Target),
    /// A Layers row's drag handle (`Drag to reorder` / `Layer is locked`).
    Grip(usize),
    /// An image row's name (`Double-click to rename`).
    Rename(usize),
    /// `.screenshot-output-estimate` in Export settings.
    Estimate,
    /// The save row's `output aria-label="Save location"`.
    Location,
    /// `.screenshot-tool-rail span`: a rail tool's label to its right.
    Rail(usize),
}

#[derive(Default)]
pub(super) struct Tips {
    /// The tooltip showing (or fading in).
    pub(super) shown: Option<Tip>,
    /// The one drawn: `shown`, or the one fading out.
    drawn: Option<Tip>,
    /// Waiting out the hover delay.
    pending: Option<Tip>,
    /// Pressed or typed over: stays hidden until the pointer leaves it.
    suppressed: Option<Tip>,
}

impl Editor {
    /// The tooltip's copy, `None` where shipping has no `title`.
    pub fn tip_text(&self, tip: Tip) -> Option<String> {
        let text = match tip {
            // Shipping's Change… and Export settings carry no `title`.
            Tip::Control(Target::Change | Target::ExportSettings | Target::Tool(_)) => {
                String::new()
            }
            Tip::Control(target) => self.help(target),
            Tip::Grip(i) if self.layer(i)?.locked => {
                captures_app::editor_chrome::layers::LOCKED.into()
            }
            Tip::Grip(_) => captures_app::editor_chrome::layers::DRAG.into(),
            Tip::Rename(_) => captures_app::editor_chrome::layers::RENAME.into(),
            Tip::Estimate => {
                "Estimated export file size for the current format, quality, and output size".into()
            }
            Tip::Location => self.save_location(),
            Tip::Rail(i) if i == shapes_index() => SHAPES_LABEL.into(),
            Tip::Rail(i) => RAIL_TOOLS[i].label.into(),
        };
        (!text.is_empty()).then_some(text)
    }

    /// The element a tooltip belongs to, in client DIPs.
    fn tip_anchor(&self, tip: Tip) -> Option<Rect> {
        let target = match tip {
            Tip::Control(target) => target,
            Tip::Rail(i) => Target::Tool(i),
            Tip::Grip(i) | Tip::Rename(i) => {
                let row = self.target_rect(Target::Layer(i));
                let list = self.l.list;
                let visible = row.y + row.h > list.y && row.y < list.y + list.h;
                if !self.exists(Target::Layer(i)) || !visible {
                    return None;
                }
                let s3 = self.t.n("s-3");
                let s4 = self.t.n("s-4");
                return Some(match tip {
                    Tip::Grip(_) => Rect::new(row.x + s3 - 2., row.y, 18., row.h),
                    _ => {
                        let x = row.x + 1. + s3 + 12. + s4 + 44. + s4;
                        let right = row.x + row.w - s3 - 3. * super::QUICK.0 - 4. - s3;
                        Rect::new(x, row.y + 8., (right - x).max(1.), row.h / 2. - 8.)
                    }
                });
            }
            Tip::Estimate => return self.estimate_rect(),
            Tip::Location => return self.location_rect(),
        };
        if !self.exists(target) {
            return None;
        }
        let r = self.target_rect(target);
        (r.w > 0. && r.h > 0.).then_some(r)
    }

    /// The save row's folder path, as `draw_bar` lays it out.
    fn location_rect(&self) -> Option<Rect> {
        self.presented.as_ref()?;
        let (t, g, l) = (&self.t, &self.g, &self.l);
        let (s3, s5) = (t.n("s-3"), t.n("s-5"));
        let label = g.text(t, Style::Status, "Filename", 1e4).ok()?;
        let path = g
            .text(t, Style::MonoTiny, &self.save_location(), 1e4)
            .ok()?;
        let saving = g.text(t, Style::Tiny, "Saving to", 1e4).ok()?;
        let room = (l.change.x - s3 - (l.folder.x + label.width + s5) - saving.width - 4.).max(20.);
        let w = path.width.min(room);
        Some(Rect::new(
            l.change.x - s3 - w,
            l.folder.y + 3.,
            w,
            path.height,
        ))
    }

    /// The tooltip under a client point, from the hovered control.
    pub(super) fn tip_at(&self, x: f32, y: f32) -> Option<Tip> {
        let tip = match self.hover {
            // The label hides while the Shapes flyout is open.
            Some(Target::Tool(i)) if !(self.flyout && i == shapes_index()) => Tip::Rail(i),
            Some(Target::Tool(_)) => return None,
            Some(Target::Layer(i)) => {
                let tip = [Tip::Grip(i), Tip::Rename(i)]
                    .into_iter()
                    .find(|&t| self.tip_anchor(t).is_some_and(|r| r.contains(x, y)))?;
                if tip == Tip::Rename(i) && (!self.layer(i)?.is_image || self.renaming() == Some(i))
                {
                    return None;
                }
                tip
            }
            Some(target) => Tip::Control(target),
            None => [Tip::Estimate, Tip::Location]
                .into_iter()
                .find(|&t| self.tip_anchor(t).is_some_and(|r| r.contains(x, y)))?,
        };
        self.tip_text(tip).is_some().then_some(tip)
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
        // Keyboard focus keeps its rail label when the pointer leaves.
        let focused = self.focused_rail();
        if tip.is_none() && focused.is_some() && self.tips.shown == focused {
            return;
        }
        self.tips.pending = None;
        self.kill_tip_timer();
        match tip {
            // The rail label shows at once; moving between anchors while
            // one shows switches at once.
            Some(Tip::Rail(_)) => self.show_tip(tip),
            Some(_) if self.tips.shown.is_some() => self.show_tip(tip),
            Some(_) => {
                self.tips.pending = tip;
                if !self.hwnd.is_invalid() {
                    unsafe {
                        SetTimer(Some(self.hwnd), TIP_TIMER, GetDoubleClickTime(), None);
                    }
                }
            }
            None => self.show_tip(focused),
        }
    }

    /// `button:focus-visible span`: the focused rail tool's label.
    fn focused_rail(&self) -> Option<Tip> {
        match self.focus {
            Some(Target::Tool(i)) if self.focus_visible && !self.flyout => Some(Tip::Rail(i)),
            _ => None,
        }
    }

    /// Focus moved: show or hide the rail label it brings.
    pub(super) fn focus_tip(&mut self) {
        let focused = self.focused_rail();
        let pointer = self.pointer.and_then(|(x, y)| self.tip_at(x, y));
        if focused.is_some() && pointer.is_none() {
            self.show_tip(focused);
        } else if matches!(self.tips.shown, Some(Tip::Rail(_))) && pointer.is_none() {
            self.show_tip(None);
        }
    }

    fn kill_tip_timer(&self) {
        if !self.hwnd.is_invalid() {
            unsafe {
                let _ = KillTimer(Some(self.hwnd), TIP_TIMER);
            }
        }
    }

    /// The hover delay ran out.
    pub(super) fn tip_due(&mut self) {
        self.kill_tip_timer();
        if let Some(tip) = self.tips.pending.take() {
            self.show_tip(Some(tip));
        }
    }

    /// Hide without waiting (a press or a key, like a `title` tooltip).
    pub(super) fn dismiss_tip(&mut self) {
        self.tips.suppressed = self.tips.shown.or(self.tips.pending);
        self.tips.pending = None;
        self.kill_tip_timer();
        if self.tips.shown.is_some() {
            self.show_tip(None);
        }
    }

    /// Show a tooltip at rest (`--select tip-NAME` renders).
    pub(super) fn settle_tip(&mut self, tip: Tip) {
        self.show_tip(Some(tip));
    }

    fn show_tip(&mut self, tip: Option<Tip>) {
        if tip == self.tips.shown {
            return;
        }
        self.tips.shown = tip;
        let node = self.tip_node;
        if tip.is_some() {
            self.tips.drawn = tip;
            self.nodes[node].dirty = true;
            self.place_tip();
        }
        let seconds = self.t.n("dur-1") / 1000.;
        self.fade_over(node, f32::from(tip.is_some()), seconds);
        self.need = true;
    }

    /// Where a tooltip may sit: under its anchor, or above it when the
    /// window has no room below (the save row).
    fn tip_bounds(&self, anchor: Rect) -> Rect {
        let left = (anchor.x + anchor.w / 2. - HALF).clamp(0., (self.w - HALF * 2.).max(0.));
        let right = (left + HALF * 2.).min(self.w);
        let below = anchor.y + anchor.h;
        let (top, bottom) = if below + ROOM <= self.h {
            (below, below + ROOM)
        } else {
            ((anchor.y - ROOM).max(0.), anchor.y - tooltip::GAP)
        };
        Rect::new(left, top, (right - left).max(0.), (bottom - top).max(0.))
    }

    /// The rail label's box: `padding: 5px s-4`, `line-height: 1.2`, a
    /// 1 px border, vertically centred 10 px right of its button.
    fn rail_box(&self, anchor: Rect, label: &str) -> Option<(Rect, gfx::Text)> {
        let t = &self.t;
        let size = t.n("text-xs");
        let text = self
            .g
            .text_sized(t, label, size, "weight-medium", None)
            .ok()?;
        let w = (text.width + 2. * t.n("s-4") + 2.).ceil();
        let h = (size * 1.2 + 10. + 2.).ceil();
        let x = anchor.x + anchor.w + RAIL_GAP;
        let y = (anchor.y + anchor.h / 2. - h / 2.).round();
        Some((Rect::new(x, y, w, h), text))
    }

    /// Put the tooltip node over the drawn tooltip's area, or hide it when
    /// its anchor went away.
    pub(super) fn place_tip(&mut self) {
        let Some(tip) = self.tips.drawn else {
            return;
        };
        let area = self.tip_anchor(tip).and_then(|anchor| match tip {
            Tip::Rail(_) => self.rail_box(anchor, &self.tip_text(tip)?).map(|(r, _)| r),
            _ => Some(self.tip_bounds(anchor)),
        });
        let node = self.tip_node;
        match area {
            Some(r) => {
                self.set_rect(node, r);
                self.set_visible(node, true);
            }
            None => {
                self.tips = Tips::default();
                self.set_visible(node, false);
            }
        }
    }

    /// Draw the tooltip in its node's coordinates.
    pub(super) fn draw_tip(&self, dc: &ID2D1DeviceContext, (w, h): (f32, f32)) -> Result<()> {
        let Some(tip) = self.tips.drawn else {
            return Ok(());
        };
        let (Some(anchor), Some(label)) = (self.tip_anchor(tip), self.tip_text(tip)) else {
            return Ok(());
        };
        let t = &self.t;
        let origin = self.nodes[self.tip_node].rect;
        if let Tip::Rail(_) = tip {
            let Some((_, text)) = self.rail_box(anchor, &label) else {
                return Ok(());
            };
            let frame = rect(0., 0., w, h);
            let radius = t.n("r-sm");
            gfx::shadow(dc, frame, radius, t.shadow("shadow-md"))?;
            gfx::fill_round(dc, frame, radius, t.color("glass-strong"))?;
            gfx::border(dc, frame, radius, 1., t.color("glass-border"))?;
            return gfx::text(
                dc,
                &text,
                1. + t.n("s-4"),
                ((h - text.height) / 2.).round(),
                t.color("glass-text"),
            );
        }
        let local = |r: Rect| {
            rect(
                r.x - origin.x,
                r.y - origin.y,
                r.x - origin.x + r.w,
                r.y - origin.y + r.h,
            )
        };
        tooltip::paint(
            dc,
            &self.g,
            t,
            &label,
            local(anchor),
            Align::Center,
            tooltip::GAP,
            rect(0., 0., w, h),
            1.,
        )
    }
}
