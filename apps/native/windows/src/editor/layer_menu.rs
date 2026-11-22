//! Shipping's per-layer settings card (`.screenshot-layer-menu-panel`), opened
//! from a Layers row's ⋯: Appearance (Blend mode and Opacity) stays on top,
//! Transform (image layers only), Arrange and Combine scroll under it, and
//! Duplicate and Delete sit in the footer. The card is placed beside the
//! sidebar like shipping's `place()`, and its rows' rules follow GTK's
//! `layer_menu.rs` and the shared `editor_layers::menu` copy.
use captures_app::editor::{Element, ImageTransform, LayerEdit};
use captures_app::editor_layers::{BLEND_MODES, menu};
use captures_app::editor_session::Request;
use windows::Win32::Graphics::Direct2D::*;
use windows::core::*;

use super::import::fresh_id;
use super::menu::MenuKind;
use super::props::{Prop, Range};
use super::{Editor, Event, Layer, Target};
use crate::gfx::{self, rect};
use crate::scene::Rect;

/// A card button, in DOM order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    RotateLeft,
    RotateRight,
    FlipH,
    FlipV,
    Front,
    Back,
    MergeDown,
    MergeVisible,
    Flatten,
    Duplicate,
    Delete,
}

pub const ACTS: [Act; 11] = [
    Act::RotateLeft,
    Act::RotateRight,
    Act::FlipH,
    Act::FlipV,
    Act::Front,
    Act::Back,
    Act::MergeDown,
    Act::MergeVisible,
    Act::Flatten,
    Act::Duplicate,
    Act::Delete,
];

/// `width: min(280px, 100vw - 24px)`, `place()`'s gap and margins.
const CARD_W: f32 = 280.;
const GAP: f32 = 10.;
const MARGIN: f32 = 8.;
/// `.screenshot-layer-menu-tile { min-height: 36px }` and
/// `.screenshot-layer-menu-action { min-height: 34px }`.
const TILE_H: f32 = 36.;
const ACTION_H: f32 = 34.;
/// `.screenshot-layer-menu-actions { gap: 2px }`.
const ACTION_GAP: f32 = 2.;

impl Act {
    /// A Transform tile (image layers only).
    pub fn is_tile(self) -> bool {
        matches!(
            self,
            Act::RotateLeft | Act::RotateRight | Act::FlipH | Act::FlipV
        )
    }

    fn in_footer(self) -> bool {
        matches!(self, Act::Duplicate | Act::Delete)
    }

    fn danger(self) -> bool {
        self == Act::Delete
    }

    fn icon(self) -> &'static str {
        match self {
            Act::RotateLeft => "rotate-counterclockwise",
            Act::RotateRight => "rotate-clockwise",
            Act::FlipH => "flip-horizontal",
            Act::FlipV => "flip-vertical",
            Act::Front => "bring-front",
            Act::Back => "send-back",
            Act::MergeDown => "merge-down",
            Act::MergeVisible => "merge-visible",
            Act::Flatten => "flatten",
            Act::Duplicate => "duplicate",
            Act::Delete => "trash",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Act::RotateLeft => menu::ROTATE_LEFT,
            Act::RotateRight => menu::ROTATE_RIGHT,
            Act::FlipH => menu::FLIP_HORIZONTAL,
            Act::FlipV => menu::FLIP_VERTICAL,
            Act::Front => menu::BRING_FRONT,
            Act::Back => menu::SEND_BACK,
            Act::MergeDown => menu::MERGE_DOWN,
            Act::MergeVisible => menu::MERGE_VISIBLE,
            Act::Flatten => menu::FLATTEN,
            Act::Duplicate => menu::DUPLICATE,
            Act::Delete => menu::DELETE,
        }
    }

    /// The accessible name: the tiles have their own `aria-label`.
    pub fn name(self) -> &'static str {
        match self {
            Act::RotateLeft => "Rotate image counterclockwise",
            Act::RotateRight => "Rotate image clockwise",
            Act::FlipH => "Flip image horizontally",
            Act::FlipV => "Flip image vertically",
            act => act.label(),
        }
    }

    /// Its `title`.
    pub fn tip(self) -> &'static str {
        match self {
            Act::RotateLeft => menu::ROTATE_LEFT_TIP,
            Act::RotateRight => menu::ROTATE_RIGHT_TIP,
            Act::FlipH => menu::FLIP_HORIZONTAL_TIP,
            Act::FlipV => menu::FLIP_VERTICAL_TIP,
            Act::Front => menu::BRING_FRONT_TIP,
            Act::Back => menu::SEND_BACK_TIP,
            Act::MergeDown => menu::MERGE_DOWN_TIP,
            Act::MergeVisible => menu::MERGE_VISIBLE_TIP,
            Act::Flatten => menu::FLATTEN_TIP,
            Act::Duplicate => menu::DUPLICATE_TIP,
            Act::Delete => menu::DELETE_TIP,
        }
    }
}

/// The card's geometry: its client rect, the static section's height, the
/// scroll viewport (card-relative) and where the footer starts.
pub struct Geom {
    pub rect: Rect,
    pub static_h: f32,
    pub view: Rect,
    pub content_h: f32,
    pub footer_y: f32,
}

impl Editor {
    /// The open card's layer row.
    pub fn card_row(&self) -> Option<usize> {
        let id = self.layer_card.as_deref()?;
        self.presented
            .as_ref()?
            .layers
            .iter()
            .position(|l| l.id == id)
    }

    /// The open card's layer.
    pub(super) fn card_element(&self) -> Option<&Element> {
        let id = self.layer_card.as_deref()?;
        self.presented
            .as_ref()?
            .document
            .elements
            .iter()
            .find(|e| e.base().id == id)
    }

    /// A control of the card.
    pub fn in_layer_card(prop: Prop) -> bool {
        matches!(prop, Prop::Blend | Prop::Range(Range::LayerOpacity))
    }

    /// The card's controls, in DOM order (shown or not).
    pub fn card_targets(&self) -> Vec<Target> {
        let mut list = vec![
            Target::Prop(Prop::Blend),
            Target::Prop(Prop::Range(Range::LayerOpacity)),
        ];
        list.extend(ACTS.map(Target::LayerAct));
        list
    }

    pub fn card_act_exists(&self, act: Act) -> bool {
        let Some(row) = self.card_row() else {
            return false;
        };
        !act.is_tile() || self.layer(row).is_some_and(|l| l.is_image)
    }

    /// Shipping's `disabled` rules (GTK `LayerMenu::sync`).
    pub fn card_act_enabled(&self, act: Act) -> bool {
        let (Some(row), Some(p)) = (self.card_row(), self.presented.as_ref()) else {
            return false;
        };
        let layer = &p.layers[row];
        match act {
            // Rows are top first.
            Act::Front => !layer.locked && row > 0,
            Act::Back => !layer.locked && row + 1 < p.layers.len(),
            Act::MergeDown => p.merge_down_ids.contains(&layer.id),
            Act::MergeVisible => p.can_merge_visible,
            Act::Flatten => p.can_flatten,
            Act::Delete => !layer.locked,
            _ => self.card_act_exists(act),
        }
    }

    /// The ⋯ of row `i`: open its card, or close it.
    pub fn toggle_layer_card(&mut self, row: usize) {
        let Some(id) = self.layer(row).map(|l| l.id.clone()) else {
            return;
        };
        if self.layer_card.as_deref() == Some(id.as_str()) {
            self.close_layer_card();
            return;
        }
        self.close_layer_card();
        if self.inline.is_some() {
            self.finish_text();
        }
        self.select_tool("v");
        self.set_crop(None);
        self.set_selected(Some(id.clone()));
        self.layer_card = Some(id);
        self.card_scroll = 0.;
        self.events.push(Event::Expanded(Target::More(row)));
        self.events.push(Event::Structure);
        self.card_changed();
    }

    /// Close the card; focus inside it returns to its ⋯.
    pub fn close_layer_card(&mut self) {
        let row = self.card_row();
        if self.layer_card.take().is_none() {
            return;
        }
        if self.menu.is_some_and(|m| m.kind == MenuKind::Blend) {
            self.close_menu();
        }
        if let Some(focus) = self.focus
            && self.card_targets().contains(&focus)
        {
            let back = row.map(Target::More).filter(|&t| self.exists(t));
            self.set_focus(back, self.focus_visible);
        }
        if let Some(row) = row {
            self.events.push(Event::Expanded(Target::More(row)));
        }
        self.events.push(Event::Structure);
        self.card_changed();
    }

    /// The document changed: a card whose layer is gone closes, an open one
    /// shows the new values and enabled states.
    pub(super) fn sync_layer_card(&mut self) {
        if self.layer_card.is_some() && self.card_row().is_none() {
            self.close_layer_card();
        } else if self.layer_card.is_some() {
            self.card_changed();
        }
    }

    /// Redraw the card, its controls and the rows' ⋯.
    pub(super) fn card_changed(&mut self) {
        let open = self.layer_card.is_some();
        for node in [self.card_node, self.card_body] {
            self.nodes[node].dirty = true;
        }
        self.set_visible(self.card_node, open);
        let mut targets = self.card_targets();
        targets.extend((0..self.layer_count()).map(Target::More));
        for target in targets {
            if self.controls.iter().any(|(t, _)| *t == target) {
                let layered = self.layered(target);
                for node in [layered.base, layered.lit, layered.ring] {
                    self.nodes[node].dirty = true;
                }
            }
        }
        self.relayout = true;
        self.refresh(false);
        self.need = true;
    }

    /// A card button.
    pub(super) fn layer_action(&mut self, act: Act) {
        let Some(id) = self.layer_card.clone() else {
            return;
        };
        let transform = |t: ImageTransform| LayerEdit::ImageTransform { transform: t };
        let edit = |id: String, edit: LayerEdit| Request::Layer { id, edit };
        match act {
            Act::RotateLeft => {
                self.submit(edit(id, transform(ImageTransform::RotateCounterclockwise)))
            }
            Act::RotateRight => self.submit(edit(id, transform(ImageTransform::RotateClockwise))),
            Act::FlipH => self.submit(edit(id, transform(ImageTransform::FlipHorizontal))),
            Act::FlipV => self.submit(edit(id, transform(ImageTransform::FlipVertical))),
            Act::Front | Act::Back => {
                self.submit(edit(
                    id.clone(),
                    LayerEdit::Arrange {
                        front: act == Act::Front,
                    },
                ));
                self.set_selected(Some(id));
                self.close_layer_card();
            }
            Act::MergeDown | Act::MergeVisible | Act::Flatten => {
                let new_id = fresh_id();
                self.select_after = Some(new_id.clone());
                self.submit(match act {
                    Act::MergeDown => Request::MergeDown { id, new_id },
                    Act::MergeVisible => Request::MergeVisible { new_id },
                    _ => Request::Flatten { new_id },
                });
                self.select_tool("v");
                self.close_layer_card();
            }
            Act::Duplicate => {
                self.duplicate_layer(id);
                self.close_layer_card();
            }
            Act::Delete => {
                if self.selected.as_deref() == Some(id.as_str()) {
                    self.set_selected(None);
                }
                self.submit(edit(id, LayerEdit::Delete));
                self.close_layer_card();
            }
        }
    }

    /// Blend mode's options and choice.
    pub fn blend_items() -> Vec<String> {
        BLEND_MODES.iter().map(|(_, l)| (*l).to_owned()).collect()
    }

    pub fn blend_selected(&self) -> Option<usize> {
        let mode = &self.card_element()?.base().blend_mode;
        BLEND_MODES.iter().position(|(v, _)| v == mode)
    }

    pub(super) fn set_blend(&mut self, index: usize) {
        let (Some(id), Some((mode, _))) = (self.layer_card.clone(), BLEND_MODES.get(index)) else {
            return;
        };
        self.submit(Request::Layer {
            id,
            edit: LayerEdit::BlendMode {
                blend_mode: (*mode).to_owned(),
            },
        });
        self.prop_dirty(Prop::Blend);
    }

    /// Layer opacity, folded into one undo step per drag.
    pub(super) fn set_layer_opacity(&mut self, opacity: f64) {
        let Some(id) = self.layer_card.clone() else {
            return;
        };
        self.submit_live(Request::Live {
            key: format!("{id}:layer-opacity"),
            request: Box::new(Request::Layer {
                id,
                edit: LayerEdit::Opacity { opacity },
            }),
        });
        let prop = Prop::Range(Range::LayerOpacity);
        self.events.push(Event::Value(Target::Prop(prop)));
        self.prop_dirty(prop);
    }

    pub(super) fn card_opacity(&self) -> f64 {
        self.card_element()
            .map_or(100., |e| e.base().opacity.round())
    }

    // ---- layout ----

    fn title_h(&self) -> f32 {
        self.t.n("text-2xs")
    }

    fn label_h(&self) -> f32 {
        self.t.n("text-sm") * 1.2
    }

    fn inner_w(&self, w: f32) -> f32 {
        w - 2. - 2. * self.t.n("s-5")
    }

    fn static_h(&self) -> f32 {
        let n = |name: &str| self.t.n(name);
        1. + n("s-5")
            + self.title_h()
            + n("s-4")
            + self.label_h()
            + n("s-3")
            + n("h-md")
            + n("s-4")
            + self.range_row_h()
            + n("s-5")
            + 1.
    }

    /// The scroll region's sections: their titles, body tops and rules, and
    /// its height, in content coordinates.
    fn card_sections(&self) -> (Vec<(&'static str, f32, f32)>, f32) {
        let n = |name: &str| self.t.n(name);
        let image = self.card_act_exists(Act::RotateLeft);
        let mut sections = Vec::new();
        if image {
            sections.push((menu::TRANSFORM, 2. * TILE_H + n("s-3")));
        }
        sections.push((menu::ARRANGE, 2. * ACTION_H + ACTION_GAP));
        sections.push((menu::COMBINE, 3. * ACTION_H + 2. * ACTION_GAP));
        let mut y = 0.;
        let mut out = Vec::new();
        let count = sections.len();
        for (k, (title, body)) in sections.into_iter().enumerate() {
            let top = y + n("s-5");
            out.push((title, top, top + self.title_h() + n("s-4")));
            y = top + self.title_h() + n("s-4") + body + n("s-5");
            if k + 1 < count {
                y += 1.;
            }
        }
        (out, y)
    }

    fn footer_h(&self) -> f32 {
        let n = |name: &str| self.t.n(name);
        1. + n("s-4") + 2. * ACTION_H + n("s-2") + n("s-4")
    }

    /// The open card's geometry, placed like shipping's `place()`.
    pub fn card_geom(&self) -> Option<Geom> {
        let row = self.card_row()?;
        let list = self.l.list;
        let r = self.row_part(Target::Layer(row));
        let m = self.row_part(Target::More(row));
        let trigger = Rect::new(list.x + r.x + m.x, list.y + r.y + m.y, m.w, m.h);
        let w = CARD_W.min(self.w - 24.).max(1.);
        let menu_h = 560f32.min(self.h - 16.);
        let mut left = trigger.x - w - GAP;
        if left < MARGIN {
            left = (trigger.x + trigger.w + GAP).min((self.w - w - MARGIN).max(MARGIN));
        }
        let top = trigger
            .y
            .min((self.h - menu_h - MARGIN).max(MARGIN))
            .max(MARGIN);
        let max_h = (self.h - top - MARGIN).max(160.);
        let static_h = self.static_h();
        let (_, content_h) = self.card_sections();
        let footer_h = self.footer_h();
        let h = (static_h + content_h + footer_h + 1.).min(max_h);
        let view_h = (h - static_h - footer_h - 1.).max(0.);
        Some(Geom {
            rect: Rect::new(left, top, w, h),
            static_h,
            view: Rect::new(1., static_h, w - 2., view_h),
            content_h,
            footer_y: static_h + view_h,
        })
    }

    /// Blend mode's and Opacity's rects, card-relative.
    pub(super) fn card_prop_rect(&self, prop: Prop) -> Option<Rect> {
        let g = self.card_geom()?;
        let n = |name: &str| self.t.n(name);
        let x = 1. + n("s-5");
        let blend_y = 1. + n("s-5") + self.title_h() + n("s-4") + self.label_h() + n("s-3");
        let inner = self.inner_w(g.rect.w);
        Some(match prop {
            Prop::Blend => Rect::new(x, blend_y, inner, n("h-md")),
            Prop::Range(Range::LayerOpacity) => {
                Rect::new(x, blend_y + n("h-md") + n("s-4"), inner, self.range_row_h())
            }
            _ => return None,
        })
    }

    /// A card button's rect relative to its parent: the scroll body's
    /// content for Transform, Arrange and Combine, the card for the footer.
    pub(super) fn card_act_rect(&self, act: Act) -> Option<Rect> {
        if !self.card_act_exists(act) {
            return None;
        }
        let g = self.card_geom()?;
        let n = |name: &str| self.t.n(name);
        let x = 1. + n("s-5");
        let inner = self.inner_w(g.rect.w);
        if act.in_footer() {
            let y = g.footer_y + 1. + n("s-4");
            let k = if act == Act::Delete { 1. } else { 0. };
            return Some(Rect::new(
                1. + n("s-4"),
                y + k * (ACTION_H + n("s-2")),
                g.rect.w - 2. - 2. * n("s-4"),
                ACTION_H,
            ));
        }
        let (sections, _) = self.card_sections();
        let body = |title: &str| {
            sections
                .iter()
                .find(|(t, ..)| *t == title)
                .map(|(.., body)| *body)
        };
        // The scroll body starts at the viewport's left edge (x = 1).
        let x = x - 1.;
        Some(match act {
            Act::RotateLeft | Act::RotateRight | Act::FlipH | Act::FlipV => {
                let top = body(menu::TRANSFORM)?;
                let k = ACTS.iter().position(|a| *a == act).unwrap_or(0);
                let tile_w = (inner - n("s-3")) / 2.;
                Rect::new(
                    x + (k % 2) as f32 * (tile_w + n("s-3")),
                    top + (k / 2) as f32 * (TILE_H + n("s-3")),
                    tile_w,
                    TILE_H,
                )
            }
            Act::Front | Act::Back => {
                let k = if act == Act::Back { 1. } else { 0. };
                Rect::new(
                    x,
                    body(menu::ARRANGE)? + k * (ACTION_H + ACTION_GAP),
                    inner,
                    ACTION_H,
                )
            }
            _ => {
                let k = match act {
                    Act::MergeDown => 0.,
                    Act::MergeVisible => 1.,
                    _ => 2.,
                };
                Rect::new(
                    x,
                    body(menu::COMBINE)? + k * (ACTION_H + ACTION_GAP),
                    inner,
                    ACTION_H,
                )
            }
        })
    }

    /// Place the card, its scroll region and body.
    pub(super) fn place_layer_card(&mut self) {
        let Some(g) = self.card_geom() else {
            self.set_visible(self.card_node, false);
            return;
        };
        self.card_scroll = self.card_scroll.clamp(0., (g.content_h - g.view.h).max(0.));
        self.set_rect(self.card_node, g.rect);
        self.set_visible(self.card_node, true);
        self.set_rect(self.card_clip, g.view);
        self.set_rect(
            self.card_body,
            Rect::new(0., -self.card_scroll, g.view.w, g.content_h.max(1.)),
        );
    }

    /// The card's control under a client point (`Some(None)` over the card
    /// itself), `None` outside it.
    pub(super) fn card_hit(&self, x: f32, y: f32) -> Option<Option<Target>> {
        let g = self.card_geom()?;
        if !g.rect.contains(x, y) {
            return None;
        }
        let view = Rect::new(g.rect.x + g.view.x, g.rect.y + g.view.y, g.view.w, g.view.h);
        Some(self.card_targets().into_iter().find(|&t| {
            let scrolled = matches!(t, Target::LayerAct(a) if !a.in_footer());
            self.exists(t)
                && self.target_rect(t).contains(x, y)
                && (!scrolled || view.contains(x, y))
        }))
    }

    /// The wheel over the card; true when it took it.
    pub(super) fn wheel_card(&mut self, x: f32, y: f32, pixels: f32) -> bool {
        let Some(g) = self.card_geom() else {
            return false;
        };
        if !g.rect.contains(x, y) {
            return false;
        }
        let before = self.card_scroll;
        self.card_scroll = (self.card_scroll - pixels).clamp(0., (g.content_h - g.view.h).max(0.));
        if self.card_scroll != before {
            self.relayout = true;
            self.need = true;
        }
        true
    }

    // ---- painting ----

    fn draw_card_title(&self, dc: &ID2D1DeviceContext, title: &str, x: f32, y: f32) -> Result<()> {
        let t = &self.t;
        let text = self.g.text_sized(
            t,
            &title.to_uppercase(),
            t.n("text-2xs"),
            "weight-semibold",
            Some("tracking-wide"),
        )?;
        gfx::text(
            dc,
            &text,
            x,
            (y + (self.title_h() - text.height) / 2.).round(),
            t.color("text-subtle"),
        )
    }

    /// The card's frame, Appearance's title and label, its rule and the
    /// footer's sunken band.
    pub(super) fn draw_layer_card(
        &self,
        dc: &ID2D1DeviceContext,
        (w, h): (f32, f32),
    ) -> Result<()> {
        let Some(g) = self.card_geom() else {
            return Ok(());
        };
        let t = &self.t;
        let c = |name: &str| t.color(name);
        let n = |name: &str| t.n(name);
        let all = rect(0., 0., w, h);
        let radius = n("r-xl");
        gfx::shadow(dc, all, radius, t.shadow("shadow-lg"))?;
        gfx::fill_round(dc, all, radius, c("surface-overlay"))?;
        let x = 1. + n("s-5");
        self.draw_card_title(dc, menu::APPEARANCE, x, 1. + n("s-5"))?;
        let label = self
            .g
            .text_sized(t, menu::BLEND_MODE, n("text-sm"), "weight-medium", None)?;
        let ly = 1. + n("s-5") + self.title_h() + n("s-4");
        gfx::text(
            dc,
            &label,
            x,
            (ly + (self.label_h() - label.height) / 2.).round(),
            c("text-muted"),
        )?;
        gfx::fill(
            dc,
            rect(1., g.static_h - 1., w - 1., g.static_h),
            c("border-subtle"),
        )?;
        let fy = g.footer_y;
        gfx::fill_round(
            dc,
            rect(1., fy, w - 1., h - 1.),
            radius - 1.,
            c("surface-sunken"),
        )?;
        gfx::fill(dc, rect(1., fy, w - 1., fy + radius), c("surface-sunken"))?;
        gfx::fill(dc, rect(1., fy, w - 1., fy + 1.), c("border-subtle"))?;
        gfx::border(dc, all, radius, 1., c("border"))
    }

    /// The scroll body: section titles and the rules between sections.
    pub(super) fn draw_layer_scroll(
        &self,
        dc: &ID2D1DeviceContext,
        (w, _): (f32, f32),
    ) -> Result<()> {
        let (sections, _) = self.card_sections();
        let n = |name: &str| self.t.n(name);
        for (k, (title, top, _)) in sections.iter().enumerate() {
            self.draw_card_title(dc, title, n("s-5"), *top)?;
            if k > 0 {
                let y = top - n("s-5") - 1.;
                gfx::fill(dc, rect(0., y, w, y + 1.), self.t.color("border-subtle"))?;
            }
        }
        Ok(())
    }

    /// A card button: `.screenshot-layer-menu-tile`, `-action`, or a footer
    /// action. Disabled ones fade through the base layer's opacity.
    pub(super) fn draw_layer_act(
        &self,
        dc: &ID2D1DeviceContext,
        act: Act,
        layer: Layer,
        (w, h): (f32, f32),
    ) -> Result<()> {
        let t = &self.t;
        let g = &self.g;
        let c = |name: &str| t.color(name);
        let n = |name: &str| t.n(name);
        let all = rect(0., 0., w, h);
        let radius = n("r-md");
        if layer == Layer::Ring {
            return if act.is_tile() {
                gfx::ring(dc, all, radius, 2., 4., c("theme-accent"))
            } else {
                gfx::ring(dc, all, radius, 0., 2., t.color_alpha("theme-accent", 0.55))
            };
        }
        let lit = layer == Layer::Lit;
        if act.is_tile() {
            let (bg, edge, ink) = if lit {
                ("control-hover", "border-strong", "text")
            } else {
                ("control", "border-subtle", "text-muted")
            };
            gfx::fill_round(dc, all, radius, c(bg))?;
            gfx::border(dc, all, radius, 1., c(edge))?;
            let x = 1. + n("s-4");
            gfx::icon(
                dc,
                &g.factory,
                act.icon(),
                &[],
                (x, (h - 15.) / 2., 15.),
                1.8,
                c(ink),
            )?;
            let text = g.text_sized(t, act.label(), n("text-sm"), "weight-medium", None)?;
            let lx = x + 16. + n("s-3");
            g.ellipsis(&text, (w - lx - n("s-4")).max(1.))?;
            return gfx::text(dc, &text, lx, ((h - text.height) / 2.).round(), c(ink));
        }
        if act.in_footer() {
            let (bg, edge) = match (lit, act.danger()) {
                (true, _) => ("control-hover", "border-strong"),
                (false, true) => ("danger-surface", "danger-border"),
                (false, false) => ("control", "border-subtle"),
            };
            gfx::fill_round(dc, all, radius, c(bg))?;
            gfx::border(dc, all, radius, 1., c(edge))?;
        } else if lit {
            let bg = if act.danger() {
                "danger-surface"
            } else {
                "surface-hover"
            };
            gfx::fill_round(dc, all, radius, c(bg))?;
        }
        let (icon_ink, ink) = if act.danger() {
            ("danger-text", "danger-text")
        } else {
            ("text-muted", "text")
        };
        let x = 1. + n("s-4");
        gfx::icon(
            dc,
            &g.factory,
            act.icon(),
            &[],
            (x + 1.5, (h - 15.) / 2., 15.),
            1.8,
            c(icon_ink),
        )?;
        let text = g.text_sized(t, act.label(), n("text-md"), "weight-medium", None)?;
        let lx = x + 18. + n("s-4");
        g.ellipsis(&text, (w - lx - n("s-4")).max(1.))?;
        gfx::text(dc, &text, lx, ((h - text.height) / 2.).round(), c(ink))
    }

    /// A row's ⋯ (`.screenshot-layer-menu > button`), active while its card
    /// is open.
    pub(super) fn draw_more(
        &self,
        dc: &ID2D1DeviceContext,
        row: usize,
        lit: bool,
        (w, h): (f32, f32),
    ) -> Result<()> {
        let t = &self.t;
        let c = |name: &str| t.color(name);
        let all = rect(0., 0., w, h);
        let radius = t.n("r-sm");
        let ink = if lit {
            gfx::fill_round(dc, all, radius, c("surface-active"))?;
            c("text")
        } else if self.card_row() == Some(row) {
            gfx::fill_round(dc, all, radius, c("surface-selected"))?;
            c(if t.appearance == "dark" {
                "theme-accent-text"
            } else {
                "theme-accent-readable"
            })
        } else {
            t.color_alpha("text-subtle", 0.5)
        };
        gfx::icon(
            dc,
            &self.g.factory,
            "more",
            &[],
            ((w - 15.) / 2., (h - 15.) / 2., 15.),
            1.8,
            ink,
        )
    }
}
