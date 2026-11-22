//! Floating mini previews (`?view=thumbnail`): each saved screenshot or
//! recording joins one topmost, non-activating stack window in the
//! configured corner of the display under the pointer. Cards follow
//! `styles/mini-preview.css`: the cover image with its size chip and the
//! clipboard confirmation, and on hover a blurred, dimmed image under Close
//! and Delete, Edit, and Copy and Save file (Show in Folder once saved). A
//! recording's card, like GTK's, has no Edit or Copy and saves its video. A
//! failed action shows `.thumbnail-message`. Membership, geometry and copy
//! come from `captures_app::preview` and `preview_chrome`.
//!
//! UI Automation lists each card as shipping's `<article>` with its buttons
//! (Invoke), size line and clipboard status, then Clear all previews
//! ([`Stack::uia_items`] through `panel_uia`). Buttons are exposed whether or
//! not the card is hovered, as they are in shipping's DOM.
//!
//! Cards rise in with shipping's `thumbnail-arrive`; Close streaks a card
//! out, Delete dissolves it into dust from the trash control and Clear all
//! streaks every card out bottom first, each holding its slot
//! (`preview_motion::StackExits`) while older cards settle into the hole, as
//! on GTK. The Close streak's horizontal blur and the arrive blur are not
//! drawn.
//!
//! Not ported yet: file drags, keyboard focus, tooltips, the capture
//! highlight and the editor-presence pill.

use std::cell::{OnceCell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};

use captures_app::motion::{Animation, Motion, Transition, Tween};
use captures_app::preview::{self, CollapsedCardPose, PreviewStack};
use captures_app::preview_chrome;
use captures_app::preview_motion::{self, DustParticle, ExitKind, StackExits, StackFan};
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
/// Drives the pile's fly and fan, and card arrivals and exits, while they run.
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
    /// The last action's failure (shipping `.thumbnail-message`), cleared
    /// when the next action starts.
    error: Option<String>,
}

impl Card {
    /// A recording's card offers no Edit or Copy, and saves the video (GTK).
    fn recording(&self) -> bool {
        self.entry.kind.is_recording()
    }
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
    /// Shipping's card arrive and exit animations; `None` (offscreen
    /// fixtures) adds and removes cards at once.
    pub card_motion: Option<CardMotion>,
    /// Arrival start per card id.
    arrivals: HashMap<String, f64>,
    /// `cards` is the display list: live cards plus exiting ones, which keep
    /// their slot here until their exit ends. `stack` holds only live ones.
    exits: StackExits,
    /// A Delete's dust chips, by card id.
    dust: HashMap<String, Dust>,
}

/// A dissolving card's chips: how each moves, and their filtered pixels,
/// rendered on the first frame that paints them.
struct Dust {
    particles: Vec<DustParticle>,
    chips: OnceCell<Option<ID2D1Bitmap1>>,
}

/// Transparent fringe around each chip for its blur, and the gap between
/// neighbouring chips' fringes in the atlas (macOS `DustTextures`).
const DUST_CHIP_PAD: f32 = 8.;
const DUST_CHIP_GUTTER: f32 = 16.;
/// `.thumbnail-dust { filter: blur(2px) }`.
const DUST_CHIP_BLUR: f32 = 2.;

/// `thumbnail-arrive`, the Close exit (`thumbnail-dismiss`) and the Delete
/// fallback for a card without an image.
#[derive(Clone, Copy)]
pub struct CardMotion {
    arrive: Animation,
    dismiss: Animation,
    fallback: Animation,
}

impl CardMotion {
    pub fn resolve(tokens: &Tokens) -> Option<Self> {
        Some(Self {
            arrive: Motion::PreviewCardArrive.resolve(tokens)?,
            dismiss: Motion::PreviewDismiss.resolve(tokens)?,
            fallback: Motion::PreviewDeleteFallback.resolve(tokens)?,
        })
    }
}

/// Opacities of a card's parts while it dissolves (all 1 otherwise): its
/// shadow and outline, its surface and image, and its chips and controls.
#[derive(Clone, Copy)]
struct Fade {
    frame: f32,
    source: f32,
    chrome: f32,
}

impl Fade {
    const NONE: Self = Self {
        frame: 1.,
        source: 1.,
        chrome: 1.,
    };
}

/// Paint `draw` at `opacity`: nothing when invisible, through a layer when
/// translucent.
fn faded(dc: &ID2D1DeviceContext, opacity: f32, draw: impl FnOnce() -> Result<()>) -> Result<()> {
    if opacity <= 0. {
        return Ok(());
    }
    if opacity >= 1. {
        return draw();
    }
    unsafe {
        dc.PushLayer(
            &D2D1_LAYER_PARAMETERS1 {
                contentBounds: rect(-1e6, -1e6, 1e6, 1e6),
                opacity,
                maskTransform: Matrix3x2::identity(),
                ..Default::default()
            },
            None::<&ID2D1Layer>,
        );
    }
    let result = draw();
    unsafe { dc.PopLayer() };
    result
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
            card_motion: None,
            arrivals: HashMap::new(),
            exits: StackExits::default(),
            dust: HashMap::new(),
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

    /// The fly, the fan, an arrival or an exit is still moving, or a finished
    /// exit still waits for [`Self::prune_exits`].
    pub fn animating(&self) -> bool {
        let now = now_ms();
        let pile = self.motion.is_some_and(|(fly, fan)| {
            fly.running(now - self.fly.1, self.reduced)
                || self
                    .fan
                    .running(self.cards.len().saturating_sub(1), now, &fan, self.reduced)
        });
        let arriving = self.card_motion.is_some_and(|m| {
            self.arrivals
                .values()
                .any(|start| m.arrive.running(now - start, self.reduced))
        });
        pile || arriving || !self.exits.is_empty()
    }

    /// The card at display `index` is playing its exit.
    fn exiting(&self, index: usize) -> bool {
        self.cards
            .get(index)
            .is_some_and(|card| self.exits.exiting(&card.entry.id).is_some())
    }

    /// Play the card's shipping exit while it keeps its slot, or drop it at
    /// once (reduced motion, fixtures, a collapsed or flying pile). Older
    /// cards settle into the slot when `settles`.
    fn leave(&mut self, id: &str, kind: ExitKind, delay_ms: f64, settles: bool) {
        let Some(index) = self.cards.iter().position(|c| c.entry.id == id) else {
            return;
        };
        let progress = self.progress();
        let playing = !self.reduced
            && self.card_motion.is_some()
            && !self.collapsed()
            && progress <= 0.
            && self.stack.ids().iter().any(|live| live == id);
        let image = self.cards[index].image.as_ref().map(|image| {
            let size = unsafe { image.GetSize() };
            (f64::from(size.width), f64::from(size.height))
        });
        let kind = match (kind, image) {
            (ExitKind::Dust, None) => ExitKind::DeleteFallback,
            _ => kind,
        };
        let begun = playing
            && self.exits.begin(
                self.stack.ids(),
                id,
                kind,
                now_ms(),
                delay_ms,
                settles,
                &preview_motion::settle_tween(),
            );
        if !begun {
            self.remove(index);
            return;
        }
        if let (ExitKind::Dust, Some(size)) = (kind, image) {
            let saved = self.cards[index].entry.saved_path.is_some();
            let origin = preview_motion::delete_origin_x(f64::from(CARD_W), saved, self.right);
            // FNV-1a of the id, as GTK, so a card always crumbles the same way.
            let seed = id.bytes().fold(0x811c_9dc5_u32, |hash, byte| {
                (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
            });
            let particles = preview_motion::dust_particles(
                f64::from(CARD_W),
                f64::from(CARD_H),
                size,
                (origin, preview_motion::DELETE_ORIGIN_Y),
                seed,
            );
            self.dust.insert(
                id.to_owned(),
                Dust {
                    particles,
                    chips: OnceCell::new(),
                },
            );
        }
        self.stack.remove(id);
        self.arrivals.remove(id);
        self.hovered = None;
        self.hot = None;
        self.pressed = None;
    }

    /// Clear all: every card streaks out as it looks now, the bottom card
    /// first, without the survivors settling. History keeps them.
    fn clear_all(&mut self) {
        let ids = self.stack.ids().to_vec();
        for (index, id) in ids.iter().enumerate() {
            let delay = preview_motion::clear_delay_ms(ids.len(), index, self.top);
            self.leave(id, ExitKind::Dismiss, delay, false);
        }
    }

    /// Drop the cards whose exit has ended; true when any left, so the
    /// window shrinks to the smaller stack where the settle ended.
    fn prune_exits(&mut self) -> bool {
        if !self
            .exits
            .prune(now_ms(), self.reduced, &preview_motion::settle_tween())
        {
            return false;
        }
        let display = self.exits.display_ids().to_vec();
        self.cards.retain(|card| display.contains(&card.entry.id));
        self.dust.retain(|id, _| display.contains(id));
        // Display indices moved; the next pointer event finds the hover again.
        self.hovered = None;
        self.hot = None;
        self.pressed = None;
        true
    }

    /// Collapse into the pile or expand it, flying from wherever the cards
    /// are now.
    pub fn set_collapsed(&mut self, collapsed: bool) {
        // The pile has no exits: cards still leaving go at once.
        if !self.exits.is_empty() {
            self.exits.clear();
            let display = self.exits.display_ids().to_vec();
            self.cards.retain(|card| display.contains(&card.entry.id));
            self.dust.clear();
        }
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
        // A card restored while it is still leaving comes back as a new one.
        if let Some(index) = self
            .cards
            .iter()
            .position(|c| c.entry.id == entry.id && self.exits.exiting(&c.entry.id).is_some())
        {
            self.remove(index);
        }
        if !self.stack.insert(entry.id.clone()) {
            return;
        }
        let image = image.and_then(|image| crate::capture::bitmap(g, image).ok());
        if self.card_motion.is_some() && !self.reduced {
            self.arrivals.insert(entry.id.clone(), now_ms());
        }
        self.cards.push(Card {
            entry,
            image,
            clipboard: false,
            busy: false,
            saved: false,
            error: None,
        });
        self.exits.sync(self.stack.ids());
    }

    fn remove(&mut self, index: usize) {
        if index < self.cards.len() {
            let card = self.cards.remove(index);
            let id = &card.entry.id;
            self.stack.remove(id);
            self.arrivals.remove(id);
            self.dust.remove(id);
            self.exits.release(id, false);
            self.exits.sync(self.stack.ids());
        }
        self.hovered = None;
        self.hot = None;
        self.pressed = None;
    }

    /// The live and exiting cards' height: an exiting card keeps its slot.
    fn content_height(&self) -> f32 {
        let count = self.cards.len();
        if count == 0 {
            0.
        } else if self.collapsed() {
            preview::collapsed_frame_height(count) as f32
        } else {
            preview::stack_height(count) as f32
        }
    }

    /// Content offset that keeps the anchored end visible.
    fn offset(&self) -> f32 {
        if self.top {
            0.
        } else {
            self.height - self.content_height()
        }
    }

    /// Display `index`'s slot, slid toward the anchor while it settles into
    /// an exiting card's hole.
    fn card_rect(&self, index: usize) -> Option<Rect> {
        let id = &self.cards.get(index)?.entry.id;
        let layout =
            preview::card_layout_in(self.cards.len(), index, self.collapsed(), self.top, false)?;
        let shift = self.exits.shift_px(
            id,
            now_ms(),
            self.reduced,
            &preview_motion::settle_tween(),
            self.top,
        );
        let y = (layout.y + shift) as f32 + self.offset();
        Some(rect(PAD, y, PAD + CARD_W, y + CARD_H))
    }

    pub fn collapsed(&self) -> bool {
        self.stack.is_collapsed()
    }

    /// Shipping shows the toolbar (Clear all and Minimize) only for two or
    /// more cards, and never on the pile.
    fn toolbar(&self) -> bool {
        !self.collapsed() && self.stack.ids().len() >= 2 && self.progress() <= 0.
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
        if self.cards[index].recording() {
            return controls;
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
        let shown = &self.cards[index];
        if !shown.clipboard && !shown.recording() {
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
            // A leaving card no longer takes the pointer (shipping's
            // `pointer-events: none`).
            if !inside(&card) || self.exiting(index) {
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
                !self.exiting(i)
                    && self
                        .card_rect(i)
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
            // A leaving card has left shipping's tree.
            let Some(r) = self.card_rect(index).filter(|_| !self.exiting(index)) else {
                continue;
            };
            let group = Self::uia_key(Hit::Card(index));
            let name = if card.recording() {
                "Recording preview"
            } else {
                "Screenshot preview"
            };
            items.push(Item::new(group, Role::Group, name, r));
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
            let meta = preview::card_metadata(card.entry.size_bytes);
            let mut text = Item::new(group + 6, Role::Text, meta, bar);
            text.parent = Some(group);
            items.push(text);
            if card.clipboard {
                let mut status = Item::new(group + 7, Role::Status, "Copied to clipboard", bar);
                status.parent = Some(group);
                items.push(status);
            }
            if let Some(error) = &card.error {
                let mut message = Item::new(group + 8, Role::Text, error.clone(), bar);
                message.parent = Some(group);
                items.push(message);
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
                self.paint_listed(dc, g, t, index, card)?;
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
                .and_then(|()| self.paint_card(dc, g, t, index, local, Fade::NONE))
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

    /// A listed card under its arrival or exit pose, then a dissolving
    /// card's dust above it.
    fn paint_listed(
        &self,
        dc: &ID2D1DeviceContext,
        g: &Gfx,
        t: &Tokens,
        index: usize,
        r: Rect,
    ) -> Result<()> {
        let id = &self.cards[index].entry.id;
        let now = now_ms();
        let (mut opacity, mut dx, mut dy, mut scale) = (1., 0., 0., 1.);
        let mut fade = Fade::NONE;
        let mut dust = None;
        if let Some(m) = &self.card_motion {
            if let Some(start) = self.arrivals.get(id) {
                let pose = m.arrive.pose_at(now - start, self.reduced);
                opacity *= pose.opacity;
                dy += pose.translate_y;
                scale *= pose.scale;
            }
            match self.exits.exiting(id) {
                Some(exit) if exit.kind == ExitKind::Dust => {
                    let elapsed = exit.elapsed_ms(now).max(0.);
                    let frame = preview_motion::dust_frame(elapsed, self.reduced);
                    fade = Fade {
                        frame: preview_motion::delete_frame_opacity(t, elapsed, self.reduced)
                            as f32,
                        source: frame.source_opacity as f32,
                        chrome: frame.chrome_opacity as f32,
                    };
                    dust = Some((elapsed, frame));
                }
                Some(exit) => {
                    let animation = if exit.kind == ExitKind::Dismiss {
                        m.dismiss
                    } else {
                        m.fallback
                    };
                    let pose = animation.pose_at(exit.elapsed_ms(now).max(0.), self.reduced);
                    let mirror = if self.right { -1. } else { 1. };
                    dx += pose.translate_x * mirror;
                    opacity *= pose.opacity;
                    scale *= pose.scale;
                }
                None => {}
            }
        }
        let mut base = Matrix3x2::identity();
        unsafe { dc.GetTransform(&mut base) };
        // Scale about the card's centre, then move it.
        let (cx, cy) = ((r.left + r.right) / 2., (r.top + r.bottom) / 2.);
        let s = scale as f32;
        let posed = Matrix3x2 {
            M11: s,
            M12: 0.,
            M21: 0.,
            M22: s,
            M31: cx + dx as f32 - s * cx,
            M32: cy + dy as f32 - s * cy,
        } * base;
        unsafe { dc.SetTransform(&posed) };
        let result = faded(dc, opacity as f32, || {
            self.paint_card(dc, g, t, index, r, fade)
        });
        unsafe { dc.SetTransform(&base) };
        result?;
        match dust {
            Some((elapsed, frame)) => self.paint_dust(dc, t, index, r, elapsed, frame),
            None => Ok(()),
        }
    }

    /// A dissolving card's chips (`thumbnail-dust-layer`): each a blurred,
    /// dimmed piece of its rounded image (`.thumbnail-dust`) that rises,
    /// turns and fades from the trash control, under a clip that opens from
    /// the card's rounded rect past the card (`thumbnail-dust-clip`).
    fn paint_dust(
        &self,
        dc: &ID2D1DeviceContext,
        t: &Tokens,
        index: usize,
        r: Rect,
        elapsed: f64,
        frame: preview_motion::DustFrame,
    ) -> Result<()> {
        let card = &self.cards[index];
        let (Some(dust), Some(image)) = (self.dust.get(&card.entry.id), &card.image) else {
            return Ok(());
        };
        if frame.layer_opacity <= 0. {
            return Ok(());
        }
        let radius = t.n("thumbnail-card-radius");
        let mut base = Matrix3x2::identity();
        unsafe { dc.GetTransform(&mut base) };
        let Some(chips) = dust.chips.get_or_init(|| {
            let scale = base.M11.hypot(base.M12).max(1.);
            dust_chips(dc, image, &dust.particles, radius, scale).ok()
        }) else {
            return Ok(());
        };
        let columns = dust_columns(dust.particles.len());
        let cell = dust_cell(&dust.particles);
        let pad = preview_motion::DUST_LAYER_PAD as f32;
        let open = pad * frame.clip_open as f32;
        let corner = radius * (1. - frame.clip_open as f32);
        let clip = rect(r.left - open, r.top - open, r.right + open, r.bottom + open);
        unsafe {
            let mask = dc
                .GetFactory()?
                .CreateRoundedRectangleGeometry(&D2D1_ROUNDED_RECT {
                    rect: clip,
                    radiusX: corner,
                    radiusY: corner,
                })?;
            let mut layer = D2D1_LAYER_PARAMETERS1 {
                contentBounds: clip,
                geometricMask: std::mem::ManuallyDrop::new(Some(mask.cast()?)),
                maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                maskTransform: Matrix3x2::identity(),
                opacity: 1.,
                ..Default::default()
            };
            dc.PushLayer(&layer, None::<&ID2D1Layer>);
            for (i, particle) in dust.particles.iter().enumerate() {
                let visual = preview_motion::dust_visual_at(particle, elapsed);
                let alpha = (visual.opacity * frame.layer_opacity) as f32;
                if alpha <= 0. {
                    continue;
                }
                let (w, h) = (particle.width as f32, particle.height as f32);
                let (sin, cos) = (visual.rotate as f32).to_radians().sin_cos();
                let s = visual.scale as f32;
                // Scale, rotate clockwise, then move to the chip's centre.
                let m = Matrix3x2 {
                    M11: s * cos,
                    M12: s * sin,
                    M21: -s * sin,
                    M22: s * cos,
                    M31: r.left + (particle.left + visual.dx) as f32 - pad + w / 2.,
                    M32: r.top + (particle.top + visual.dy) as f32 - pad + h / 2.,
                } * base;
                dc.SetTransform(&m);
                let (x, y) = dust_chip_origin(i, columns, cell);
                let (fw, fh) = (w + 2. * DUST_CHIP_PAD, h + 2. * DUST_CHIP_PAD);
                dc.DrawBitmap(
                    chips,
                    Some(&rect(-fw / 2., -fh / 2., fw / 2., fh / 2.)),
                    alpha,
                    D2D1_INTERPOLATION_MODE_LINEAR,
                    Some(&rect(x, y, x + fw, y + fh)),
                    None,
                );
            }
            dc.SetTransform(&base);
            dc.PopLayer();
            // Release the mask reference the parameters own.
            std::mem::ManuallyDrop::drop(&mut layer.geometricMask);
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
        fade: Fade,
    ) -> Result<()> {
        let card = &self.cards[index];
        // A leaving card keeps the dimmed hover look it had when it went; a
        // dissolving one also keeps its controls while they fade.
        let hovered = self.hovered == Some(index) || self.exiting(index);
        let controls = self.hovered == Some(index) || self.dust.contains_key(&card.entry.id);
        let radius = t.n("thumbnail-card-radius");
        // `--thumbnail-card-shadow` and the 1 px white ring.
        faded(dc, fade.frame, || {
            gfx::shadow(dc, r, radius, t.shadow("thumbnail-card-shadow"))
        })?;
        let white = |a: f32| D2D1_COLOR_F {
            r: 1.,
            g: 1.,
            b: 1.,
            a,
        };
        faded(dc, fade.source, || {
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
            Ok(())
        })?;
        faded(dc, fade.frame, || {
            gfx::ring(dc, r, radius, 0., 1., white(0.08))
        })?;
        faded(dc, fade.chrome, || {
            self.paint_chrome(dc, g, t, index, r, (hovered, controls))?;
            paint_message(dc, g, t, card.error.as_deref(), r)
        })
    }

    /// The bottom bar's chips and, while `controls`, the hover controls.
    fn paint_chrome(
        &self,
        dc: &ID2D1DeviceContext,
        g: &Gfx,
        t: &Tokens,
        index: usize,
        r: Rect,
        (hovered, controls): (bool, bool),
    ) -> Result<()> {
        let card = &self.cards[index];
        let white = |a: f32| D2D1_COLOR_F {
            r: 1.,
            g: 1.,
            b: 1.,
            a,
        };
        // `.thumbnail-bottom-bar`: the meta chip (hidden on hover) and the
        // clipboard confirmation.
        let bottom = r.bottom - 8.;
        if !hovered {
            let meta = preview::card_metadata(card.entry.size_bytes);
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
        if !controls {
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

/// `.thumbnail-message`: the last action's error on one ellipsized line over
/// the bottom bar, in the signal text colour.
fn paint_message(
    dc: &ID2D1DeviceContext,
    g: &Gfx,
    t: &Tokens,
    error: Option<&str>,
    r: Rect,
) -> Result<()> {
    let Some(error) = error else {
        return Ok(());
    };
    let (pad_x, pad_y) = (t.n("s-4"), t.n("s-2"));
    let text = g.text_sized(t, error, t.n("text-2xs"), "weight-regular", None)?;
    let width = r.right - r.left - 16. - 2. * pad_x;
    g.ellipsis(&text, width)?;
    let h = text.height + 2. * pad_y;
    let bar = rect(r.left + 8., r.bottom - 8. - h, r.right - 8., r.bottom - 8.);
    gfx::fill_round(dc, bar, t.n("r-xs"), t.color("glass-strong"))?;
    gfx::text(
        dc,
        &text,
        bar.left + pad_x,
        bar.top + pad_y,
        t.color("theme-signal-text"),
    )
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
        // A hovered saved recording above an unsaved card whose Save failed.
        Some("recording") => {
            stack.cards[0].entry.kind = captures_history::ArtifactKind::Video;
            stack.cards[0].clipboard = false;
            stack.cards[1].error =
                Some("Could not save: the Captures folder is not writable".into());
            stack.pointer(Some((150., 90.)));
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

/// Columns of the square-ish chip atlas.
fn dust_columns(count: usize) -> usize {
    ((count as f64).sqrt().ceil() as usize).max(1)
}

/// One atlas cell: the largest chip, its blur fringe and the gutter.
fn dust_cell(particles: &[DustParticle]) -> (f32, f32) {
    let largest =
        |side: fn(&DustParticle) -> f64| particles.iter().map(side).fold(0., f64::max) as f32;
    let extra = 2. * DUST_CHIP_PAD + DUST_CHIP_GUTTER;
    (largest(|p| p.width) + extra, largest(|p| p.height) + extra)
}

/// The top left of chip `index`'s fringe in the atlas, in DIPs.
fn dust_chip_origin(index: usize, columns: usize, (w, h): (f32, f32)) -> (f32, f32) {
    let half = DUST_CHIP_GUTTER / 2.;
    (
        (index % columns) as f32 * w + half,
        (index / columns) as f32 * h + half,
    )
}

/// Every chip of a dissolving card, filtered once into one bitmap at
/// `scale` (macOS `DustTextures`): its slice of the card's `object-fit:
/// cover`, `radius`-rounded image at `brightness(.5)`, then `blur(2px)`
/// into a transparent fringe. Slices are cut before the blur, so each chip
/// gets soft edges of its own. Without the blur effect (Wine) the chips
/// stay sharp.
fn dust_chips(
    dc: &ID2D1DeviceContext,
    image: &ID2D1Bitmap1,
    particles: &[DustParticle],
    radius: f32,
    scale: f32,
) -> Result<ID2D1Bitmap1> {
    let size = unsafe { image.GetSize() };
    if particles.is_empty() || size.width <= 0. || size.height <= 0. {
        return Err(Error::empty());
    }
    let columns = dust_columns(particles.len());
    let cell = dust_cell(particles);
    let rows = particles.len().div_ceil(columns);
    let px = D2D_SIZE_U {
        width: (columns as f32 * cell.0 * scale).ceil() as u32,
        height: (rows as f32 * cell.1 * scale).ceil() as u32,
    };
    let props = D2D1_BITMAP_PROPERTIES1 {
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
        },
        dpiX: 96. * scale,
        dpiY: 96. * scale,
        bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET,
        colorContext: std::mem::ManuallyDrop::new(None),
    };
    let k = (CARD_W / size.width).max(CARD_H / size.height);
    let placed = Matrix3x2 {
        M11: k,
        M12: 0.,
        M21: 0.,
        M22: k,
        M31: (CARD_W - size.width * k) / 2.,
        M32: (CARD_H - size.height * k) / 2.,
    };
    let card = D2D1_ROUNDED_RECT {
        rect: rect(0., 0., CARD_W, CARD_H),
        radiusX: radius,
        radiusY: radius,
    };
    unsafe {
        // A context of our own: the window's is mid-draw on its surface.
        let ctx = dc
            .GetDevice()?
            .CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
        let slices = ctx.CreateBitmap(px, None, 0, &props)?;
        ctx.SetTarget(&slices);
        ctx.SetDpi(96. * scale, 96. * scale);
        let brush = ctx.CreateBitmapBrush(
            image,
            Some(&D2D1_BITMAP_BRUSH_PROPERTIES1 {
                extendModeX: D2D1_EXTEND_MODE_CLAMP,
                extendModeY: D2D1_EXTEND_MODE_CLAMP,
                interpolationMode: D2D1_INTERPOLATION_MODE_LINEAR,
            }),
            None,
        )?;
        brush.SetTransform(&placed);
        let shade = ctx.CreateSolidColorBrush(
            &D2D1_COLOR_F {
                r: 0.,
                g: 0.,
                b: 0.,
                a: preview_chrome::HOVER_MEDIA_BRIGHTNESS as f32,
            },
            None,
        )?;
        ctx.BeginDraw();
        ctx.Clear(Some(&D2D1_COLOR_F::default()));
        for (i, p) in particles.iter().enumerate() {
            let (x, y) = dust_chip_origin(i, columns, cell);
            let (left, top) = (p.source_left as f32, p.source_top as f32);
            // Card coordinates, moved so this slice lands inside its fringe.
            ctx.SetTransform(&Matrix3x2::translation(
                x + DUST_CHIP_PAD - left,
                y + DUST_CHIP_PAD - top,
            ));
            ctx.PushAxisAlignedClip(
                &rect(left, top, left + p.width as f32, top + p.height as f32),
                D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
            );
            ctx.FillRoundedRectangle(&card, &brush);
            ctx.FillRoundedRectangle(&card, &shade);
            ctx.PopAxisAlignedClip();
        }
        ctx.EndDraw(None, None)?;
        let blurred = (|| -> Result<ID2D1Bitmap1> {
            let effect = ctx.CreateEffect(&CLSID_D2D1GaussianBlur)?;
            effect.SetInput(0, &slices, true);
            effect.SetValue(
                D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION.0 as u32,
                D2D1_PROPERTY_TYPE_FLOAT,
                &DUST_CHIP_BLUR.to_le_bytes(),
            )?;
            let chips = ctx.CreateBitmap(px, None, 0, &props)?;
            ctx.SetTarget(&chips);
            ctx.BeginDraw();
            ctx.Clear(Some(&D2D1_COLOR_F::default()));
            ctx.SetTransform(&Matrix3x2::identity());
            ctx.DrawImage(
                &effect.cast::<ID2D1Image>()?,
                None,
                None,
                D2D1_INTERPOLATION_MODE_LINEAR,
                D2D1_COMPOSITE_MODE_SOURCE_OVER,
            );
            ctx.EndDraw(None, None)?;
            Ok(chips)
        })();
        ctx.SetTarget(None);
        Ok(blurred.unwrap_or(slices))
    }
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
        previews.stack.reduced = crate::appearance::reduced_motion();
        previews.stack.push(&g, entry.clone(), image.as_ref());
        if clipboard {
            previews.set_clipboard(&entry.id);
        }
        previews.place_on_pointer_monitor()?;
        previews.render()?;
        previews.animate();
        Ok(())
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

/// Settings changed elsewhere: redraw the stack in the saved appearance and
/// accent.
pub fn settings_changed(settings: &captures_settings::AppSettings) {
    PREVIEWS.with(|p| {
        let Ok(mut p) = p.try_borrow_mut() else {
            return;
        };
        let Some(previews) = p.as_mut() else { return };
        previews.tokens =
            crate::appearance::tokens(previews.options.appearance.as_deref(), settings);
        if let Err(error) = previews.render() {
            eprintln!("Could not restyle the mini previews: {error}");
        }
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
                stack.card_motion = CardMotion::resolve(&tokens);
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
        self.stack.reduced = crate::appearance::reduced_motion();
        let Some(index) = hit.card() else {
            // Clear all: dismiss every card; History keeps them.
            self.stack.clear_all();
            let _ = self.relayout();
            self.animate();
            return;
        };
        let Some(card) = self.stack.cards.get(index) else {
            return;
        };
        if card.busy {
            return;
        }
        let (id, saved_path) = (card.entry.id.clone(), card.entry.saved_path.clone());
        let recording = card.recording();
        // Each action starts without the last one's message (`setError("")`).
        self.stack.cards[index].error = None;
        match hit {
            Hit::Close(_) => {
                self.stack.leave(&id, ExitKind::Dismiss, 0., true);
                let _ = self.relayout();
                self.animate();
            }
            // Unsaved captures only dismiss, as dust like any Delete; a saved
            // file goes to the Recycle Bin first (shipping `trash_artifact`).
            // History keeps both.
            Hit::Delete(_) if saved_path.is_none() => {
                self.stack.leave(&id, ExitKind::Dust, 0., true);
                let _ = self.relayout();
                self.animate();
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
            // Recordings have no Edit or Copy control.
            Hit::Edit(_) if !recording => {
                if let Err(error) = crate::editor::open(&self.options, &id) {
                    self.stack.cards[index].error = Some(error.message());
                }
            }
            Hit::Copy(_) if !recording => {
                let path = self
                    .root
                    .join(&id)
                    .join(captures_history::HISTORY_IMAGE_FILE);
                match crate::live::copy_png(&path) {
                    Ok(()) => {
                        self.set_clipboard(&id);
                        self.stack.hot = None;
                    }
                    Err(error) => self.stack.cards[index].error = Some(error),
                }
            }
            Hit::Save(_) if saved_path.is_some() => {
                if let Err(error) = crate::history::reveal(saved_path.as_deref().unwrap_or("")) {
                    self.stack.cards[index].error = Some(error);
                }
            }
            Hit::Save(_) => {
                let settings =
                    captures_settings::load(&self.options.settings_file).unwrap_or_default();
                self.stack.cards[index].busy = true;
                let (root, sender) = (self.root.clone(), self.sender.clone());
                let hwnd = self.hwnd.0 as isize;
                let directory = PathBuf::from(settings.output_directory);
                // A recording saves its video, a screenshot its image in
                // the chosen format.
                let request = if recording {
                    captures_app::Request::SaveRecording {
                        root,
                        id: id.clone(),
                        directory,
                    }
                } else {
                    captures_app::Request::SaveScreenshot {
                        root,
                        id: id.clone(),
                        directory,
                        format: settings.screenshot_format,
                    }
                };
                std::thread::spawn(move || {
                    let result = captures_app::execute(request);
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
            Hit::Edit(_)
            | Hit::Copy(_)
            | Hit::Card(_)
            | Hit::ClearAll
            | Hit::Minimize
            | Hit::Expand => {}
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
        // Finished exits leave and the window shrinks to the smaller stack;
        // the pointer finds its card again in the new order.
        if self.stack.prune_exits() {
            let _ = self.relayout();
            self.stack.pointer(self.cursor());
        }
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
                            Err(error) => card.error = Some(error),
                        }
                    }
                    crate::history::with_history(|h| h.reload());
                }
                Done::Deleted(id, result) => {
                    let index = self.stack.cards.iter().position(|c| c.entry.id == id);
                    match (index, result) {
                        (Some(_), Ok(())) => self.stack.leave(&id, ExitKind::Dust, 0., true),
                        (Some(index), Err(error)) => {
                            self.stack.cards[index].busy = false;
                            self.stack.cards[index].error = Some(error);
                        }
                        (None, _) => {}
                    }
                    crate::history::with_history(|h| h.reload());
                }
            }
        }
        let _ = self.relayout();
        self.animate();
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
                error: None,
            });
        }
        stack.height = stack.content_height();
        stack
    }

    #[test]
    fn a_recording_card_offers_no_edit_or_copy_and_shows_its_error() {
        use captures_settings::MiniPreviewPlacement as P;
        let mut s = stack(P::BottomLeft, &[false]);
        s.cards[0].entry.kind = captures_history::ArtifactKind::Video;
        s.cards[0].error = Some("disk full".into());
        let card = s.card_rect(0).unwrap();
        assert!(s.pointer(Some((card.left + 30., card.top + 30.))));
        // Edit's corner is the card itself; the only main action is Save.
        assert_eq!(s.hit(card.right - 20., card.top + 20.), Some(Hit::Card(0)));
        let items = s.uia_items();
        let names: Vec<&str> = items.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "Recording preview",
                "Delete",
                "Save file",
                "842 KB",
                "disk full"
            ]
        );
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
        // Shipping layout with the tighter 16 DIP stack gap: cards at y 28 and
        // 204 in a 416 DIP window, Clear all 8 DIPs under the last card.
        let mut s = stack(P::BottomLeft, &[true, false]);
        assert_eq!(s.card_rect(0).map(|r| (r.left, r.top)), Some((28., 28.)));
        assert_eq!(s.card_rect(1).map(|r| (r.left, r.top)), Some((28., 204.)));
        assert_eq!((s.clear_rect().left, s.clear_rect().top), (28., 372.));
        // Controls exist only on the hovered card: a saved card offers
        // Close and Delete, an unsaved one Delete (dismiss) only.
        assert_eq!(s.hit(50., 50.), Some(Hit::Card(0)));
        assert!(s.pointer(Some((50., 50.))));
        assert_eq!(s.hit(50., 50.), Some(Hit::Close(0)));
        assert_eq!(s.hit(84., 50.), Some(Hit::Delete(0)));
        assert_eq!(s.hit(290., 50.), Some(Hit::Edit(0)));
        // Saved: Copy above Show in Folder, centred like shipping (100,63).
        assert_eq!(s.hit(110., 75.), Some(Hit::Copy(0)));
        assert!(s.pointer(Some((50., 226.))));
        assert_eq!(s.hit(50., 226.), Some(Hit::Delete(1)));
        assert_eq!(s.hit(84., 226.), Some(Hit::Card(1)));
        assert_eq!(s.hit(40., 382.), Some(Hit::ClearAll));
        // Clicks reach the window underneath everywhere else: above the top
        // card, in the side margin, between cards and beside the toolbar.
        assert!(s.solid(50., 50.) && s.solid(40., 382.));
        for (x, y) in [(50., 10.), (10., 100.), (50., 196.), (300., 382.)] {
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
                ("842 KB", true),
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
        assert_eq!((s.minimize_rect().left, s.minimize_rect().top), (58., 372.));
        assert_eq!(s.hit(70., 382.), Some(Hit::Minimize));
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
    fn a_closed_card_holds_its_slot_while_it_streaks_out() {
        use captures_settings::MiniPreviewPlacement as P;
        let tokens = Tokens::variant("light", "mustard");
        let mut s = stack(P::BottomLeft, &[true, false]);
        s.card_motion = CardMotion::resolve(&tokens);
        assert!(s.card_motion.is_some(), "shipping motions resolve");
        let newest = s.card_rect(1).unwrap();
        s.leave("a1", ExitKind::Dismiss, 0., true);
        // Still drawn in its slot and sizing the window, but no longer live,
        // hit or listed; the toolbar leaves with the second live card.
        assert_eq!(s.cards.len(), 2);
        assert_eq!(s.stack.ids(), ["a0"]);
        assert_eq!(s.content_height(), preview::stack_height(2) as f32);
        assert_eq!(s.card_rect(1).map(|r| r.top), Some(newest.top));
        assert_eq!(s.hit(newest.left + 10., newest.top + 10.), None);
        assert!(!s.toolbar());
        assert!(s.uia_items().iter().all(|i| i.key < 200 || i.key >= 300));
        assert!(s.animating());
        // Reduced motion removes the next card at once.
        s.reduced = true;
        s.leave("a0", ExitKind::Dust, 0., true);
        assert!(s.stack.ids().is_empty());
        assert!(s.cards.iter().all(|c| c.entry.id != "a0"));
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
        assert_eq!(s.card_rect(0).map(|r| r.top), Some(228.));
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
