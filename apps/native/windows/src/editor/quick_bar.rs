//! The quick format bar over a selected text layer under Select
//! (`captures_app::editor_chrome::text_quick_bar`): the Font select, Smaller
//! / the size / Larger, then the Text section's Bold, Italic and alignment
//! buttons, on a solid `--surface-raised` card centred above the layer's
//! selection box (below it when there is no room) and kept inside the
//! canvas viewport. Its controls are `Prop::Quick` controls of their own, so
//! hover, focus, popups and UI Automation work as for the inspector's, and
//! every edit goes through the inspector's `apply_text`, so undo, the
//! properties panel and the canvas stay in step. It hides while a layer is
//! dragged, resized or rotated, while a label is being typed, and outside
//! Select.
use captures_app::editor::TextElement;
use captures_app::editor_chrome::{text_format, text_quick_bar as bar};
use windows::Win32::Graphics::Direct2D::*;
use windows::core::*;

use super::menu::MenuKind;
use super::props::{FORMATS, Prop, TextChange};
use super::{Editor, Event, Layer, Target};
use crate::gfx::{self, Style, rect};
use crate::scene::Rect;

/// A control of the bar, left to right.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quick {
    Font,
    Smaller,
    Larger,
    /// A Text format button: Bold, Italic, then `text_format::ALIGN`.
    Format(usize),
}

/// The bar's controls, in DOM order.
pub const PARTS: [Quick; 3 + FORMATS] = [
    Quick::Font,
    Quick::Smaller,
    Quick::Larger,
    Quick::Format(0),
    Quick::Format(1),
    Quick::Format(2),
    Quick::Format(3),
    Quick::Format(4),
];

/// Room for `--shadow-md` around the card.
pub const BLEED: f32 = 24.;

/// What sits in the bar's row.
#[derive(Clone, Copy, PartialEq)]
enum Slot {
    Part(Quick),
    Size,
    Separator,
}

impl Quick {
    /// A stable index, for UI Automation runtime ids.
    pub fn index(self) -> i32 {
        match self {
            Quick::Font => 0,
            Quick::Smaller => 1,
            Quick::Larger => 2,
            Quick::Format(i) => 3 + i as i32,
        }
    }
}

/// The row, left to right: `(slot, x, width)`, bar-relative.
fn slots() -> impl Iterator<Item = (Slot, f32, f32)> {
    let control = bar::CONTROL as f32;
    let row = [
        (Slot::Part(Quick::Font), bar::FONT_WIDTH as f32),
        (Slot::Separator, 1.),
        (Slot::Part(Quick::Smaller), control),
        (Slot::Size, bar::SIZE_WIDTH as f32),
        (Slot::Part(Quick::Larger), control),
        (Slot::Separator, 1.),
    ]
    .into_iter()
    .chain((0..FORMATS).map(move |i| (Slot::Part(Quick::Format(i)), control)));
    let mut x = bar::PADDING as f32;
    row.map(move |(slot, width)| {
        let at = x;
        x += width + bar::GAP as f32;
        (slot, at, width)
    })
}

/// The card's size: one row of controls inside its padding.
fn bar_size() -> (f32, f32) {
    let right = slots().last().map_or(0., |(_, x, w)| x + w);
    let pad = bar::PADDING as f32;
    (right + pad, bar::CONTROL as f32 + 2. * pad)
}

/// The size as the bar shows it: whole sizes without decimals.
fn size_label(size: f64) -> String {
    if size.fract() == 0. {
        format!("{size:.0}")
    } else {
        format!("{size:.1}")
    }
}

impl Editor {
    /// The label the bar is for: the selected, visible text layer while
    /// Select is active, with no label open and nothing being dragged.
    pub(super) fn quick_text(&self) -> Option<&TextElement> {
        if self.tool != "v"
            || self.inline.is_some()
            || self.gesture.is_some()
            || self.held_outline.is_some()
        {
            return None;
        }
        self.selected_text().filter(|text| text.base.visible)
    }

    /// The card in well DIPs: `text_quick_bar::place` around the selection
    /// box and the rotate grip above it, so the bar never covers the grip.
    pub(super) fn quick_rect(&self) -> Option<Rect> {
        self.quick_text()?;
        let mut points: Vec<(f32, f32)> = self
            .selection_outline()?
            .iter()
            .filter_map(|p| self.to_well(*p))
            .collect();
        if points.len() != 4 {
            return None;
        }
        if let (Some(grip), Some(image), Some((dw, _))) =
            (self.rotate_grip(), self.image_rect(), self.document_size())
            && let Some((x, y)) = self.to_well(grip.handle)
        {
            let r = (grip.hit_radius * image.width / dw) as f32;
            points.extend([(x - r, y - r), (x + r, y + r)]);
        }
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for (x, y) in points {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
        let (w, h) = bar_size();
        let [x, y] = bar::place(
            [x0, y0, x1 - x0, y1 - y0].map(f64::from),
            [w, h].map(f64::from),
            [self.l.well.w, self.l.well.h].map(f64::from),
        );
        Some(Rect::new((x as f32).round(), (y as f32).round(), w, h))
    }

    /// The card in client DIPs (hit testing, UI Automation).
    pub fn quick_client_rect(&self) -> Option<Rect> {
        let r = self.quick_rect()?;
        Some(Rect::new(
            self.l.well.x + r.x,
            self.l.well.y + r.y,
            r.w,
            r.h,
        ))
    }

    /// The size readout in client DIPs (UI Automation).
    pub fn quick_size_rect(&self) -> Rect {
        let Some(card) = self.quick_client_rect() else {
            return Rect::default();
        };
        slots()
            .find(|(slot, ..)| *slot == Slot::Size)
            .map_or_else(Rect::default, |(_, x, w)| {
                let pad = bar::PADDING as f32;
                Rect::new(card.x + x, card.y + pad, w, bar::CONTROL as f32)
            })
    }

    /// The readout's accessible text, e.g. "Text size 32".
    pub fn quick_size_name(&self) -> String {
        self.selected_text()
            .map(|t| format!("{} {}", bar::SIZE, size_label(t.font_size)))
            .unwrap_or_default()
    }

    /// Whether a client point is on the card, which takes it from the canvas.
    pub(super) fn quick_contains(&self, x: f32, y: f32) -> bool {
        self.quick_client_rect().is_some_and(|r| r.contains(x, y))
    }

    /// A control's rect, bar-relative, while the bar shows.
    pub(super) fn quick_part_rect(&self, part: Quick) -> Option<Rect> {
        self.quick_text()?;
        let (_, x, w) = slots().find(|(slot, ..)| *slot == Slot::Part(part))?;
        let pad = bar::PADDING as f32;
        Some(Rect::new(x, pad, w, bar::CONTROL as f32))
    }

    /// Smaller and Larger stop at the ends of the size ladder.
    pub fn quick_enabled(&self, part: Quick) -> bool {
        let Some(text) = self.quick_text() else {
            return false;
        };
        match part {
            Quick::Smaller | Quick::Larger => {
                bar::step(text.font_size, part == Quick::Larger) != text.font_size
            }
            Quick::Font | Quick::Format(_) => true,
        }
    }

    pub fn quick_pressed(&self, part: Quick) -> Option<bool> {
        match part {
            Quick::Format(i) => Some(self.format_on(i)),
            Quick::Font | Quick::Smaller | Quick::Larger => None,
        }
    }

    pub fn quick_name(part: Quick) -> &'static str {
        match part {
            Quick::Font => bar::FONT,
            Quick::Smaller => bar::SMALLER,
            Quick::Larger => bar::LARGER,
            Quick::Format(i) => Editor::format_name(i),
        }
    }

    /// A press or Enter on a control: the inspector's edit for it.
    pub(super) fn activate_quick(&mut self, part: Quick) {
        match part {
            Quick::Font => self.toggle_menu(MenuKind::QuickFont),
            Quick::Smaller | Quick::Larger => {
                let Some(current) = self.selected_text().map(|t| t.font_size) else {
                    return;
                };
                let next = bar::step(current, part == Quick::Larger);
                if next != current {
                    self.apply_text(TextChange::Size(next));
                }
            }
            Quick::Format(index) => {
                // The inspector's buttons announce their own toggles.
                self.activate_format(index);
                let toggled: Vec<usize> = if index >= 2 {
                    (2..FORMATS).collect()
                } else {
                    vec![index]
                };
                for i in toggled {
                    self.events
                        .push(Event::Toggled(Target::Prop(Prop::Quick(Quick::Format(i)))));
                }
            }
        }
    }

    /// Show and place the card and its controls for the current selection,
    /// zoom and pan; runs before every frame.
    pub(super) fn place_quick(&mut self) {
        let card = self.quick_rect();
        let node = self.quick_node;
        let shown = card.is_some();
        if self.nodes[node].visible != shown {
            self.events.push(Event::Structure);
            if !shown && let Some(Target::Prop(Prop::Quick(_))) = self.focus {
                self.set_focus(None, false);
            }
        }
        self.set_visible(node, shown);
        let Some(card) = card else {
            self.quick_drawn = None;
            return;
        };
        self.set_rect(node, card);
        // The card draws the size readout.
        let size = self.selected_text().map(|t| t.font_size);
        if self.quick_drawn != size {
            self.quick_drawn = size;
            self.nodes[node].dirty = true;
        }
        for part in PARTS {
            let layered = self.layered(Target::Prop(Prop::Quick(part)));
            if let Some(r) = self.quick_part_rect(part) {
                self.set_rect(layered.base, r);
                let inner = Rect::new(0., 0., r.w, r.h);
                self.set_rect(layered.lit, inner);
                self.set_rect(layered.ring, inner);
            }
            self.set_visible(layered.base, true);
        }
        // An open Font list follows its trigger.
        if self.menu_open(MenuKind::QuickFont) {
            self.place_menu();
        }
    }

    // ---- drawing ----

    /// The card: `shadow-md`, `surface-raised`, a subtle border, the
    /// separators and the size readout.
    pub(super) fn draw_quick_bar(&self, dc: &ID2D1DeviceContext, (w, h): (f32, f32)) -> Result<()> {
        let t = &self.t;
        let all = rect(0., 0., w, h);
        let radius = t.n("r-md");
        gfx::shadow(dc, all, radius, t.shadow("shadow-md"))?;
        gfx::fill_round(dc, all, radius, t.color("surface-raised"))?;
        gfx::border(dc, all, radius, 1., t.color("border-subtle"))?;
        let inset = bar::PADDING as f32 + 4.;
        for (slot, x, width) in slots() {
            match slot {
                Slot::Separator => gfx::fill(
                    dc,
                    rect(x, inset, x + 1., h - inset),
                    t.color("border-subtle"),
                )?,
                Slot::Size => {
                    let Some(size) = self.selected_text().map(|t| t.font_size) else {
                        continue;
                    };
                    let text = self.g.text(t, Style::Button, &size_label(size), 1e4)?;
                    gfx::text(
                        dc,
                        &text,
                        x + ((width - text.width) / 2.).round(),
                        ((h - text.height) / 2.).round(),
                        t.color("text"),
                    )?;
                }
                Slot::Part(_) => {}
            }
        }
        Ok(())
    }

    /// A control: the Font select's trigger, Smaller or Larger, or a format
    /// button drawn as the Text section's.
    pub(super) fn draw_quick_part(
        &self,
        dc: &ID2D1DeviceContext,
        part: Quick,
        layer: Layer,
        (w, h): (f32, f32),
    ) -> Result<()> {
        let t = &self.t;
        let c = |name: &str| t.color(name);
        let all = rect(0., 0., w, h);
        if layer == Layer::Ring {
            return match part {
                Quick::Font => gfx::ring(
                    dc,
                    all,
                    t.n("r-md"),
                    0.,
                    2.,
                    t.color_alpha("theme-accent", 0.55),
                ),
                Quick::Smaller | Quick::Larger | Quick::Format(_) => {
                    gfx::ring(dc, all, t.n("r-md"), 2., 4., c("theme-accent"))
                }
            };
        }
        match part {
            Quick::Font => self.draw_select(dc, MenuKind::QuickFont, layer, (w, h)),
            Quick::Format(index) => self.draw_format(dc, index, layer, (w, h)),
            Quick::Smaller | Quick::Larger => {
                // An unpressed format button with the shipping icon.
                let radius = t.n("r-sm");
                let ink = if layer == Layer::Lit {
                    gfx::fill_round(dc, all, radius, c("control-hover"))?;
                    gfx::border(dc, all, radius, 1., c("border-subtle"))?;
                    c("text")
                } else {
                    gfx::fill_round(dc, all, radius, c("control"))?;
                    gfx::border(dc, all, radius, 1., c("border-subtle"))?;
                    c("text-muted")
                };
                let size = text_format::ICON as f32;
                let icon = if part == Quick::Larger {
                    "plus"
                } else {
                    "minus"
                };
                gfx::icon(
                    dc,
                    &self.g.factory,
                    icon,
                    &[],
                    ((w - size) / 2., (h - size) / 2., size),
                    1.8,
                    ink,
                )
            }
        }
    }
}
