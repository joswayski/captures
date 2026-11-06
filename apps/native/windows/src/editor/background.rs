//! The header's Background color picker (`CanvasBackgroundPicker`): a
//! `.screenshot-canvas-tool` trigger with the background chip, and a
//! floating card (`role="dialog" aria-label="Canvas background"`) with the
//! Solid background check and a compact `ColorField` of the shared swatches
//! plus the custom-color tile.
//!
//! The card is its own retained node above the canvas and the sidebar; its
//! controls are ordinary Properties controls (`Check::Solid` and
//! `Field::Canvas` swatches), so hover, focus and the swatch drawing are
//! shared. A change commits the document background through
//! `Request::SetBackground`, one undo step, as shipping's `commitDocument`.
use captures_app::editor_chrome::colors;
use captures_app::editor_session::Request;
use windows::Win32::Graphics::Direct2D::*;
use windows::core::*;

use super::props::{CUSTOM, Check, Field, Prop};
use super::{Editor, Event, Target};
use crate::gfx::{self, Style, rect};
use crate::scene::Rect;

/// `.screenshot-canvas-bg-chip`.
const CHIP: f32 = 14.;
/// The toggle row's `min-height: 24px`.
const TOGGLE_H: f32 = 24.;
/// `positionMenu`'s gap below the trigger and viewport padding.
const GAP: f32 = 6.;
const PADDING: f32 = 8.;

impl Editor {
    /// The document's background (`None` is transparent).
    pub fn background(&self) -> Option<String> {
        self.presented.as_ref()?.document.background.clone()
    }

    /// The swatches' value: the background, or the last solid one.
    pub(super) fn swatch_value(&self) -> String {
        self.background().unwrap_or_else(|| self.last_solid.clone())
    }

    /// The trigger's accessible name.
    pub fn background_label(&self) -> String {
        colors::background_label(self.background().as_deref())
    }

    /// `setLastSolid` follows every solid value the document shows.
    pub(super) fn follow_background(&mut self) {
        if let Some(value) = self.background()
            && value != self.last_solid
        {
            self.last_solid = value;
        }
    }

    /// `commitDocument({ ...doc, background })`; the session drops a no-op.
    pub(super) fn set_background(&mut self, color: Option<String>) {
        if self.presented.is_none() || color == self.background() {
            return;
        }
        if let Some(value) = &color {
            self.last_solid.clone_from(value);
        }
        self.notice = None;
        self.submit(Request::SetBackground { color });
    }

    pub fn bg_open(&self) -> bool {
        self.bg_open
    }

    /// Open or close the card. Closing with focus inside it returns focus to
    /// the trigger.
    pub fn set_bg_menu(&mut self, open: bool) {
        if open == self.bg_open || (open && self.presented.is_none()) {
            return;
        }
        self.bg_open = open;
        self.set_visible(self.bg_menu, open);
        if !open
            && let Some(Target::Prop(prop)) = self.focus
            && Self::in_bg_menu(prop)
        {
            self.set_focus(Some(Target::Background), self.focus_visible);
        }
        self.events.push(Event::Expanded(Target::Background));
        self.events.push(Event::Structure);
        self.nodes[self.bg_menu].dirty = true;
        self.bg_dirty();
        self.relayout = true;
        self.refresh(false);
        self.need = true;
    }

    /// The trigger and the card's controls redraw (a new background).
    pub(super) fn bg_dirty(&mut self) {
        let mut targets = vec![Target::Background];
        targets.extend(Self::bg_menu_props().map(Target::Prop));
        for target in targets {
            let layered = self.layered(target);
            for node in [layered.base, layered.lit, layered.ring] {
                self.nodes[node].dirty = true;
            }
        }
        self.need = true;
    }

    /// A control of the card.
    pub fn in_bg_menu(prop: Prop) -> bool {
        matches!(
            prop,
            Prop::Check(Check::Solid) | Prop::Swatch(Field::Canvas, _)
        )
    }

    /// The card's controls, in DOM order.
    pub(super) fn bg_menu_props() -> impl Iterator<Item = Prop> {
        std::iter::once(Prop::Check(Check::Solid))
            .chain((0..=CUSTOM).map(|i| Prop::Swatch(Field::Canvas, i)))
    }

    /// The card's width inside its padding.
    fn bg_inner_w(&self) -> f32 {
        colors::MENU_WIDTH as f32 - 2. - 2. * self.t.n("s-5")
    }

    fn bg_grid(&self) -> colors::Grid {
        colors::grid(f64::from(self.bg_inner_w()), colors::COMPACT_CELL)
    }

    /// `.screenshot-canvas-bg-menu` in client DIPs: below the trigger, or
    /// above it when there's more room there, clamped into the window.
    pub fn bg_menu_rect(&self) -> Rect {
        let (s2, s3, s4, s5) = (
            self.t.n("s-2"),
            self.t.n("s-3"),
            self.t.n("s-4"),
            self.t.n("s-5"),
        );
        let grid = self.bg_grid().height(f64::from(s3), f64::from(s2)) as f32;
        let w = colors::MENU_WIDTH as f32;
        let h = 2. + 2. * s5 + TOGGLE_H + s4 + grid;
        let trigger = self.l.background;
        let below = self.h - (trigger.y + trigger.h) - PADDING;
        let above = trigger.y - PADDING;
        let top = if below < h && above > below {
            trigger.y - h - GAP
        } else {
            trigger.y + trigger.h + GAP
        };
        let top = top.max(PADDING).min((self.h - h - PADDING).max(PADDING));
        let left = trigger
            .x
            .max(PADDING)
            .min((self.w - w - PADDING).max(PADDING));
        Rect::new(left, top, w, h)
    }

    /// A card control's rect, card-relative; `None` while it's closed.
    pub(super) fn bg_prop_rect(&self, prop: Prop) -> Option<Rect> {
        if !self.bg_open || self.presented.is_none() {
            return None;
        }
        let (s2, s3, s4, s5) = (
            self.t.n("s-2"),
            self.t.n("s-3"),
            self.t.n("s-4"),
            self.t.n("s-5"),
        );
        let x = 1. + s5;
        Some(match prop {
            Prop::Check(Check::Solid) => Rect::new(x, 1. + s5, self.bg_inner_w(), TOGGLE_H),
            Prop::Swatch(Field::Canvas, i) => {
                let tile = colors::TILE as f32;
                let (cx, cy) = self.bg_grid().center(i, f64::from(s3), f64::from(s2));
                Rect::new(
                    x + cx as f32 - tile / 2.,
                    1. + s5 + TOGGLE_H + s4 + cy as f32 - tile / 2.,
                    tile,
                    tile,
                )
            }
            _ => return None,
        })
    }

    /// The compact color field's rect in client DIPs.
    pub fn bg_field_rect(&self) -> Rect {
        let r = self.bg_menu_rect();
        let (s2, s3, s4, s5) = (
            self.t.n("s-2"),
            self.t.n("s-3"),
            self.t.n("s-4"),
            self.t.n("s-5"),
        );
        let h = self.bg_grid().height(f64::from(s3), f64::from(s2)) as f32;
        Rect::new(
            r.x + 1. + s5,
            r.y + 1. + s5 + TOGGLE_H + s4,
            self.bg_inner_w(),
            h,
        )
    }

    /// A card control was activated.
    pub(super) fn activate_bg(&mut self, prop: Prop) {
        match prop {
            Prop::Check(Check::Solid) => {
                let solid = self.background().is_some();
                let next = (!solid).then(|| self.last_solid.clone());
                self.set_background(next);
            }
            Prop::Swatch(Field::Canvas, CUSTOM) => self.pick_color = Some(Field::Canvas),
            Prop::Swatch(Field::Canvas, i) => {
                self.set_background(Some(colors::SWATCHES[i].to_owned()));
            }
            _ => {}
        }
    }

    // ---- drawing ----

    /// The trigger: chip (the color, or the transparent checkerboard), label
    /// and chevron; hovered or open it takes `--surface-hover`.
    pub(super) fn draw_bg_trigger(
        &self,
        dc: &ID2D1DeviceContext,
        lit: bool,
        (w, h): (f32, f32),
    ) -> Result<()> {
        let t = &self.t;
        let c = |name: &str| t.color(name);
        let (s3, s4) = (t.n("s-3"), t.n("s-4"));
        let all = rect(0., 0., w, h);
        if lit || self.bg_open {
            gfx::fill_round(dc, all, t.n("r-sm"), c("surface-hover"))?;
        }
        let ink = c(if lit || self.bg_open {
            "text"
        } else {
            "text-muted"
        });
        let chip = rect(
            1. + s4,
            ((h - CHIP) / 2.).round(),
            1. + s4 + CHIP,
            ((h - CHIP) / 2.).round() + CHIP,
        );
        let radius = t.n("r-xs");
        match self.background() {
            Some(hex) => gfx::fill_round(dc, chip, radius, super::hex_color(&hex))?,
            None => {
                // `.is-transparent`: 4 px checks from the chip's corner.
                gfx::fill_round(dc, chip, radius, c("canvas-checker-b"))?;
                unsafe { dc.PushAxisAlignedClip(&chip, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE) };
                let a = c("canvas-checker-a");
                let mut drawn = Ok(());
                for row in 0..4 {
                    for col in 0..4 {
                        if (row + col) % 2 == 0 && drawn.is_ok() {
                            let x = chip.left + col as f32 * 4.;
                            let y = chip.top + row as f32 * 4.;
                            drawn = gfx::fill(dc, rect(x, y, x + 4., y + 4.), a);
                        }
                    }
                }
                unsafe { dc.PopAxisAlignedClip() };
                drawn?;
            }
        }
        gfx::border(dc, chip, radius, 1., c("border-strong"))?;
        let text = self.g.text(t, Style::Button, colors::BACKGROUND, 1e4)?;
        let x = chip.right + s3;
        gfx::text(dc, &text, x, ((h - text.height) / 2.).round(), ink)?;
        gfx::icon(
            dc,
            &self.g.factory,
            "chevron-down",
            &[],
            (x + text.width + s3, (h - 11.) / 2., 11.),
            1.8,
            c("text-subtle"),
        )
    }

    /// The card's frame: `--surface-overlay`, `--border`, `--r-xl` and
    /// `--shadow-lg`.
    pub(super) fn draw_bg_menu(&self, dc: &ID2D1DeviceContext, (w, h): (f32, f32)) -> Result<()> {
        let t = &self.t;
        let all = rect(0., 0., w, h);
        let radius = t.n("r-xl");
        gfx::shadow(dc, all, radius, t.shadow("shadow-lg"))?;
        gfx::fill_round(dc, all, radius, t.color("surface-overlay"))?;
        gfx::border(dc, all, radius, 1., t.color("border"))
    }
}
