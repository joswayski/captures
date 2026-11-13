//! Floating mini previews (`?view=thumbnail`): each saved screenshot joins
//! one topmost, non-activating stack window in the configured corner of the
//! display under the pointer. Cards follow `styles/mini-preview.css`: the
//! cover image with its size chip and the clipboard confirmation, and on
//! hover a blurred, dimmed image under Close and Delete, Edit, and Copy and
//! Save file (Show in Folder once saved). Membership, geometry and copy come
//! from `captures_app::preview` and `preview_chrome`.
//!
//! UI Automation lists each card as shipping's `<article>` with its buttons
//! (Invoke), size line and clipboard status, then Clear all previews
//! ([`Stack::uia_items`] through `panel_uia`). Buttons are exposed whether or
//! not the card is hovered, as they are in shipping's DOM.
//!
//! Not ported yet: the collapsed pile and its Minimize control, arrive/exit
//! motion, file drags, keyboard focus, tooltips and the editor-presence pill.

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};

use captures_app::motion::{Transition, Tween};
use captures_app::preview::{self, CollapsedCardPose, PreviewStack};
use captures_app::preview_chrome;
use captures_app::preview_motion::StackFan;
use captures_history::HistoryEntry;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectComposition::*;
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint, ValidateRect,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Accessibility::{UiaReturnRawElementProvider, UiaRootObjectId};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;
use windows_numerics::Matrix3x2;

use crate::gfx::{self, Gfx, rect};
use crate::panel_uia::{self, Item, Role};
use crate::tokens::Tokens;

/// Not exported by the `windows` crate's `WindowsAndMessaging`.
const WM_MOUSELEAVE: u32 = 0x02A3;
/// Drives the pile's fly and fan while they run.
const MOTION_TIMER: usize = 7;
/// Polls the pointer while empty areas let clicks through.
const POLL_TIMER: usize = 8;
/// Posted when a worker finished a Save or Delete.
const WM_APP_PREVIEW_DONE: u32 = WM_APP + 31;

type Rect = D2D_RECT_F;

/// The card width inside the stack window's side padding.
const CARD_W: f32 = (preview::THUMBNAIL_WIDTH - 2. * preview::THUMBNAIL_PADDING) as f32;
const CARD_H: f32 = preview::THUMBNAIL_CARD_HEIGHT as f32;
const PAD: f32 = preview::THUMBNAIL_PADDING as f32;
const GUTTER: f32 = preview::THUMBNAIL_CONTROL_GUTTER as f32;
/// `.icon-button` and `.thumbnail-stack-control`.
const CONTROL: f32 = 28.;
/// `.thumbnail-main-actions button`.
const ACTION_W: f32 = 140.;
const ACTION_H: f32 = 32.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Close(usize),
    Delete(usize),
    Edit(usize),
    Copy(usize),
    /// Save file, or Show in Folder once the card has a saved path.
    Save(usize),
    Card(usize),
    ClearAll,
    /// `.thumbnail-stack-minimize`: collapse into the pile.
    Minimize,
    /// `.thumbnail-collapsed-hit-target`: the pile's one button, which
    /// expands it.
    Expand,
}

impl Hit {
    fn card(self) -> Option<usize> {
        match self {
            Self::Close(i)
            | Self::Delete(i)
            | Self::Edit(i)
            | Self::Copy(i)
            | Self::Save(i)
            | Self::Card(i) => Some(i),
            Self::ClearAll | Self::Minimize | Self::Expand => None,
        }
    }
}

pub struct Card {
    pub entry: HistoryEntry,
    image: Option<ID2D1Bitmap1>,
    /// This card's image is what the clipboard holds (`clipboardCurrent`).
    pub clipboard: bool,
    /// A Save or Delete is running.
    busy: bool,
    /// Save finished: the primary action reads Saved until the pointer leaves.
    saved: bool,
}

/// The stack's state and drawing, independent of its window so the
/// offscreen renderer can paint it.
pub struct Stack {
    pub cards: Vec<Card>,
    pub stack: PreviewStack,
    pub top: bool,
    pub right: bool,
    /// Window height in DIPs; the content scrolls to keep the newest card
    /// and the controls in view when it is taller.
    pub height: f32,
    pub hovered: Option<usize>,
    pub hot: Option<Hit>,
    pub pressed: Option<Hit>,
    /// The pointer is over the collapsed pile, which fans it.
    pub pile_hovered: bool,
    /// Shipping's `preview_stack_fly` and `preview_stack_fan` tweens; `None`
    /// (offscreen fixtures) lands every change at once.
    pub motion: Option<(Tween, Tween)>,
    pub reduced: bool,
    /// Collapse progress when the latest fly began, and when (ms).
    fly: (f64, f64),
    fan: StackFan,
}

/// Milliseconds on a monotonic clock shared by every preview motion.
fn now_ms() -> f64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_secs_f64()
        * 1000.
}

fn lerp(from: f64, to: f64, t: f64) -> f64 {
    from + (to - from) * t
}

/// A rear card between its rest and fanned poses.
fn mix(rest: CollapsedCardPose, fan: CollapsedCardPose, t: f64) -> CollapsedCardPose {
    CollapsedCardPose {
        dx: lerp(rest.dx, fan.dx, t),
        dy: lerp(rest.dy, fan.dy, t),
        rotation_deg: lerp(rest.rotation_deg, fan.rotation_deg, t),
        scale_x: lerp(rest.scale_x, fan.scale_x, t),
        scale_y: lerp(rest.scale_y, fan.scale_y, t),
        ..rest
    }
}

impl Stack {
    pub fn new(placement: captures_settings::MiniPreviewPlacement) -> Self {
        Self {
            cards: Vec::new(),
            stack: PreviewStack::default(),
            top: placement.is_top(),
            right: placement.is_right(),
            height: 0.,
            hovered: None,
            hot: None,
            pressed: None,
            pile_hovered: false,
            motion: None,
            reduced: false,
            fly: (0., 0.),
            fan: StackFan::default(),
        }
    }

    /// Whether changes land at once.
    fn instant(&self) -> bool {
        self.reduced || self.motion.is_none()
    }

    /// Eased collapse progress: 0 is the list, 1 the pile.
    pub fn progress(&self) -> f64 {
        let target = if self.collapsed() { 1. } else { 0. };
        match &self.motion {
            Some((fly, _)) => fly.value(self.fly.0, target, now_ms() - self.fly.1, self.reduced),
            None => target,
        }
    }

    /// The fly or the fan is still moving.
    pub fn animating(&self) -> bool {
        let Some((fly, fan)) = &self.motion else {
            return false;
        };
        let now = now_ms();
        fly.running(now - self.fly.1, self.reduced)
            || self
                .fan
                .running(self.cards.len().saturating_sub(1), now, fan, self.reduced)
    }

    /// Collapse into the pile or expand it, flying from wherever the cards
    /// are now.
    pub fn set_collapsed(&mut self, collapsed: bool) {
        let from = self.progress();
        self.stack.set_collapsed(collapsed);
        self.fly = (from, now_ms());
        self.hovered = None;
        self.hot = None;
        self.pile_hovered = false;
        self.set_fan(false);
    }

    fn set_fan(&mut self, open: bool) {
        let count = self.cards.len();
        let cards: Vec<(&str, usize)> = self
            .cards
            .iter()
            .enumerate()
            .map(|(i, c)| (c.entry.id.as_str(), count - 1 - i))
            .collect();
        let tween = self.motion.map_or(
            Tween {
                duration_ms: 0.,
                easing: captures_app::motion::CubicBezier::LINEAR,
            },
            |(_, fan)| fan,
        );
        let instant = self.instant();
        self.fan.set(open, now_ms(), &cards, &tween, instant);
    }

    pub fn push(&mut self, g: &Gfx, entry: HistoryEntry, image: Option<&image::RgbaImage>) {
        if !self.stack.insert(entry.id.clone()) {
            return;
        }
        let image = image.and_then(|image| crate::capture::bitmap(g, image).ok());
        self.cards.push(Card {
            entry,
            image,
            clipboard: false,
            busy: false,
            saved: false,
        });
    }

    fn remove(&mut self, index: usize) {
        if index < self.cards.len() {
            let card = self.cards.remove(index);
            self.stack.remove(&card.entry.id);
        }
        self.hovered = None;
        self.hot = None;
        self.pressed = None;
    }

    fn content_height(&self) -> f32 {
        self.stack.content_height() as f32
    }

    /// Content offset that keeps the anchored end visible.
    fn offset(&self) -> f32 {
        if self.top {
            0.
        } else {
            self.height - self.content_height()
        }
    }

    fn card_rect(&self, index: usize) -> Option<Rect> {
        let layout = self.stack.card_layout(index, self.top)?;
        let y = layout.y as f32 + self.offset();
        Some(rect(PAD, y, PAD + CARD_W, y + CARD_H))
    }

    pub fn collapsed(&self) -> bool {
        self.stack.is_collapsed()
    }

    /// Shipping shows the toolbar (Clear all and Minimize) only for two or
    /// more cards, and never on the pile.
    fn toolbar(&self) -> bool {
        !self.collapsed() && self.cards.len() >= 2 && self.progress() <= 0.
    }

    /// The front card of the collapsed pile, the pile's hit target.
    fn front_rect(&self) -> Option<Rect> {
        self.card_rect(self.cards.len().checked_sub(1)?)
    }

    /// Minimize sits beside Clear all, `gap: var(--s-1)`; the right anchor's
    /// `row-reverse` puts it inside Clear all.
    fn minimize_rect(&self) -> Rect {
        let clear = self.clear_rect();
        let step = CONTROL + 2.;
        let x = if self.right {
            clear.left - step
        } else {
            clear.left + step
        };
        rect(x, clear.top, x + CONTROL, clear.bottom)
    }

    fn clear_rect(&self) -> Rect {
        // 8 DIPs from the nearest card, inside the control gutter.
        let y = if self.top {
            GUTTER - 8. - CONTROL
        } else {
            self.content_height() - GUTTER + 8.
        } + self.offset();
        let x = if self.right {
            PAD + CARD_W - CONTROL
        } else {
            PAD
        };
        rect(x, y, x + CONTROL, y + CONTROL)
    }

    /// Close and Delete (top-left, top-right on the right), then Edit in the
    /// opposite corner. Unsaved captures show Delete only, which dismisses.
    fn top_controls(&self, index: usize, card: Rect) -> Vec<(Hit, Rect)> {
        let saved = self.cards[index].entry.saved_path.is_some();
        let y = card.top + 8.;
        let at = |x: f32| rect(x, y, x + CONTROL, y + CONTROL);
        let mut controls = Vec::new();
        let order: Vec<Hit> = if saved {
            vec![Hit::Close(index), Hit::Delete(index)]
        } else {
            vec![Hit::Delete(index)]
        };
        let gap = 6.;
        let total = order.len() as f32 * CONTROL + (order.len() as f32 - 1.) * gap;
        let start = if self.right {
            card.right - 8. - total
        } else {
            card.left + 8.
        };
        for (i, hit) in order.into_iter().enumerate() {
            controls.push((hit, at(start + i as f32 * (CONTROL + gap))));
        }
        let edit = if self.right {
            card.left + 8.
        } else {
            card.right - 8. - CONTROL
        };
        controls.push((Hit::Edit(index), at(edit)));
        controls
    }

    /// Copy (unless the clipboard already holds this card), then the
    /// accent Save file / Show in Folder, centred.
    fn main_actions(&self, index: usize, card: Rect) -> Vec<(Hit, Rect)> {
        let mut hits = Vec::new();
        if !self.cards[index].clipboard {
            hits.push(Hit::Copy(index));
        }
        hits.push(Hit::Save(index));
        let gap = 6.;
        let total = hits.len() as f32 * ACTION_H + (hits.len() as f32 - 1.) * gap;
        let x = (card.left + card.right - ACTION_W) / 2.;
        let mut y = (card.top + card.bottom - total) / 2.;
        hits.into_iter()
            .map(|hit| {
                let r = rect(x, y, x + ACTION_W, y + ACTION_H);
                y += ACTION_H + gap;
                (hit, r)
            })
            .collect()
    }

    /// The pointer is over a card, the toolbar or the pile, or the stack is
    /// mid-fly; elsewhere clicks reach the window underneath, like shipping's
    /// polled `set_thumbnail_ignore_cursor_events`.
    pub fn solid(&self, x: f32, y: f32) -> bool {
        let progress = self.progress();
        (progress > 0. && progress < 1.) || self.pressed.is_some() || self.hit(x, y).is_some()
    }

    pub fn hit(&self, x: f32, y: f32) -> Option<Hit> {
        let inside = |r: &Rect| x >= r.left && x < r.right && y >= r.top && y < r.bottom;
        // Nothing takes the pointer mid-fly.
        let progress = self.progress();
        if progress > 0. && progress < 1. {
            return None;
        }
        if self.collapsed() {
            return self.front_rect().filter(|r| inside(r)).map(|_| Hit::Expand);
        }
        if self.toolbar() {
            if inside(&self.clear_rect()) {
                return Some(Hit::ClearAll);
            }
            if inside(&self.minimize_rect()) {
                return Some(Hit::Minimize);
            }
        }
        for index in 0..self.cards.len() {
            let Some(card) = self.card_rect(index) else {
                continue;
            };
            if !inside(&card) {
                continue;
            }
            // Controls are only targets while the card shows them.
            if self.hovered == Some(index) {
                for (hit, r) in self
                    .top_controls(index, card)
                    .into_iter()
                    .chain(self.main_actions(index, card))
                {
                    if inside(&r) {
                        return Some(hit);
                    }
                }
            }
            return Some(Hit::Card(index));
        }
        None
    }

    /// Pointer at `(x, y)` DIPs, or `None` when it left; true when the look
    /// changed.
    pub fn pointer(&mut self, at: Option<(f32, f32)>) -> bool {
        if self.collapsed() {
            let before = (self.pile_hovered, self.hot);
            self.hovered = None;
            self.hot = at.and_then(|(x, y)| self.hit(x, y));
            self.pile_hovered = self.hot == Some(Hit::Expand);
            if self.pile_hovered != before.0 {
                self.set_fan(self.pile_hovered);
            }
            return (self.pile_hovered, self.hot) != before;
        }
        self.pile_hovered = false;
        // Hover follows the card under the pointer, then targets its controls.
        let card = at.and_then(|(x, y)| {
            (0..self.cards.len()).find(|&i| {
                self.card_rect(i)
                    .is_some_and(|r| x >= r.left && x < r.right && y >= r.top && y < r.bottom)
            })
        });
        let before = (self.hovered, self.hot);
        if self.hovered != card {
            if let Some(old) = self.hovered.and_then(|i| self.cards.get_mut(i)) {
                old.saved = false;
            }
            self.hovered = card;
        }
        self.hot = at.and_then(|(x, y)| self.hit(x, y));
        (self.hovered, self.hot) != before
    }

    /// Automation key of a button, unique per card position.
    fn uia_key(hit: Hit) -> u32 {
        let base = |i: usize| 100 * (i as u32 + 1);
        match hit {
            Hit::ClearAll => 1,
            Hit::Minimize => 2,
            Hit::Expand => 3,
            Hit::Card(i) => base(i),
            Hit::Close(i) => base(i) + 1,
            Hit::Delete(i) => base(i) + 2,
            Hit::Edit(i) => base(i) + 3,
            Hit::Copy(i) => base(i) + 4,
            Hit::Save(i) => base(i) + 5,
        }
    }

    fn uia_hit(key: u32) -> Option<Hit> {
        match key {
            1 => return Some(Hit::ClearAll),
            2 => return Some(Hit::Minimize),
            3 => return Some(Hit::Expand),
            _ => {}
        }
        let i = (key / 100).checked_sub(1)? as usize;
        Some(match key % 100 {
            0 => Hit::Card(i),
            1 => Hit::Close(i),
            2 => Hit::Delete(i),
            3 => Hit::Edit(i),
            4 => Hit::Copy(i),
            5 => Hit::Save(i),
            _ => return None,
        })
    }

    /// The stack's parts in shipping's DOM order: each `<article>` with its
    /// Close/Delete, Edit, Copy and Save file / Show in Folder buttons, its
    /// size line and the `role="status"` clipboard confirmation, then Clear
    /// all previews.
    pub fn uia_items(&self) -> Vec<Item> {
        let mut items = Vec::new();
        // The pile is one button; its cards leave the tree.
        if self.collapsed() {
            if let Some(front) = self.front_rect() {
                let count = self.cards.len();
                let name = if count == 1 {
                    "Expand preview".to_owned()
                } else {
                    format!("Expand {count} previews")
                };
                items.push(Item::new(
                    Self::uia_key(Hit::Expand),
                    Role::Button,
                    name,
                    front,
                ));
            }
            return items;
        }
        for (index, card) in self.cards.iter().enumerate() {
            let Some(r) = self.card_rect(index) else {
                continue;
            };
            let group = Self::uia_key(Hit::Card(index));
            items.push(Item::new(group, Role::Group, "Screenshot preview", r));
            let mut child = |hit: Hit, role: Role, name: &str, rect: Rect| {
                let mut item = Item::new(Self::uia_key(hit), role, name, rect);
                item.parent = Some(group);
                item.enabled = !card.busy;
                items.push(item);
            };
            for (hit, rect) in self
                .top_controls(index, r)
                .into_iter()
                .chain(self.main_actions(index, r))
            {
                let name = match hit {
                    Hit::Close(_) => "Close",
                    Hit::Delete(_) => "Delete",
                    Hit::Edit(_) => preview_chrome::EDIT_LABEL,
                    Hit::Copy(_) => "Copy",
                    _ if card.saved => "Saved",
                    _ if card.entry.saved_path.is_some() => "Show in Folder",
                    _ => "Save file",
                };
                child(hit, Role::Button, name, rect);
            }
            let bar = rect(r.left + 8., r.bottom - 30., r.right - 8., r.bottom - 8.);
            let meta =
                preview::card_metadata(card.entry.width, card.entry.height, card.entry.size_bytes);
            let mut text = Item::new(group + 6, Role::Text, meta, bar);
            text.parent = Some(group);
            items.push(text);
            if card.clipboard {
                let mut status = Item::new(group + 7, Role::Status, "Copied to clipboard", bar);
                status.parent = Some(group);
                items.push(status);
            }
        }
        if self.toolbar() {
            items.push(Item::new(
                Self::uia_key(Hit::ClearAll),
                Role::Button,
                preview::STACK_CLEAR_ALL_LABEL,
                self.clear_rect(),
            ));
            items.push(Item::new(
                Self::uia_key(Hit::Minimize),
                Role::Button,
                preview::STACK_MINIMIZE_LABEL,
                self.minimize_rect(),
            ));
        }
        items
    }

    pub fn paint(&self, dc: &ID2D1DeviceContext, g: &Gfx, t: &Tokens) -> Result<()> {
        if self.collapsed() || self.progress() > 0. {
            return self.paint_pile(dc, g, t);
        }
        for index in 0..self.cards.len() {
            if let Some(card) = self.card_rect(index) {
                self.paint_card(dc, g, t, index, card)?;
            }
        }
        if self.toolbar() {
            for (hit, icon, side) in [
                (Hit::ClearAll, "close", 16.),
                (Hit::Minimize, "preview-stack", 24.),
            ] {
                let r = if hit == Hit::ClearAll {
                    self.clear_rect()
                } else {
                    self.minimize_rect()
                };
                let hot = self.hot == Some(hit);
                // `.thumbnail-stack-control`
                gfx::shadow(dc, r, t.n("r-md"), t.shadow("thumbnail-card-shadow"))?;
                let fill = if hot {
                    "glass-raised-solid"
                } else {
                    "glass-strong-solid"
                };
                gfx::fill_round(dc, r, t.n("r-md"), t.color(fill))?;
                let edge = if hot {
                    "glass-border-strong"
                } else {
                    "glass-border"
                };
                gfx::border(dc, r, t.n("r-md"), 1., t.color(edge))?;
                // `close` is a 24-unit icon at 16 px; `preview-stack` a
                // 16-unit one, so 24 here draws it at 16 px too.
                let inset = (CONTROL - 16.) / 2.;
                gfx::icon(
                    dc,
                    &g.factory,
                    icon,
                    &[],
                    (r.left + inset, r.top + inset, side),
                    if side > 16. { 1.4 * 1.5 } else { 1.8 },
                    t.color("glass-text"),
                )?;
            }
        }
        Ok(())
    }

    /// The collapsed pile, and the fly between it and the list: oldest
    /// card first so the newest paints on top, each card moving from its list
    /// slot to its shared `collapsed_card_pose` (blended toward the fanned
    /// pose by its staggered fan progress) under its `collapsed_dim_opacity`
    /// shade, like the GTK host. The pose is drawn as its flattened affine;
    /// the keystone and rear-card media blur are not.
    fn paint_pile(&self, dc: &ID2D1DeviceContext, g: &Gfx, t: &Tokens) -> Result<()> {
        let count = self.cards.len();
        let progress = self.progress();
        let now = now_ms();
        let instant = self.instant();
        let fan_tween = self.motion.map(|(_, fan)| fan);
        let top = self.top;
        let gravity = if top { -1. } else { 1. };
        let content = lerp(
            preview::stack_height(count),
            preview::collapsed_frame_height(count),
            progress,
        );
        let offset = if top {
            0.
        } else {
            f64::from(self.height) - content
        };
        let front = preview::collapsed_padding(count);
        let half = (f64::from(CARD_W) / 2., f64::from(CARD_H) / 2.);
        let local = rect(-CARD_W / 2., -CARD_H / 2., CARD_W / 2., CARD_H / 2.);
        let mut base = Matrix3x2::identity();
        unsafe { dc.GetTransform(&mut base) };
        for index in 0..count {
            let depth = count - 1 - index;
            let id = &self.cards[index].entry.id;
            let expanded =
                preview::card_layout_in(count, index, false, top, false).map_or(0., |l| l.y);
            let rest = preview::collapsed_card_pose(id, depth, false, gravity, top);
            let fanned = preview::collapsed_card_pose(id, depth, true, gravity, top);
            let fan = match &fan_tween {
                Some(tween) => self.fan.progress(id, depth, now, tween, instant).0,
                None => f64::from(u8::from(self.fan.open())),
            };
            let pose = mix(rest, fanned, fan);
            let cx = f64::from(PAD) + half.0 + pose.dx * progress;
            let cy = offset + lerp(expanded + half.1, front + half.1 + pose.dy, progress);
            let (sin, cos) = ((pose.rotation_deg * progress) as f32)
                .to_radians()
                .sin_cos();
            let (sx, sy) = (
                lerp(1., pose.scale_x, progress) as f32,
                lerp(1., pose.scale_y, progress) as f32,
            );
            // Row vectors: scale, rotate clockwise, then move to the centre.
            let m = Matrix3x2 {
                M11: sx * cos,
                M12: sx * sin,
                M21: -sy * sin,
                M22: sy * cos,
                M31: cx as f32,
                M32: cy as f32,
            } * base;
            unsafe { dc.SetTransform(&m) };
            let result = glow(dc, t, local, (fan * progress) as f32)
                .and_then(|()| self.paint_card(dc, g, t, index, local))
                .and_then(|()| {
                    let dim = (preview::collapsed_dim_opacity(depth) * progress * (1. - 0.35 * fan))
                        as f32;
                    if dim > 0. {
                        let mut shade = t.color("glass-strong-solid");
                        shade.a *= dim;
                        gfx::fill_round(dc, local, t.n("thumbnail-card-radius"), shade)?;
                    }
                    Ok(())
                });
            unsafe { dc.SetTransform(&base) };
            result?;
        }
        Ok(())
    }

    fn paint_card(
        &self,
        dc: &ID2D1DeviceContext,
        g: &Gfx,
        t: &Tokens,
        index: usize,
        r: Rect,
    ) -> Result<()> {
        let card = &self.cards[index];
        let hovered = self.hovered == Some(index);
        let radius = t.n("thumbnail-card-radius");
        // `--thumbnail-card-shadow` and the 1 px white ring.
        gfx::shadow(dc, r, radius, t.shadow("thumbnail-card-shadow"))?;
        let white = |a: f32| D2D1_COLOR_F {
            r: 1.,
            g: 1.,
            b: 1.,
            a,
        };
        gfx::fill_round(dc, r, radius, t.color("glass-strong-solid"))?;
        if let Some(image) = &card.image {
            cover(dc, image, (r, radius), hovered)?;
        }
        if hovered {
            // `brightness(.5)` on the opaque image.
            gfx::fill_round(
                dc,
                r,
                radius,
                D2D1_COLOR_F {
                    r: 0.,
                    g: 0.,
                    b: 0.,
                    a: preview_chrome::HOVER_MEDIA_BRIGHTNESS as f32,
                },
            )?;
        }
        gfx::ring(dc, r, radius, 0., 1., white(0.08))?;

        // `.thumbnail-bottom-bar`: the meta chip (hidden on hover) and the
        // clipboard confirmation.
        let bottom = r.bottom - 8.;
        if !hovered {
            let meta =
                preview::card_metadata(card.entry.width, card.entry.height, card.entry.size_bytes);
            let text = g.text_sized(t, &meta, t.n("text-2xs"), "weight-regular", None)?;
            let h = t.n("text-2xs") * 1.3 + 6.;
            let chip = rect(
                r.left + 8.,
                bottom - h,
                r.left + 8. + text.width + 2. * t.n("s-3"),
                bottom,
            );
            gfx::fill_round(
                dc,
                chip,
                t.n("r-xs"),
                D2D1_COLOR_F {
                    r: 10. / 255.,
                    g: 10. / 255.,
                    b: 14. / 255.,
                    a: 0.66,
                },
            )?;
            gfx::text(
                dc,
                &text,
                chip.left + t.n("s-3"),
                chip.top + (h - text.height) / 2.,
                t.color("glass-text"),
            )?;
        }
        if card.clipboard {
            let label = g.text_sized(
                t,
                "Copied to clipboard",
                t.n("text-2xs"),
                "weight-semibold",
                None,
            )?;
            let h = t.n("text-2xs") * 1.3 + 8.;
            let w = 2. * t.n("s-4") + 12. + t.n("s-2") + label.width;
            let chip = rect(r.right - 8. - w, bottom - h, r.right - 8., bottom);
            gfx::fill_round(
                dc,
                chip,
                h / 2.,
                D2D1_COLOR_F {
                    r: 10. / 255.,
                    g: 22. / 255.,
                    b: 15. / 255.,
                    a: 0.9,
                },
            )?;
            gfx::border(dc, chip, h / 2., 1., t.color_alpha("positive", 0.55))?;
            gfx::icon(
                dc,
                &g.factory,
                "check",
                &[],
                (chip.left + t.n("s-4"), chip.top + (h - 12.) / 2., 12.),
                2.4,
                t.color("positive-text"),
            )?;
            gfx::text(
                dc,
                &label,
                chip.left + t.n("s-4") + 12. + t.n("s-2"),
                chip.top + (h - label.height) / 2.,
                D2D1_COLOR_F {
                    r: 234. / 255.,
                    g: 1.,
                    b: 240. / 255.,
                    a: 1.,
                },
            )?;
        }
        if !hovered {
            return Ok(());
        }

        for (hit, b) in self.top_controls(index, r) {
            let hot = self.hot == Some(hit) && !card.busy;
            let lift = if hot { -1. } else { 0. };
            let b = rect(b.left, b.top + lift, b.right, b.bottom + lift);
            let delete = matches!(hit, Hit::Delete(_));
            let fill = if hot && delete {
                t.color("theme-signal")
            } else if hot {
                t.color("glass-raised")
            } else {
                t.color("glass-strong")
            };
            gfx::shadow(dc, b, t.n("r-md"), t.shadow("shadow-sm"))?;
            gfx::fill_round(dc, b, t.n("r-md"), fill)?;
            gfx::border(dc, b, t.n("r-md"), 1., t.color("glass-border"))?;
            let (name, ink) = match hit {
                Hit::Close(_) => ("close", t.color("glass-text")),
                Hit::Delete(_) if hot => ("trash", white(1.)),
                Hit::Delete(_) => ("trash", t.color("theme-signal-text")),
                _ => ("edit", t.color("glass-text")),
            };
            gfx::icon(
                dc,
                &g.factory,
                name,
                &[],
                (b.left + 6., b.top + 6., 16.),
                1.8,
                ink,
            )?;
        }
        for (hit, b) in self.main_actions(index, r) {
            let hot = self.hot == Some(hit);
            let feedback = preview_chrome::main_action_feedback(
                hot && !card.busy,
                self.pressed == Some(hit) && hot,
                !card.busy,
            );
            let lift = feedback.translate_y as f32;
            let b = rect(b.left, b.top + lift, b.right, b.bottom + lift);
            let primary = matches!(hit, Hit::Save(_));
            let (mut fill, mut ink) = if primary {
                (t.color("theme-accent"), t.color("theme-accent-ink"))
            } else {
                (t.color("glass-strong"), t.color("glass-text"))
            };
            let k = feedback.brightness as f32;
            fill.r = (fill.r * k).min(1.);
            fill.g = (fill.g * k).min(1.);
            fill.b = (fill.b * k).min(1.);
            fill.a *= feedback.opacity as f32;
            ink.a *= feedback.opacity as f32;
            gfx::shadow(dc, b, t.n("r-md"), t.shadow("shadow-md"))?;
            gfx::fill_round(dc, b, t.n("r-md"), fill)?;
            if !primary {
                gfx::border(dc, b, t.n("r-md"), 1., t.color("glass-border-strong"))?;
            }
            let (icon, label) = match hit {
                Hit::Copy(_) => ("copy", "Copy"),
                _ if card.saved => ("check", "Saved"),
                _ if card.entry.saved_path.is_some() => ("folder", "Show in Folder"),
                _ => ("save", "Save file"),
            };
            let text = g.text_sized(t, label, t.n("text-xs"), "weight-semibold", None)?;
            let content = 16. + t.n("s-3") + text.width;
            let x = (b.left + b.right - content) / 2.;
            let cy = (b.top + b.bottom) / 2.;
            gfx::icon(dc, &g.factory, icon, &[], (x, cy - 8., 16.), 1.8, ink)?;
            gfx::text(dc, &text, x + 16. + t.n("s-3"), cy - text.height / 2., ink)?;
        }
        Ok(())
    }
}

/// The hovered pile's accent glow behind each card, eased in with its fan
/// (the hovered `.thumbnail-stack-minimized > .thumbnail-card` box-shadow
/// `0 0 0 1px` accent at 0.55 and `0 0 22px` accent at 0.28, after the
/// card shadow that `paint_card` draws).
fn glow(dc: &ID2D1DeviceContext, t: &Tokens, card: Rect, amount: f32) -> Result<()> {
    if amount <= 0. {
        return Ok(());
    }
    let accent = t.color("theme-accent");
    let layer = |blur: f32, spread: f32, alpha: f32| crate::tokens::Shadow {
        x: 0.,
        y: 0.,
        blur,
        spread,
        color: [accent.r, accent.g, accent.b, accent.a * alpha * amount],
    };
    gfx::shadow(
        dc,
        card,
        t.n("thumbnail-card-radius"),
        &[layer(0., 1., 0.55), layer(22., 0., 0.28)],
    )
}

/// `--scene capture --tool previews`: shipping's `?view=thumbnail&mock=1`
/// pair, a saved 1600×1000 capture on the clipboard above an unsaved
/// 1200×900 one, both showing `image`; `hovered` hovers the first.
pub fn fixture(g: &Gfx, image: &image::RgbaImage, select: Option<&str>) -> Stack {
    let mut stack = Stack::new(captures_settings::MiniPreviewPlacement::BottomLeft);
    let pile = matches!(select, Some("pile" | "pile-hover"));
    let mut cards = vec![
        ("first", (1600, 1000, 842_000), true),
        ("second", (1200, 900, 432_000), false),
    ];
    if pile {
        cards.insert(0, ("zeroth", (1920, 1080, 1_204_000), false));
    }
    for (id, (w, h, size), saved) in cards {
        let entry = serde_json::from_value(serde_json::json!({
            "id": id,
            "preview_url": "",
            "full_url": "",
            "width": w,
            "height": h,
            "size_bytes": size,
            "created_at": "2026-11-09T00:00:00Z",
            "saved_path": saved.then_some("C:\\Captures\\first.png"),
        }))
        .expect("fixture entry");
        stack.push(g, entry, Some(image));
    }
    let last = stack.cards.len() - 1;
    stack.cards[last - 1].clipboard = !pile;
    stack.stack.set_collapsed(pile);
    stack.height = stack.content_height();
    match select {
        Some("pile-hover") => {
            let front = stack.front_rect().expect("front card");
            stack.pointer(Some((front.left + 40., front.top + 40.)));
        }
        Some("pile") | None => {}
        Some(_) => {
            stack.pointer(Some((150., 90.)));
        }
    }
    stack
}

/// `object-fit: cover` into the rounded `r`; hovered cards scale by 1.015
/// and blur by 2 px (`.thumbnail-card:hover img`). Without the blur effect
/// (Wine) the image is drawn sharp.
fn cover(
    dc: &ID2D1DeviceContext,
    image: &ID2D1Bitmap1,
    (r, radius): (Rect, f32),
    hovered: bool,
) -> Result<()> {
    let size = unsafe { image.GetSize() };
    if size.width <= 0. || size.height <= 0. {
        return Ok(());
    }
    let (w, h) = (r.right - r.left, r.bottom - r.top);
    let mut k = (w / size.width).max(h / size.height);
    if hovered {
        k *= preview_chrome::HOVER_MEDIA_SCALE as f32;
    }
    let placed = Matrix3x2 {
        M11: k,
        M12: 0.,
        M21: 0.,
        M22: k,
        M31: r.left + (w - size.width * k) / 2.,
        M32: r.top + (h - size.height * k) / 2.,
    };
    let shape = D2D1_ROUNDED_RECT {
        rect: r,
        radiusX: radius,
        radiusY: radius,
    };
    unsafe {
        let blurred = hovered.then(|| -> Result<ID2D1Brush> {
            let effect = dc.CreateEffect(&CLSID_D2D1GaussianBlur)?;
            effect.SetInput(0, image, true);
            let deviation = preview_chrome::HOVER_MEDIA_BLUR as f32 / k;
            effect.SetValue(
                D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION.0 as u32,
                D2D1_PROPERTY_TYPE_FLOAT,
                &deviation.to_le_bytes(),
            )?;
            effect.SetValue(
                D2D1_GAUSSIANBLUR_PROP_BORDER_MODE.0 as u32,
                D2D1_PROPERTY_TYPE_ENUM,
                &(D2D1_BORDER_MODE_HARD.0 as u32).to_le_bytes(),
            )?;
            let brush = dc.CreateImageBrush(
                &effect.cast::<ID2D1Image>()?,
                &D2D1_IMAGE_BRUSH_PROPERTIES {
                    sourceRectangle: rect(0., 0., size.width, size.height),
                    extendModeX: D2D1_EXTEND_MODE_CLAMP,
                    extendModeY: D2D1_EXTEND_MODE_CLAMP,
                    interpolationMode: D2D1_INTERPOLATION_MODE_LINEAR,
                },
                Some(&D2D1_BRUSH_PROPERTIES {
                    opacity: 1.,
                    transform: placed,
                }),
            )?;
            brush.cast()
        });
        let brush = match blurred {
            Some(Ok(brush)) => brush,
            _ => {
                let brush = dc.CreateBitmapBrush(
                    image,
                    Some(&D2D1_BITMAP_BRUSH_PROPERTIES1 {
                        extendModeX: D2D1_EXTEND_MODE_CLAMP,
                        extendModeY: D2D1_EXTEND_MODE_CLAMP,
                        interpolationMode: D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC,
                    }),
                    None,
                )?;
                brush.SetTransform(&placed);
                brush.cast()?
            }
        };
        dc.FillRoundedRectangle(&shape, &brush);
    }
    Ok(())
}

enum Done {
    Saved(String, std::result::Result<String, String>),
    Deleted(String, std::result::Result<(), String>),
}

struct Previews {
    hwnd: HWND,
    stack: Stack,
    g: Gfx,
    tokens: Tokens,
    scale: f32,
    px: (u32, u32),
    target: Option<(
        IDCompositionTarget,
        IDCompositionVisual2,
        IDCompositionSurface,
    )>,
    options: crate::Options,
    root: PathBuf,
    sender: Sender<Done>,
    results: Receiver<Done>,
    tracking: bool,
    /// `WS_EX_TRANSPARENT` is set: clicks pass through and `POLL_TIMER` runs.
    passthrough: bool,
    /// The display the stack sits on, chosen when a card arrives.
    monitor: windows::Win32::Graphics::Gdi::HMONITOR,
    /// Hidden while New Capture or a countdown is open.
    suppressed: bool,
}

thread_local! {
    static PREVIEWS: RefCell<Option<Previews>> = const { RefCell::new(None) };
}

/// The cards' History artifacts in stack order, for an update restart.
pub fn ids() -> Vec<String> {
    PREVIEWS.with(|p| {
        p.try_borrow()
            .ok()
            .and_then(|p| p.as_ref().map(|p| p.stack.stack.ids().to_vec()))
            .unwrap_or_default()
    })
}

/// History Restore: bring a screenshot back as a card.
pub fn restore(options: &crate::Options, entry: &HistoryEntry) {
    let root = options
        .history_root
        .clone()
        .unwrap_or_else(captures_app::default_history_root);
    let preview = root
        .join(&entry.id)
        .join(captures_history::HISTORY_PREVIEW_FILE);
    show(options, entry, &preview, false);
}

/// Add a saved screenshot's card. `clipboard` is whether auto-copy put it
/// on the clipboard. Does nothing when mini previews are off.
pub fn show(
    options: &crate::Options,
    entry: &HistoryEntry,
    preview: &std::path::Path,
    clipboard: bool,
) {
    let settings = captures_settings::load(&options.settings_file).unwrap_or_default();
    if !settings.show_mini_previews {
        return;
    }
    let before = panel_uia::watch(panel_uia::Window::Previews);
    let result = PREVIEWS.with(|p| -> Result<()> {
        let mut p = p.borrow_mut();
        if p.is_none() {
            *p = Some(Previews::new(options, &settings)?);
        }
        let previews = p.as_mut().ok_or_else(Error::empty)?;
        let image = image::open(preview).ok().map(|image| image.into_rgba8());
        let g = previews.g.clone();
        previews.stack.push(&g, entry.clone(), image.as_ref());
        if clipboard {
            previews.set_clipboard(&entry.id);
        }
        previews.place_on_pointer_monitor()?;
        previews.render()
    });
    if let Err(error) = result {
        eprintln!("Could not show the mini preview: {error}");
    }
    panel_uia::raise(panel_uia::Window::Previews, before);
}

/// The stack's parts for UI Automation, while it exists.
pub fn uia_snapshot() -> Option<panel_uia::Snapshot> {
    PREVIEWS.with(|p| {
        let p = p.try_borrow().ok()?;
        let previews = p.as_ref()?;
        Some(panel_uia::Snapshot {
            hwnd: previews.hwnd,
            scale: previews.scale,
            size: (CARD_W + 2. * PAD, previews.stack.height),
            name: "Captures Previews",
            focusable: false,
            items: previews.stack.uia_items(),
        })
    })
}

/// Invoke a card button or Clear all through the click path.
pub fn uia_act(key: u32, act: panel_uia::Act) -> Option<()> {
    PREVIEWS.with(|p| {
        let mut p = p.try_borrow_mut().ok()?;
        let previews = p.as_mut()?;
        if let (Some(hit), panel_uia::Act::Invoke) = (Stack::uia_hit(key), act) {
            previews.run(hit);
        }
        Some(())
    })
}

/// Hide the stack while capture UI is open, unless shipping's Include mini
/// previews in captures keeps it.
pub fn set_suppressed(suppressed: bool) {
    PREVIEWS.with(|p| {
        let Ok(mut p) = p.try_borrow_mut() else {
            return;
        };
        let Some(previews) = p.as_mut() else { return };
        if previews.suppressed == suppressed {
            return;
        }
        previews.suppressed = suppressed;
        previews.sync_visibility();
    });
}

impl Previews {
    fn new(options: &crate::Options, settings: &captures_settings::AppSettings) -> Result<Self> {
        let tokens = crate::appearance::tokens(options.appearance.as_deref(), settings);
        let g = match crate::host::with_host(|h| h.g.clone()) {
            Some(g) => g,
            None => Gfx::new(&tokens, true)?,
        };
        let hwnd = unsafe { window()? };
        let (sender, results) = channel();
        Ok(Self {
            hwnd,
            stack: {
                let mut stack = Stack::new(settings.mini_preview_placement);
                stack.motion = Transition::PreviewStackFly
                    .resolve(&tokens)
                    .zip(Transition::PreviewStackFan.resolve(&tokens));
                stack.reduced = crate::appearance::reduced_motion();
                stack
            },
            g,
            tokens,
            scale: 1.,
            px: (0, 0),
            target: None,
            options: options.clone(),
            root: options
                .history_root
                .clone()
                .unwrap_or_else(captures_app::default_history_root),
            sender,
            results,
            tracking: false,
            passthrough: false,
            monitor: Default::default(),
            suppressed: false,
        })
    }

    fn set_clipboard(&mut self, id: &str) {
        for card in &mut self.stack.cards {
            card.clipboard = card.entry.id == id;
        }
    }

    /// Move the stack to the display under the pointer, where the capture
    /// was taken.
    fn place_on_pointer_monitor(&mut self) -> Result<()> {
        unsafe {
            let mut cursor = POINT::default();
            let _ = GetCursorPos(&mut cursor);
            self.monitor = MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST);
        }
        self.place()
    }

    /// Size and place the window with `thumbnail_geometry` on its display.
    fn place(&mut self) -> Result<()> {
        unsafe {
            let monitor = self.monitor;
            let mut info = MONITORINFO {
                cbSize: size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            let _ = GetMonitorInfoW(monitor, &mut info);
            let (mut dx, mut dy) = (96u32, 96u32);
            let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
            let scale = dx.max(96) as f64 / 96.;
            let (work, full) = (info.rcWork, info.rcMonitor);
            let bounds = preview::ThumbnailMonitorBounds {
                work_x: work.left,
                work_y: work.top,
                work_width: (work.right - work.left) as u32,
                work_height: (work.bottom - work.top) as u32,
                full_x: full.left,
                full_y: full.top,
                full_width: (full.right - full.left) as u32,
                full_height: (full.bottom - full.top) as u32,
                scale_factor: scale,
            };
            let placement = captures_settings::load(&self.options.settings_file)
                .unwrap_or_default()
                .mini_preview_placement;
            let geometry = preview::thumbnail_geometry(
                bounds,
                self.stack.cards.len(),
                self.stack.collapsed(),
                None,
                placement,
            );
            self.stack.top = geometry.anchor.is_top();
            self.stack.right = placement.is_right();
            self.stack.height = geometry.height as f32;
            self.scale = scale as f32;
            let px = (
                (preview::THUMBNAIL_WIDTH * scale).round() as u32,
                (geometry.height * scale).round() as u32,
            );
            SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                (geometry.x * scale).round() as i32,
                (geometry.y * scale).round() as i32,
                px.0 as i32,
                px.1 as i32,
                SWP_NOACTIVATE,
            )?;
            if px != self.px || self.target.is_none() {
                let dcomp = self.g.dcomp.clone().ok_or_else(Error::empty)?;
                let surface = self.g.surface(px.0, px.1)?;
                let (target, visual) = match self.target.take() {
                    Some((target, visual, _)) => (target, visual),
                    None => {
                        let target = dcomp.CreateTargetForHwnd(self.hwnd, true)?;
                        let visual = dcomp.CreateVisual()?;
                        target.SetRoot(&visual)?;
                        (target, visual)
                    }
                };
                visual.SetContent(&surface)?;
                self.target = Some((target, visual, surface));
                self.px = px;
            }
        }
        self.sync_visibility();
        Ok(())
    }

    fn sync_visibility(&self) {
        let settings = captures_settings::load(&self.options.settings_file).unwrap_or_default();
        let visible = preview::stack_should_be_visible(
            self.stack.cards.len(),
            self.suppressed,
            settings.show_mini_previews,
            settings.include_mini_previews_in_captures,
        );
        unsafe {
            let _ = ShowWindow(self.hwnd, if visible { SW_SHOWNOACTIVATE } else { SW_HIDE });
        }
    }

    fn render(&mut self) -> Result<()> {
        let Some((_, _, surface)) = &self.target else {
            return Ok(());
        };
        let (stack, g, t) = (&self.stack, &self.g, &self.tokens);
        gfx::draw_surface(surface, self.px, (self.scale, self.scale), 0., |dc| {
            stack.paint(dc, g, t)
        })?;
        unsafe { self.g.dcomp.as_ref().ok_or_else(Error::empty)?.Commit() }
    }

    /// The stack changed size: refit, or hide once it is empty.
    fn relayout(&mut self) -> Result<()> {
        if self.stack.cards.is_empty() {
            self.sync_visibility();
            return Ok(());
        }
        self.place()?;
        self.render()
    }

    fn run(&mut self, hit: Hit) {
        if matches!(hit, Hit::Minimize | Hit::Expand) {
            self.stack.reduced = crate::appearance::reduced_motion();
            self.stack.set_collapsed(hit == Hit::Minimize);
            // Expanding grows the frame first; collapsing keeps it until the
            // pile lands, then shrinks to the pile (`motion_tick`).
            if hit == Hit::Expand || !self.stack.animating() {
                let _ = self.relayout();
            } else {
                let _ = self.render();
            }
            self.animate();
            return;
        }
        let Some(index) = hit.card() else {
            // Clear all: dismiss every card; History keeps them.
            self.stack.cards.clear();
            self.stack.stack = PreviewStack::default();
            self.stack.hovered = None;
            self.stack.hot = None;
            let _ = self.relayout();
            return;
        };
        let Some(card) = self.stack.cards.get(index) else {
            return;
        };
        if card.busy {
            return;
        }
        let (id, saved_path) = (card.entry.id.clone(), card.entry.saved_path.clone());
        match hit {
            Hit::Close(_) => {
                self.stack.remove(index);
                let _ = self.relayout();
            }
            // Unsaved captures only dismiss; a saved file goes to the
            // Recycle Bin (shipping `trash_artifact`). History keeps both.
            Hit::Delete(_) if saved_path.is_none() => {
                self.stack.remove(index);
                let _ = self.relayout();
            }
            Hit::Delete(_) => {
                self.stack.cards[index].busy = true;
                let (root, sender, hwnd) = (self.root.clone(), self.sender.clone(), self.hwnd);
                let hwnd = hwnd.0 as isize;
                std::thread::spawn(move || {
                    let result = captures_app::execute(captures_app::Request::TrashPreview {
                        root,
                        id: id.clone(),
                        saved_path: saved_path.map(PathBuf::from),
                    })
                    .map(drop)
                    .map_err(|e| e.to_string());
                    let _ = sender.send(Done::Deleted(id, result));
                    post(hwnd);
                });
            }
            Hit::Edit(_) => {
                if let Err(error) = crate::editor::open(&self.options, &id) {
                    eprintln!("couldn't open the editor: {error}");
                }
            }
            Hit::Copy(_) => {
                let path = self
                    .root
                    .join(&id)
                    .join(captures_history::HISTORY_IMAGE_FILE);
                match crate::live::copy_png(&path) {
                    Ok(()) => {
                        self.set_clipboard(&id);
                        self.stack.hot = None;
                    }
                    Err(error) => eprintln!("Could not copy the screenshot: {error}"),
                }
            }
            Hit::Save(_) if saved_path.is_some() => {
                if let Err(error) = crate::history::reveal(saved_path.as_deref().unwrap_or("")) {
                    eprintln!("Could not show the file: {error}");
                }
            }
            Hit::Save(_) => {
                let settings =
                    captures_settings::load(&self.options.settings_file).unwrap_or_default();
                self.stack.cards[index].busy = true;
                let (root, sender) = (self.root.clone(), self.sender.clone());
                let hwnd = self.hwnd.0 as isize;
                std::thread::spawn(move || {
                    let result = captures_app::execute(captures_app::Request::SaveScreenshot {
                        root,
                        id: id.clone(),
                        directory: PathBuf::from(settings.output_directory),
                        format: settings.screenshot_format,
                    });
                    let result = match result {
                        Ok(captures_app::Response::Saved { path, .. }) => {
                            Ok(path.to_string_lossy().into_owned())
                        }
                        Ok(_) => Err("unexpected response".into()),
                        Err(error) => Err(error.to_string()),
                    };
                    let _ = sender.send(Done::Saved(id, result));
                    post(hwnd);
                });
            }
            Hit::Card(_) | Hit::ClearAll | Hit::Minimize | Hit::Expand => {}
        }
        let _ = self.render();
    }

    /// Let clicks through the window, polling the pointer until it is back
    /// over the stack, or take them again.
    fn set_passthrough(&mut self, on: bool) {
        if self.passthrough == on {
            return;
        }
        self.passthrough = on;
        unsafe {
            let style = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE);
            let flag = WS_EX_TRANSPARENT.0 as isize;
            SetWindowLongPtrW(
                self.hwnd,
                GWL_EXSTYLE,
                if on { style | flag } else { style & !flag },
            );
            if on {
                SetTimer(Some(self.hwnd), POLL_TIMER, 50, None);
            } else {
                let _ = KillTimer(Some(self.hwnd), POLL_TIMER);
            }
        }
    }

    /// The pointer in client DIPs, or `None` outside the window.
    fn cursor(&self) -> Option<(f32, f32)> {
        let mut p = POINT::default();
        let mut client = RECT::default();
        unsafe {
            GetCursorPos(&mut p).ok()?;
            let _ = windows::Win32::Graphics::Gdi::ScreenToClient(self.hwnd, &mut p);
            GetClientRect(self.hwnd, &mut client).ok()?;
        }
        (p.x >= client.left && p.x < client.right && p.y >= client.top && p.y < client.bottom)
            .then(|| (p.x as f32 / self.scale, p.y as f32 / self.scale))
    }

    fn poll(&mut self) {
        match self.cursor() {
            Some((x, y)) if !self.stack.solid(x, y) => {}
            // Back over the stack, or gone: Windows sends mouse input again.
            at => {
                self.set_passthrough(false);
                if self.stack.pointer(at) {
                    let _ = self.render();
                    self.animate();
                }
            }
        }
    }

    /// Tick the frame timer while the fly or fan runs.
    fn animate(&self) {
        if self.stack.animating() {
            unsafe {
                SetTimer(Some(self.hwnd), MOTION_TIMER, 16, None);
            }
        }
    }

    fn motion_tick(&mut self) {
        let _ = self.render();
        if self.stack.animating() {
            return;
        }
        unsafe {
            let _ = KillTimer(Some(self.hwnd), MOTION_TIMER);
        }
        // A landed pile shrinks its frame to the pile.
        if self.stack.collapsed() {
            let _ = self.relayout();
        }
    }

    fn finish(&mut self) {
        while let Ok(done) = self.results.try_recv() {
            match done {
                Done::Saved(id, result) => {
                    if let Some(card) = self.stack.cards.iter_mut().find(|c| c.entry.id == id) {
                        card.busy = false;
                        match result {
                            Ok(path) => {
                                card.entry.saved_path = Some(path);
                                card.saved = true;
                            }
                            Err(error) => eprintln!("Could not save the screenshot: {error}"),
                        }
                    }
                    crate::history::with_history(|h| h.reload());
                }
                Done::Deleted(id, result) => {
                    let index = self.stack.cards.iter().position(|c| c.entry.id == id);
                    match (index, result) {
                        (Some(index), Ok(())) => self.stack.remove(index),
                        (Some(index), Err(error)) => {
                            self.stack.cards[index].busy = false;
                            eprintln!("Could not delete the file: {error}");
                        }
                        (None, _) => {}
                    }
                    crate::history::with_history(|h| h.reload());
                }
            }
        }
        let _ = self.relayout();
    }

    fn handle(&mut self, msg: u32, _wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
        let point = |lp: LPARAM, scale: f32| {
            (
                (lp.0 & 0xffff) as i16 as f32 / scale,
                ((lp.0 >> 16) & 0xffff) as i16 as f32 / scale,
            )
        };
        match msg {
            WM_PAINT => unsafe {
                let _ = ValidateRect(Some(self.hwnd), None);
            },
            WM_ERASEBKGND => return Some(LRESULT(1)),
            // Clicks never take focus from the app the user is in.
            WM_MOUSEACTIVATE => return Some(LRESULT(MA_NOACTIVATE as isize)),
            WM_MOUSEMOVE => {
                if !self.tracking {
                    let mut track = TRACKMOUSEEVENT {
                        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: self.hwnd,
                        dwHoverTime: 0,
                    };
                    self.tracking = unsafe { TrackMouseEvent(&mut track) }.is_ok();
                }
                let (x, y) = point(lp, self.scale);
                if self.stack.pointer(Some((x, y))) {
                    let _ = self.render();
                    self.animate();
                }
                if !self.stack.solid(x, y) {
                    self.set_passthrough(true);
                }
            }
            WM_MOUSELEAVE => {
                self.tracking = false;
                self.stack.pressed = None;
                if self.stack.pointer(None) {
                    let _ = self.render();
                    self.animate();
                }
            }
            WM_SETCURSOR => {
                // Windows sends this before the WM_MOUSEMOVE that updates
                // `hot`, so hit-test where the pointer is now.
                let mut p = POINT::default();
                unsafe {
                    let _ = GetCursorPos(&mut p);
                    let _ = windows::Win32::Graphics::Gdi::ScreenToClient(self.hwnd, &mut p);
                }
                let at = (p.x as f32 / self.scale, p.y as f32 / self.scale);
                // The first event over the pile starts its fan.
                if self.stack.pointer(Some(at)) {
                    let _ = self.render();
                    self.animate();
                }
                let control = !matches!(self.stack.hot, None | Some(Hit::Card(_)));
                let cursor = if control { IDC_HAND } else { IDC_ARROW };
                unsafe {
                    SetCursor(LoadCursorW(None, cursor).ok());
                }
                return Some(LRESULT(1));
            }
            WM_LBUTTONDOWN => {
                let (x, y) = point(lp, self.scale);
                self.stack.pressed = self.stack.hit(x, y);
                unsafe { SetCapture(self.hwnd) };
                let _ = self.render();
            }
            WM_LBUTTONUP => {
                let (x, y) = point(lp, self.scale);
                let _ = unsafe { ReleaseCapture() };
                let pressed = self.stack.pressed.take();
                let hit = self.stack.hit(x, y);
                if pressed.is_some() && pressed == hit {
                    self.run(hit.unwrap_or(Hit::ClearAll));
                } else {
                    let _ = self.render();
                }
            }
            WM_APP_PREVIEW_DONE => self.finish(),
            WM_TIMER if _wp.0 == MOTION_TIMER => self.motion_tick(),
            WM_TIMER if _wp.0 == POLL_TIMER => self.poll(),
            _ => return None,
        }
        Some(LRESULT(0))
    }
}

fn post(hwnd: isize) {
    unsafe {
        let _ = PostMessageW(
            Some(HWND(hwnd as *mut _)),
            WM_APP_PREVIEW_DONE,
            WPARAM(0),
            LPARAM(0),
        );
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_GETOBJECT && lp.0 as i32 == UiaRootObjectId {
        return unsafe {
            UiaReturnRawElementProvider(
                hwnd,
                wp,
                lp,
                &panel_uia::node(panel_uia::Window::Previews, None),
            )
        };
    }
    let before = panel_uia::watch(panel_uia::Window::Previews);
    let handled = PREVIEWS.with(|p| {
        let mut p = p.try_borrow_mut().ok()?;
        let previews = p.as_mut()?;
        (previews.hwnd == hwnd)
            .then(|| previews.handle(msg, wp, lp))
            .flatten()
    });
    if handled.is_some() {
        panel_uia::raise(panel_uia::Window::Previews, before);
    }
    handled.unwrap_or_else(|| unsafe { DefWindowProcW(hwnd, msg, wp, lp) })
}

/// The stack's topmost, non-activating tool window (`Captures Previews`).
unsafe fn window() -> Result<HWND> {
    unsafe {
        let instance = GetModuleHandleW(None)?;
        let class = w!("CapturesPreviews");
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            lpszClassName: class,
            ..Default::default()
        });
        let hwnd = CreateWindowExW(
            // Layered so `WS_EX_TRANSPARENT` lets clicks reach other apps.
            WS_EX_TOPMOST
                | WS_EX_TOOLWINDOW
                | WS_EX_NOACTIVATE
                | WS_EX_NOREDIRECTIONBITMAP
                | WS_EX_LAYERED,
            class,
            w!("Captures Previews"),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            Some(instance.into()),
            None,
        )?;
        SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA)?;
        Ok(hwnd)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stack(placement: captures_settings::MiniPreviewPlacement, saved: &[bool]) -> Stack {
        let mut stack = Stack::new(placement);
        for (i, &saved) in saved.iter().enumerate() {
            let id = format!("a{i}");
            stack.stack.insert(id.clone());
            stack.cards.push(Card {
                entry: HistoryEntry {
                    id,
                    saved_path: saved.then(|| format!("C:\\out\\{i}.png")),
                    ..fixture_entry()
                },
                image: None,
                clipboard: false,
                busy: false,
                saved: false,
            });
        }
        stack.height = stack.content_height();
        stack
    }

    fn fixture_entry() -> HistoryEntry {
        serde_json::from_value(serde_json::json!({
            "id": "x",
            "preview_url": "",
            "full_url": "",
            "width": 1600,
            "height": 1000,
            "size_bytes": 842_000,
            "created_at": "2026-11-09T00:00:00Z",
        }))
        .unwrap()
    }

    #[test]
    fn bottom_left_stack_matches_the_shipping_layout() {
        use captures_settings::MiniPreviewPlacement as P;
        // Measured from `?view=thumbnail&mock=1&stage=1`: cards at y 28 and
        // 212 in a 424 DIP window, Clear all 8 DIPs under the last card.
        let mut s = stack(P::BottomLeft, &[true, false]);
        assert_eq!(s.card_rect(0).map(|r| (r.left, r.top)), Some((28., 28.)));
        assert_eq!(s.card_rect(1).map(|r| (r.left, r.top)), Some((28., 212.)));
        assert_eq!((s.clear_rect().left, s.clear_rect().top), (28., 380.));
        // Controls exist only on the hovered card: a saved card offers
        // Close and Delete, an unsaved one Delete (dismiss) only.
        assert_eq!(s.hit(50., 50.), Some(Hit::Card(0)));
        assert!(s.pointer(Some((50., 50.))));
        assert_eq!(s.hit(50., 50.), Some(Hit::Close(0)));
        assert_eq!(s.hit(84., 50.), Some(Hit::Delete(0)));
        assert_eq!(s.hit(290., 50.), Some(Hit::Edit(0)));
        // Saved: Copy above Show in Folder, centred like shipping (100,63).
        assert_eq!(s.hit(110., 75.), Some(Hit::Copy(0)));
        assert!(s.pointer(Some((50., 234.))));
        assert_eq!(s.hit(50., 234.), Some(Hit::Delete(1)));
        assert_eq!(s.hit(84., 234.), Some(Hit::Card(1)));
        assert_eq!(s.hit(40., 390.), Some(Hit::ClearAll));
        // Clicks reach the window underneath everywhere else: above the top
        // card, in the side margin, between cards and beside the toolbar.
        assert!(s.solid(50., 50.) && s.solid(40., 390.));
        for (x, y) in [(50., 10.), (10., 100.), (50., 208.), (300., 390.)] {
            assert!(!s.solid(x, y), "({x}, {y}) should pass through");
        }
    }

    #[test]
    fn automation_lists_every_cards_buttons_without_hover() {
        use captures_settings::MiniPreviewPlacement as P;
        let mut s = stack(P::BottomLeft, &[true, false]);
        s.cards[1].clipboard = true;
        s.cards[1].busy = true;
        let items = s.uia_items();
        let names = |group: u32| {
            items
                .iter()
                .filter(|i| i.parent == Some(group))
                .map(|i| (i.name.as_str(), i.enabled))
                .collect::<Vec<_>>()
        };
        // Saved card: Close, Delete, Edit, Copy, Show in Folder and its size.
        assert_eq!(
            names(100),
            vec![
                ("Close", true),
                ("Delete", true),
                ("Edit", true),
                ("Copy", true),
                ("Show in Folder", true),
                ("1600 × 1000 · 842 KB", true),
            ]
        );
        // Unsaved, on the clipboard and busy: no Close or Copy, buttons off,
        // and the clipboard status.
        let second = names(200);
        assert_eq!(
            &second[..3],
            &[("Delete", false), ("Edit", false), ("Save file", false)]
        );
        assert_eq!(second.last(), Some(&("Copied to clipboard", true)));
        let toolbar: Vec<_> = items[items.len() - 2..]
            .iter()
            .map(|i| i.name.as_str())
            .collect();
        assert_eq!(toolbar, ["Clear all previews", "Minimize previews"]);
        // Keys round-trip to the click targets.
        for hit in [Hit::ClearAll, Hit::Close(0), Hit::Save(1), Hit::Edit(3)] {
            assert_eq!(Stack::uia_hit(Stack::uia_key(hit)), Some(hit));
        }
    }

    #[test]
    fn the_pile_is_one_expand_target_and_the_toolbar_needs_two_cards() {
        use captures_settings::MiniPreviewPlacement as P;
        // One card: no Clear all or Minimize (it has its own Close/Delete).
        let single = stack(P::BottomLeft, &[false]);
        assert!(single.uia_items().iter().all(|i| i.key > 3));
        // Two cards: Minimize sits 2 DIPs beside Clear all, inside it on
        // the right (`row-reverse`).
        let mut s = stack(P::BottomLeft, &[true, false]);
        assert_eq!((s.minimize_rect().left, s.minimize_rect().top), (58., 380.));
        assert_eq!(s.hit(70., 390.), Some(Hit::Minimize));
        let right = stack(P::BottomRight, &[true, false]);
        assert_eq!(right.minimize_rect().right, right.clear_rect().left - 2.);
        // Collapsed: only the front card answers, as Expand, and the tree is
        // that one button.
        s.stack.set_collapsed(true);
        s.height = s.content_height();
        let front = s.front_rect().expect("front card");
        let (x, y) = (front.left + 10., front.top + 10.);
        assert_eq!(s.hit(x, y), Some(Hit::Expand));
        assert!(s.pointer(Some((x, y))));
        assert!(s.pile_hovered && s.hovered.is_none());
        assert_eq!(s.hit(40., front.top - 30.), None);
        let items = s.uia_items();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "Expand 2 previews");
        assert_eq!(Stack::uia_hit(items[0].key), Some(Hit::Expand));
    }

    #[test]
    fn nothing_takes_the_pointer_while_the_pile_flies() {
        use captures_settings::MiniPreviewPlacement as P;
        let slow = Tween {
            duration_ms: 60_000.,
            easing: captures_app::motion::CubicBezier::LINEAR,
        };
        let mut s = stack(P::BottomLeft, &[true, false]);
        s.motion = Some((slow, slow));
        s.set_collapsed(true);
        s.stack.set_collapsed(true);
        assert!(s.animating());
        let progress = s.progress();
        assert!((0. ..0.01).contains(&progress), "{progress}");
        // Mid-fly: no toolbar, no card, no Expand target anywhere.
        assert_eq!(s.hit(40., 390.), None);
        assert_eq!(s.hit(150., 100.), None);
        // Reduced motion lands at once.
        s.reduced = true;
        assert_eq!(s.progress(), 1.);
        assert!(!s.animating());
    }

    #[test]
    fn right_and_top_placements_mirror_the_controls() {
        use captures_settings::MiniPreviewPlacement as P;
        // Shipping top-right: Clear all at the top edge-aligned (284, 16),
        // the newest card first under the gutter, Delete at the card's
        // right and Edit at its left.
        let mut s = stack(P::TopRight, &[false, false]);
        assert_eq!((s.clear_rect().left, s.clear_rect().top), (284., 16.));
        assert_eq!(s.card_rect(1).map(|r| r.top), Some(52.));
        assert_eq!(s.card_rect(0).map(|r| r.top), Some(236.));
        assert!(s.pointer(Some((100., 100.))));
        assert_eq!(s.hit(290., 70.), Some(Hit::Delete(1)));
        assert_eq!(s.hit(45., 70.), Some(Hit::Edit(1)));
    }

    #[test]
    fn a_taller_stack_than_the_window_keeps_the_newest_card_in_view() {
        use captures_settings::MiniPreviewPlacement as P;
        let mut s = stack(P::BottomRight, &[false, false, false]);
        s.height = 300.;
        // The newest (last) card sits just above the gutter at the bottom.
        let newest = s.card_rect(2).unwrap();
        assert_eq!(newest.bottom, 300. - GUTTER);
        assert_eq!(s.clear_rect().top, 300. - GUTTER + 8.);
    }
}
