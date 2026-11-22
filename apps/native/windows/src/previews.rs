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
//! on GTK. Arriving cards rise out of shipping's 3 px blur (`blurred`);
//! a closing card's image stretches under the streak's stepped horizontal
//! blur, and Clear all keeps each card's own look. Clear all and Minimize
//! enter, dissolve and leave with the shared `StackToolbar` motions, and
//! Minimize grows into "Show less" while hovered or focused.
//!
//! Dragging an expanded card's image drags its file out (`crate::drag`);
//! an accepted drop elsewhere dismisses the card and a drop back on the
//! stack shakes it (shipping `finishFileDrag`).
//!
//! Close, Delete, Edit and Clear all show shipping's preview tooltips on the
//! host's shared `tooltip::Fade`.
//!
//! Clicks never activate the window (shipping's `focusable(false)`), but
//! assistive tech can focus a button over UI Automation; Tab walks the
//! buttons, Enter and Space press one and Escape collapses the stack (GTK).
//!
//! A new card holds shipping's accent capture highlight for 1 s, then fades
//! it (`thumbnail-capture-highlight`).
//!
//! While a screenshot editor shows a card's capture, its Edit control is
//! shipping's "In editor" pill with the accent card ring
//! (`preview_chrome::EditorPresence`, polled from `editor::ids`); once the
//! editor closes the pill shrinks back and the Edit icon lingers for 3 s.

use std::cell::{OnceCell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};

use captures_app::motion::{Animation, Motion, Pose, Transition, Tween};
use captures_app::preview::{self, CollapsedCardPose, PreviewStack};
use captures_app::preview_chrome::{self, EditorPhase, EditorPresence};
use captures_app::preview_motion::{
    self, DragSway, DustParticle, ExitKind, StackExits, StackFan, StackToolbar, ToolbarCause,
};
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
    GetKeyState, ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
    VIRTUAL_KEY, VK_ESCAPE, VK_RETURN, VK_SHIFT, VK_SPACE, VK_TAB,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;
use windows_numerics::Matrix3x2;

use crate::gfx::{self, Gfx, rect};
use crate::panel_uia::{self, Item, Role};
use crate::tokens::{Shadow, Tokens};
use crate::tooltip::Fade as Tip;
use captures_app::tray_notice::LogicalRect;

/// Not exported by the `windows` crate's `WindowsAndMessaging`.
const WM_MOUSELEAVE: u32 = 0x02A3;
/// Drives the pile's fly and fan, and card arrivals and exits, while they run.
const MOTION_TIMER: usize = 7;
/// Polls the pointer while empty areas let clicks through.
const POLL_TIMER: usize = 8;
/// Checks which captures open editors show, for the "In editor" pills, and
/// ends their leave and linger (editors do not notify the stack).
const PRESENCE_TIMER: usize = 9;
const PRESENCE_INTERVAL_MS: u32 = 250;
/// Posted when a worker finished a Save or Delete.
const WM_APP_PREVIEW_DONE: u32 = WM_APP + 31;
/// A card's file is ready: run the drag loop outside the borrow.
const WM_APP_PREVIEW_DRAG: u32 = WM_APP + 32;

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
    /// `.thumbnail-overflow-cue-older` / `-newer`: the chevrons at the
    /// window's top and bottom edge of a stack taller than its window.
    CueUp,
    CueDown,
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
            Self::ClearAll | Self::Minimize | Self::Expand | Self::CueUp | Self::CueDown => None,
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
    /// A file drag of this card is running (`.thumbnail-file-dragging`): its
    /// image hides so the drag image never recaptures it.
    dragging: bool,
    /// Whether an editor shows this capture, and how its Edit control and
    /// ring follow (`.thumbnail-editor-active`, `-leaving`, `-lingering`).
    presence: EditorPresence,
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
    /// Since when the hovered pile's sparkles have run.
    sparkle_since: Option<f64>,
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
    /// When each card's capture highlight began, by card id.
    highlights: HashMap<String, f64>,
    /// Each card's hover look easing in or out, by card id.
    hover_fades: HashMap<String, HoverFade>,
    /// `cards` is the display list: live cards plus exiting ones, which keep
    /// their slot here until their exit ends. `stack` holds only live ones.
    exits: StackExits,
    /// A Delete's dust chips, by card id.
    dust: HashMap<String, Dust>,
    /// How far an expanded stack taller than its window is scrolled away
    /// from its newest card (0 keeps the newest in view, as shipping's
    /// `scrollThumbnailStackToNewest`), in DIPs.
    scroll: f32,
    /// A cue's eased scroll: where it started and when (ms).
    scroll_from: Option<(f32, f64)>,
    /// When each overflow cue appeared (`thumbnail-overflow-cue-in`).
    cues_shown: (Option<f64>, Option<f64>),
    /// The pile's shared gravity: -1 peeks down at the top of the work
    /// area, 1 peeks up at the bottom, between them for a carried pile.
    pub gravity: f64,
    /// The carried pile's rear-card lean (`DragSway`), while it is carried
    /// and until it settles after.
    pub sway: Option<DragSway>,
    /// When each card dropped back on the stack began its refusal shake
    /// (`thumbnail-drop-reject`), by card id.
    rejects: HashMap<String, f64>,
    /// The card corner tooltips (Close, Delete, Edit: 120 ms `ease`) and the
    /// toolbar's Clear all tooltip (`--dur-1` `--ease-out`).
    pub tips: [Tip<Hit>; 2],
    /// The control with keyboard focus (`:focus-visible`), while the window
    /// has it.
    pub focus: Option<Hit>,
    /// Clear all and Minimize: shown or not, and the motion they enter or
    /// leave with.
    toolbar: StackToolbar,
    /// The tokens the toolbar's motions, Minimize's morph and the editor
    /// control's eases resolve against; `None` (offscreen fixtures) lands
    /// them at once.
    pub motion_tokens: Option<Tokens>,
    /// Minimize growing into "Show less" while hovered or focused.
    minimize: Morph,
    /// The card whose Edit just opened its editor: its pill reads "In
    /// editor" until the pointer and focus leave it
    /// (`data-editor-just-opened`).
    just_opened: Option<String>,
    /// The "In editor" pill's width, as wide as its wider label
    /// (`preview_chrome::editor_pill_width`).
    pub pill_width: f32,
}

/// A card's hover look easing toward `on` since `at`, from where each
/// channel was then: the image filter (`0.18s ease`), its zoom (`0.22s
/// ease`), the controls (`0.16s ease`) and the size chip's fade (`0.15s
/// ease`), as shipping's `.thumbnail-card:hover` transitions.
#[derive(Clone, Copy, Debug, PartialEq)]
struct HoverFade {
    on: bool,
    at: f64,
    from: [f32; 4],
}

const HOVER_MS: [f64; 4] = [180., 220., 160., 150.];

impl HoverFade {
    fn values(&self, now: f64) -> [f32; 4] {
        let ease = captures_app::motion::CubicBezier::new(0.25, 0.1, 0.25, 1.)
            .unwrap_or(captures_app::motion::CubicBezier::LINEAR);
        let target = f32::from(u8::from(self.on));
        std::array::from_fn(|i| {
            let p = ease.ease(((now - self.at) / HOVER_MS[i]).clamp(0., 1.)) as f32;
            self.from[i] + (target - self.from[i]) * p
        })
    }

    fn running(&self, now: f64) -> bool {
        now - self.at < HOVER_MS[1]
    }
}

/// Minimize's "Show less" morph easing toward `open` since `at`, from where
/// it was then: its width (`--thumbnail-minimize-morph`, 240 ms) and the
/// icon-to-label swap (`--thumbnail-minimize-swap`, 180 ms), both
/// `--ease-out`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Morph {
    open: bool,
    at: f64,
    from: [f32; 2],
}

const MORPH_MS: [f64; 2] = [240., 180.];

/// The Edit control growing into or out of the "In editor" pill:
/// `max-width 0.28s var(--editor-control-ease)`.
const EDITOR_GROW_MS: f64 = 280.;
const EDITOR_EASE: [f64; 4] = [0.2, 0.8, 0.2, 1.];

impl Morph {
    fn values(&self, now: f64, ease: captures_app::motion::CubicBezier) -> [f32; 2] {
        let target = f32::from(u8::from(self.open));
        std::array::from_fn(|i| {
            let p = ease.ease(((now - self.at) / MORPH_MS[i]).clamp(0., 1.)) as f32;
            self.from[i] + (target - self.from[i]) * p
        })
    }

    fn running(&self, now: f64) -> bool {
        now - self.at < MORPH_MS[0]
    }
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

/// `--thumbnail-drop-reject-duration` (`--dur-5`).
const REJECT_MS: f64 = preview::PREVIEW_DROP_REJECT_MS as f64;

/// `thumbnail-drop-reject`'s horizontal offset `elapsed_ms` in, linear
/// (`--thumbnail-drop-reject-ease`) between its keyframes; `x` holds
/// `--thumbnail-drop-reject-x-1` … `-x-4`.
fn reject_offset([a, b, c, d]: [f32; 4], elapsed_ms: f64) -> f32 {
    let t = (elapsed_ms / REJECT_MS).clamp(0., 1.) as f32;
    let stops = [
        (0., 0.),
        (0.1, -a),
        (0.3, a),
        (0.5, -b),
        (0.65, c),
        (0.8, -d),
        (0.9, d),
        (1., 0.),
    ];
    stops.windows(2).find(|w| t <= w[1].0).map_or(0., |w| {
        let ((t0, x0), (t1, x1)) = (w[0], w[1]);
        x0 + (x1 - x0) * (t - t0) / (t1 - t0)
    })
}

/// `THUMBNAIL_STACK_SCROLL_DURATION_MS`: a cue's one-slot ease-out scroll.
const SCROLL_MS: f64 = 380.;
/// Chromium's wheel step: 100 DIPs per notch.
const WHEEL_STEP: f32 = 100.;
/// `.thumbnail-overflow-cue`: 46 × 22, 6 DIPs from the window edge.
const CUE_W: f32 = 46.;
const CUE_H: f32 = 22.;
const CUE_INSET: f32 = 6.;
/// `thumbnail-overflow-cue-in`: `var(--dur-3) var(--ease-out)`.
const CUE_IN_MS: f64 = 200.;

/// `thumbnail-arrive`, the Close exit (`thumbnail-dismiss`) and the Delete
/// fallback for a card without an image.
#[derive(Clone, Copy)]
pub struct CardMotion {
    arrive: Animation,
    dismiss: Animation,
    /// `thumbnail-dismiss-streak` on a closing card's image.
    streak: Animation,
    fallback: Animation,
    /// `thumbnail-capture-highlight`: the accent outline a new card holds for
    /// 1 s, then fades over 3.5 s.
    highlight: Animation,
    /// `thumbnail-stack-sparkle` for the hovered pile's two dot layers
    /// (1.8 s, and 2.2 s after 0.5 s), looping.
    sparkles: [Animation; 2],
}

impl CardMotion {
    pub fn resolve(tokens: &Tokens) -> Option<Self> {
        Some(Self {
            arrive: Motion::PreviewCardArrive.resolve(tokens)?,
            dismiss: Motion::PreviewDismiss.resolve(tokens)?,
            streak: Motion::PreviewDismissStreak.resolve(tokens)?,
            fallback: Motion::PreviewDeleteFallback.resolve(tokens)?,
            highlight: Motion::PreviewCaptureHighlight.resolve(tokens)?,
            sparkles: [
                Motion::PreviewPileSparkle.resolve(tokens)?,
                Motion::PreviewPileSparkleLate.resolve(tokens)?,
            ],
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

/// Paint `draw` through CSS `filter: blur(radius)`: a Gaussian of standard
/// deviation `radius`, scaled with the current transform as the filter is
/// with its element. It records on a context of our own, since the window's
/// is mid-draw with clips pushed. Without the effect (Wine) it draws sharp.
fn blurred(
    dc: &ID2D1DeviceContext,
    radius: f32,
    draw: impl Fn(&ID2D1DeviceContext) -> Result<()>,
) -> Result<()> {
    if radius < 0.05 {
        return draw(dc);
    }
    let mut transform = Matrix3x2::identity();
    unsafe { dc.GetTransform(&mut transform) };
    let image = || -> Result<ID2D1Image> {
        unsafe {
            let ctx = dc
                .GetDevice()?
                .CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            let list = ctx.CreateCommandList()?;
            ctx.SetTarget(&list);
            ctx.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            ctx.BeginDraw();
            ctx.SetTransform(&transform);
            let drawn = draw(&ctx);
            let ended = ctx.EndDraw(None, None);
            ctx.SetTarget(None);
            drawn?;
            ended?;
            list.Close()?;
            let effect = dc.CreateEffect(&CLSID_D2D1GaussianBlur)?;
            effect.SetInput(0, &list, true);
            let deviation = radius * transform.M11.hypot(transform.M12);
            effect.SetValue(
                D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION.0 as u32,
                D2D1_PROPERTY_TYPE_FLOAT,
                &deviation.to_le_bytes(),
            )?;
            effect.GetOutput()
        }
    };
    match image() {
        Ok(image) => unsafe {
            dc.SetTransform(&Matrix3x2::identity());
            dc.DrawImage(
                &image,
                None,
                None,
                D2D1_INTERPOLATION_MODE_LINEAR,
                D2D1_COMPOSITE_MODE_SOURCE_OVER,
            );
            dc.SetTransform(&transform);
            Ok(())
        },
        Err(_) => draw(dc),
    }
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
            sparkle_since: None,
            motion: None,
            reduced: false,
            fly: (0., 0.),
            fan: StackFan::default(),
            card_motion: None,
            arrivals: HashMap::new(),
            highlights: HashMap::new(),
            hover_fades: HashMap::new(),
            exits: StackExits::default(),
            dust: HashMap::new(),
            scroll: 0.,
            scroll_from: None,
            cues_shown: (None, None),
            gravity: if placement.is_top() { -1. } else { 1. },
            sway: None,
            rejects: HashMap::new(),
            tips: [
                Tip::timed(Transition::PreviewIconTooltip),
                Tip::timed(Transition::PreviewStackTooltip),
            ],
            focus: None,
            toolbar: StackToolbar::default(),
            motion_tokens: None,
            minimize: Morph::default(),
            just_opened: None,
            pill_width: preview_chrome::EDITOR_PILL_MAX_WIDTH as f32,
        }
    }

    /// Size the "In editor" pill to its labels in this host's type.
    pub fn measure_pill(&mut self, g: &Gfx, t: &Tokens) {
        let width = |label| {
            g.text_sized(t, label, t.n("text-2xs"), "weight-semibold", None)
                .map_or(0., |text| f64::from(text.width))
        };
        self.pill_width = preview_chrome::editor_pill_width(
            width(preview_chrome::EDITOR_PRESENT_LABEL),
            width(preview_chrome::EDITOR_SHOW_LABEL),
        ) as f32;
    }

    /// Report the captures open editors show; true when a card's editor
    /// control changed phase (shipping `editorPresence`).
    pub fn sync_presence(&mut self, open: &[String], now: f64) -> bool {
        let reduced = self.reduced;
        let mut changed = false;
        for card in &mut self.cards {
            changed |= card
                .presence
                .set_active(open.contains(&card.entry.id), now, reduced);
        }
        if let Some(id) = &self.just_opened {
            let on = |hit: Option<Hit>| matches!(hit, Some(Hit::Edit(i)) if self.cards.get(i).is_some_and(|c| &c.entry.id == id));
            if !on(self.hot) && !on(self.focus) {
                self.just_opened = None;
            }
        }
        changed
    }

    /// How far a card's Edit control has grown into the "In editor" pill (0
    /// the compact icon; `max-width 0.28s`), and its card's accent ring,
    /// which eases out over 0.55 s `--ease-standard` once the editor closes.
    fn editor_look(&self, index: usize) -> (f32, f32) {
        let presence = self.cards[index].presence;
        let elapsed = presence.phase_elapsed_ms(now_ms());
        let eased = |ms: f64, curve: Option<[f64; 4]>| -> f32 {
            match curve
                .and_then(|[x1, y1, x2, y2]| captures_app::motion::CubicBezier::new(x1, y1, x2, y2))
            {
                Some(curve) if !self.instant() => curve.ease((elapsed / ms).clamp(0., 1.)) as f32,
                _ => 1.,
            }
        };
        let standard = self
            .motion_tokens
            .as_ref()
            .and_then(|t| captures_app::motion::MotionTokens::easing(t, "ease-standard"));
        match presence.phase() {
            EditorPhase::Present => (eased(EDITOR_GROW_MS, Some(EDITOR_EASE)), 1.),
            EditorPhase::Leaving => (
                1. - eased(EDITOR_GROW_MS, Some(EDITOR_EASE)),
                1. - eased(preview_chrome::EDITOR_PRESENCE_LEAVE_MS, standard),
            ),
            EditorPhase::Idle | EditorPhase::Lingering => (0., 0.),
        }
    }

    /// The Edit control in the card's far corner, growing inward into the
    /// "In editor" pill.
    fn edit_rect(&self, index: usize, card: Rect) -> Rect {
        let w = CONTROL + (self.pill_width - CONTROL) * self.editor_look(index).0;
        let y = card.top + 8.;
        if self.right {
            rect(card.left + 8., y, card.left + 8. + w, y + CONTROL)
        } else {
            rect(card.right - 8. - w, y, card.right - 8., y + CONTROL)
        }
    }

    /// Grow Minimize into "Show less" while it is hovered or focused and the
    /// toolbar is idle, or shrink it back, from where it is now.
    pub fn sync_minimize(&mut self, now: f64) {
        let open = self.toolbar()
            && (self.hot == Some(Hit::Minimize) || self.focus == Some(Hit::Minimize));
        if open != self.minimize.open {
            let from = self.morph_values(now);
            self.minimize = Morph {
                open,
                at: now,
                from,
            };
        }
    }

    fn morph_values(&self, now: f64) -> [f32; 2] {
        let ease = self
            .motion_tokens
            .as_ref()
            .and_then(|t| captures_app::motion::MotionTokens::easing(t, "ease-out"))
            .and_then(|[x1, y1, x2, y2]| captures_app::motion::CubicBezier::new(x1, y1, x2, y2));
        match ease {
            Some(ease) if !self.instant() => self.minimize.values(now, ease),
            _ => [f32::from(u8::from(self.minimize.open)); 2],
        }
    }

    /// Minimize's width and icon-to-label swap now (0 the icon, 1 "Show
    /// less"). Shipping keeps "Show less" while the stack collapses and the
    /// icon while the toolbar enters or leaves otherwise.
    fn minimize_look(&self) -> [f32; 2] {
        let now = now_ms();
        let forced = self
            .motion_tokens
            .as_ref()
            .and_then(|t| self.toolbar.minimize_override(t, now, self.reduced));
        match forced {
            Some(open) => [f32::from(u8::from(open)); 2],
            None => self.morph_values(now),
        }
    }

    /// Shipping shows the toolbar (Clear all and Minimize) only for two or
    /// more cards, and never on the pile. `cause` picks how it enters or
    /// leaves when that changes.
    fn sync_toolbar(&mut self, cause: ToolbarCause) {
        let shown = !self.collapsed() && self.stack.ids().len() >= 2;
        self.toolbar.set(shown, cause, now_ms());
    }

    /// How the toolbar draws now, or `None` once it is gone.
    fn toolbar_pose(&self) -> Option<Pose> {
        match &self.motion_tokens {
            Some(tokens) => self.toolbar.pose(tokens, now_ms(), self.reduced),
            None => self.toolbar.shown().then_some(Pose::REST),
        }
    }

    fn toolbar_moving(&self) -> bool {
        self.motion_tokens
            .as_ref()
            .is_some_and(|tokens| self.toolbar.running(tokens, now_ms(), self.reduced))
    }

    /// The card shows its controls: hovered, or holding keyboard focus
    /// (`.thumbnail-card:focus-within`).
    fn shows_controls(&self, index: usize) -> bool {
        self.hovered == Some(index) || self.focus.and_then(Hit::card) == Some(index)
    }

    /// Ease each card's hover look toward whether it shows its controls now.
    pub fn sync_hover(&mut self, now: f64) {
        for index in 0..self.cards.len() {
            let on = self.shows_controls(index);
            let id = &self.cards[index].entry.id;
            let fade = self.hover_fades.get(id).copied();
            if fade.map_or(on, |f| f.on != on) {
                let from = fade.map_or([0.; 4], |f| f.values(now));
                self.hover_fades
                    .insert(id.clone(), HoverFade { on, at: now, from });
            }
        }
        let cards = &self.cards;
        self.hover_fades.retain(|id, fade| {
            (fade.on || fade.running(now)) && cards.iter().any(|c| &c.entry.id == id)
        });
    }

    /// A card's hover look now (0 resting, 1 hovered): image filter, zoom,
    /// controls and the size chip's fade. Instant under reduced motion and
    /// offscreen.
    fn hover_look(&self, index: usize) -> [f32; 4] {
        let on = f32::from(u8::from(self.shows_controls(index)));
        if self.instant() {
            return [on; 4];
        }
        self.hover_fades
            .get(&self.cards[index].entry.id)
            .map_or([on; 4], |fade| fade.values(now_ms()))
    }

    /// The buttons keyboard focus visits, in automation (DOM) order.
    fn focus_order(&self) -> Vec<Hit> {
        self.uia_items()
            .into_iter()
            .filter(|item| matches!(item.role, Role::Button) && item.enabled)
            .filter_map(|item| Self::uia_hit(item.key))
            .collect()
    }

    /// Tab (or Shift+Tab) to the next (previous) button, wrapping.
    fn move_focus(&mut self, back: bool) {
        let order = self.focus_order();
        if order.is_empty() {
            self.focus = None;
            return;
        }
        let at = self
            .focus
            .and_then(|hit| order.iter().position(|h| *h == hit));
        let next = match (at, back) {
            (None, false) => 0,
            (None, true) => order.len() - 1,
            (Some(i), false) => (i + 1) % order.len(),
            (Some(i), true) => (i + order.len() - 1) % order.len(),
        };
        self.focus = Some(order[next]);
    }

    /// Focus on a control that left (its card closed, the stack collapsed)
    /// moves to the first one there is.
    fn sync_focus(&mut self) {
        if let Some(hit) = self.focus {
            let order = self.focus_order();
            if !order.contains(&hit) {
                self.focus = order.first().copied();
            }
        }
    }

    /// Where a control is drawn.
    fn control_rect(&self, hit: Hit) -> Option<Rect> {
        match hit {
            Hit::ClearAll => Some(self.clear_rect()),
            Hit::Minimize => Some(self.minimize_rect()),
            Hit::Expand => self.front_rect(),
            Hit::CueUp | Hit::CueDown => self
                .cues()
                .into_iter()
                .find(|(h, _)| *h == hit)
                .map(|(_, r)| r),
            Hit::Card(i) => self.card_rect(i),
            _ => {
                let i = hit.card()?;
                let card = self.card_rect(i)?;
                self.top_controls(i, card)
                    .into_iter()
                    .chain(self.main_actions(i, card))
                    .find(|(h, _)| *h == hit)
                    .map(|(_, r)| r)
            }
        }
    }

    /// `:focus-visible`: card controls take a 2 px ring of 70 % accent
    /// (`box-shadow: 0 0 0 2px`); toolbar controls, the pile and the cues a
    /// 2 px accent outline 2 px out.
    fn paint_focus(&self, dc: &ID2D1DeviceContext, t: &Tokens) -> Result<()> {
        // Nothing holds still mid-fly.
        let progress = self.progress();
        if progress > 0. && progress < 1. {
            return Ok(());
        }
        let Some((hit, r)) = self
            .focus
            .and_then(|hit| Some((hit, self.control_rect(hit)?)))
        else {
            return Ok(());
        };
        let mut accent = t.color("theme-accent");
        let (radius, from, to) = match hit {
            Hit::Expand => (t.n("thumbnail-card-radius"), 2., 4.),
            Hit::ClearAll | Hit::Minimize | Hit::CueUp | Hit::CueDown => (t.n("r-md"), 2., 4.),
            _ => {
                accent.a *= 0.7;
                (t.n("r-md"), 0., 2.)
            }
        };
        gfx::ring(dc, r, radius, from, to, accent)
    }

    /// The tooltip the control under the pointer shows (`data-tooltip`):
    /// Close, Delete or Edit on a card showing its controls, or Clear all.
    /// Copy, Save file and Minimize carry their own text. Keyboard focus
    /// shows one too (`:focus-visible::after`) when the pointer names none.
    fn tip_targets(&self) -> [Option<Hit>; 2] {
        let tipped = |hit: &Hit| {
            matches!(
                hit,
                Hit::Close(_) | Hit::Delete(_) | Hit::Edit(_) | Hit::ClearAll
            )
        };
        match self.hot.filter(tipped).or(self.focus.filter(tipped)) {
            // The pill and its leave carry no tooltip.
            Some(Hit::Edit(i)) if self.cards[i].presence.phase().tooltip().is_none() => {
                [None, None]
            }
            Some(hit @ (Hit::Close(i) | Hit::Delete(i) | Hit::Edit(i)))
                if self.shows_controls(i) && !self.exiting(i) =>
            {
                [Some(hit), None]
            }
            Some(Hit::ClearAll) if self.toolbar() => [None, Some(Hit::ClearAll)],
            _ => [None, None],
        }
    }

    /// Fade the tooltips toward the control under the pointer.
    pub fn sync_tips(&mut self, t: &Tokens, now: f64) {
        let targets = self.tip_targets();
        for (fade, key) in self.tips.iter_mut().zip(targets) {
            fade.set(key, t, now);
        }
    }

    pub fn tips_animating(&self, t: &Tokens, now: f64) -> bool {
        self.tips.iter().any(|fade| fade.animating(t, now))
    }

    /// A tooltip's copy, its control and whether it opens above it.
    fn tip(&self, hit: Hit) -> Option<(&'static str, Rect, bool)> {
        let above = preview_chrome::icon_tooltip_above(self.top);
        match hit {
            Hit::Close(i) | Hit::Delete(i) | Hit::Edit(i) => {
                let card = self.card_rect(i)?;
                let (_, r) = self
                    .top_controls(i, card)
                    .into_iter()
                    .find(|(h, _)| *h == hit)?;
                let label = match hit {
                    Hit::Close(_) => "Close",
                    Hit::Delete(_) => "Delete",
                    _ => preview_chrome::EDIT_LABEL,
                };
                Some((label, r, above))
            }
            Hit::ClearAll => Some(("Clear all", self.clear_rect(), above)),
            _ => None,
        }
    }

    fn paint_tips(&self, dc: &ID2D1DeviceContext, g: &Gfx, t: &Tokens) -> Result<()> {
        let now = crate::tooltip::now_ms();
        for fade in &self.tips {
            for (hit, progress) in fade.frames(t, now) {
                if let Some((label, anchor, above)) = self.tip(hit) {
                    paint_tip(dc, g, t, label, anchor, above, progress)?;
                }
            }
        }
        Ok(())
    }

    /// A card's file was dropped back on this stack: refuse it with a
    /// shake instead of dismissing it (shipping `playDropReject`). Reduced
    /// motion skips the shake, as GTK does.
    fn reject(&mut self, id: &str) {
        if !self.reduced && self.card_motion.is_some() {
            self.rejects.insert(id.to_owned(), now_ms());
        }
    }

    /// Forget finished shakes; true while one still runs.
    fn prune_rejects(&mut self) -> bool {
        let now = now_ms();
        self.rejects.retain(|_, start| now - *start < REJECT_MS);
        !self.rejects.is_empty()
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
                || self
                    .highlights
                    .values()
                    .any(|start| m.highlight.running(now - start, self.reduced))
        });
        let scrolling = self
            .scroll_from
            .is_some_and(|(_, start)| !self.reduced && now - start < SCROLL_MS);
        let cues = [self.cues_shown.0, self.cues_shown.1]
            .into_iter()
            .flatten()
            .any(|start| !self.reduced && now - start < CUE_IN_MS);
        let easing = self.rejects.values().any(|start| now - start < REJECT_MS)
            || self.sparkling()
            || (!self.instant() && self.hover_fades.values().any(|fade| fade.running(now)));
        let presence = !self.instant()
            && self.cards.iter().any(|card| {
                matches!(
                    card.presence.phase(),
                    EditorPhase::Present | EditorPhase::Leaving
                ) && card.presence.phase_elapsed_ms(now) < preview_chrome::EDITOR_PRESENCE_LEAVE_MS
            });
        pile || arriving
            || presence
            || self.toolbar_moving()
            || (!self.instant() && self.minimize.running(now))
            || !self.exits.is_empty()
            || scrolling
            || cues
            || self.sway.is_some()
            || easing
    }

    /// The largest [`Self::scroll`]: how much of the content the window
    /// cannot show.
    fn max_scroll(&self) -> f32 {
        if self.collapsed() {
            0.
        } else {
            (self.content_height() - self.height).max(0.)
        }
    }

    /// The scroll now, part way through a cue's eased scroll.
    fn scroll_now(&self) -> f32 {
        let target = self.scroll.clamp(0., self.max_scroll());
        match self.scroll_from {
            Some((from, start)) if !self.reduced => {
                let t = ((now_ms() - start) / SCROLL_MS).clamp(0., 1.);
                // `easeOutCubic`
                let eased = 1. - (1. - t).powi(3);
                from + (target - from) * eased as f32
            }
            _ => target,
        }
    }

    /// Shipping's `scrollTop`: 0 shows the top of the content.
    fn scroll_top(&self) -> f32 {
        if self.top {
            self.scroll_now()
        } else {
            self.max_scroll() - self.scroll_now()
        }
    }

    /// Scroll to shipping `scrollTop` `top`, eased like a cue or at once
    /// like the wheel.
    fn scroll_to(&mut self, top: f32, eased: bool) {
        let max = self.max_scroll();
        let top = top.clamp(0., max);
        let from = self.scroll_now();
        self.scroll = if self.top { top } else { max - top };
        self.scroll_from =
            (eased && !self.reduced && from != self.scroll).then(|| (from, now_ms()));
        // Cards moved under a resting pointer; the next sample finds them.
        self.hovered = None;
        self.hot = None;
    }

    /// An overflow cue: one card slot up (`-1`) or down (`1`), eased.
    fn scroll_slots(&mut self, slots: i32) {
        let top = preview::stack_scroll_target(
            f64::from(self.scroll_top()),
            f64::from(self.content_height()),
            f64::from(self.height),
            slots,
        );
        self.scroll_to(top as f32, true);
    }

    /// The wheel over the stack, `delta` DIPs down.
    pub fn wheel(&mut self, delta: f32) -> bool {
        if self.collapsed() || self.progress() > 0. || self.max_scroll() <= 0. {
            return false;
        }
        let before = self.scroll_top();
        self.scroll_to(before + delta, false);
        self.scroll_top() != before
    }

    /// Which edges hide cards; none while the pile is out or Clear all runs.
    fn overflow(&self) -> preview::StackOverflow {
        if self.collapsed() || self.progress() > 0. || self.stack.ids().is_empty() {
            return preview::StackOverflow::default();
        }
        preview::stack_overflow(
            f64::from(self.scroll_top()),
            f64::from(self.content_height()),
            f64::from(self.height),
        )
    }

    /// Note when each cue appears, for its pop-in; true when one changed.
    fn sync_cues(&mut self) -> bool {
        let overflow = self.overflow();
        // Fixtures (no card motion) show them already in.
        let now = now_ms()
            - if self.card_motion.is_none() {
                CUE_IN_MS
            } else {
                0.
            };
        let before = self.cues_shown;
        let track = |shown: bool, since: Option<f64>| match (shown, since) {
            (true, None) => Some(now),
            (true, since) => since,
            (false, _) => None,
        };
        self.cues_shown = (
            track(overflow.above, self.cues_shown.0),
            track(overflow.below, self.cues_shown.1),
        );
        before.0.is_some() != self.cues_shown.0.is_some()
            || before.1.is_some() != self.cues_shown.1.is_some()
    }

    /// The overflow cues showing now, with their rects.
    fn cues(&self) -> Vec<(Hit, Rect)> {
        let overflow = self.overflow();
        let cx = PAD + CARD_W / 2.;
        let mut cues = Vec::new();
        if overflow.above {
            cues.push((
                Hit::CueUp,
                rect(
                    cx - CUE_W / 2.,
                    CUE_INSET,
                    cx + CUE_W / 2.,
                    CUE_INSET + CUE_H,
                ),
            ));
        }
        if overflow.below {
            let bottom = self.height - CUE_INSET;
            cues.push((
                Hit::CueDown,
                rect(cx - CUE_W / 2., bottom - CUE_H, cx + CUE_W / 2., bottom),
            ));
        }
        cues
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
        self.highlights.remove(id);
        // A Close or Delete that leaves one card takes the toolbar with the
        // card's exit; Clear all takes it once every card has gone.
        if settles {
            self.sync_toolbar(ToolbarCause::Exit);
        }
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
        self.sync_toolbar(ToolbarCause::Clear);
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
        // The list flies back out to its newest card.
        if !collapsed {
            self.scroll = 0.;
        }
        self.scroll_from = None;
        self.hovered = None;
        self.hot = None;
        self.pile_hovered = false;
        self.set_fan(false);
        self.sync_toolbar(if collapsed {
            ToolbarCause::Collapse
        } else {
            ToolbarCause::Expand
        });
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
            // Shipping highlights a card once its image is ready.
            if image.is_some() {
                self.highlights.insert(entry.id.clone(), now_ms());
            }
        }
        self.cards.push(Card {
            entry,
            image,
            clipboard: false,
            busy: false,
            saved: false,
            error: None,
            dragging: false,
            presence: EditorPresence::default(),
        });
        self.exits.sync(self.stack.ids());
        self.sync_toolbar(ToolbarCause::Other);
        // A new capture brings the newest card back into view.
        self.scroll = 0.;
        self.scroll_from = None;
    }

    fn remove(&mut self, index: usize) {
        if index < self.cards.len() {
            let card = self.cards.remove(index);
            let id = &card.entry.id;
            self.stack.remove(id);
            self.arrivals.remove(id);
            self.highlights.remove(id);
            self.dust.remove(id);
            self.exits.release(id, false);
            self.exits.sync(self.stack.ids());
        }
        self.sync_toolbar(ToolbarCause::Other);
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

    /// Content offset: the anchored end (the newest card) in view, less the
    /// scroll toward older cards.
    fn offset(&self) -> f32 {
        if self.top {
            -self.scroll_now()
        } else {
            self.height - self.content_height() + self.scroll_now()
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

    /// The toolbar takes the pointer, focus and automation: shown, at rest
    /// and over a list that has landed (shipping's `pointer-events: none`
    /// while it enters or leaves).
    fn toolbar(&self) -> bool {
        self.toolbar.shown() && !self.toolbar_moving() && self.progress() <= 0.
    }

    /// The front card of the collapsed pile, the pile's hit target.
    fn front_rect(&self) -> Option<Rect> {
        self.card_rect(self.cards.len().checked_sub(1)?)
    }

    /// Minimize sits beside Clear all, `gap: var(--s-1)`; the right anchor's
    /// `row-reverse` puts it inside Clear all. As "Show less" it grows
    /// inward, away from Clear all, to 92 DIPs.
    fn minimize_rect(&self) -> Rect {
        let clear = self.clear_rect();
        let step = CONTROL + 2.;
        let width = CONTROL
            + (preview::STACK_MINIMIZE_HOVER_WIDTH as f32 - CONTROL) * self.minimize_look()[0];
        if self.right {
            let right = clear.left - 2.;
            rect(right - width, clear.top, right, clear.bottom)
        } else {
            let x = clear.left + step;
            rect(x, clear.top, x + width, clear.bottom)
        }
    }

    fn clear_rect(&self) -> Rect {
        // 8 DIPs from the nearest card, inside the control gutter; the
        // toolbar stays at the window's edge while the cards scroll.
        let y = if self.top {
            GUTTER - 8. - CONTROL
        } else {
            self.height - GUTTER + 8.
        };
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
        controls.push((Hit::Edit(index), self.edit_rect(index, card)));
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
        if let Some((hit, _)) = self.cues().into_iter().find(|(_, r)| inside(r)) {
            return Some(hit);
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
            // An editor's pill and the lingering Edit stay without hover;
            // a leaving one takes no pointer.
            let phase = self.cards[index].presence.phase();
            for (hit, r) in self
                .top_controls(index, card)
                .into_iter()
                .chain(self.main_actions(index, card))
            {
                let edit = matches!(hit, Hit::Edit(_));
                let shown = self.hovered == Some(index) || (edit && phase.pinned());
                if shown && (!edit || phase.interactive()) && inside(&r) {
                    return Some(hit);
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
                self.sparkle_since = self.pile_hovered.then(now_ms);
            }
            return (self.pile_hovered, self.hot) != before;
        }
        self.pile_hovered = false;
        self.sparkle_since = None;
        // Hover follows the card under the pointer, then targets its
        // controls; the toolbar and cues float above scrolled cards.
        let over_stack_control = at.and_then(|(x, y)| self.hit(x, y)).is_some_and(|hit| {
            matches!(
                hit,
                Hit::ClearAll | Hit::Minimize | Hit::CueUp | Hit::CueDown
            )
        });
        let card = at.filter(|_| !over_stack_control).and_then(|(x, y)| {
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
            Hit::CueUp => 4,
            Hit::CueDown => 5,
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
            4 => return Some(Hit::CueUp),
            5 => return Some(Hit::CueDown),
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
                    Hit::Edit(_) => card.presence.phase().accessible_label(),
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
        for (hit, r) in self.cues() {
            let label = preview::overflow_cue_label(hit == Hit::CueUp, self.top);
            items.push(Item::new(Self::uia_key(hit), Role::Button, label, r));
        }
        items
    }

    /// The stack inside its window's bounds, as the window clips it (the
    /// offscreen renderer has no window).
    pub fn paint(&self, dc: &ID2D1DeviceContext, g: &Gfx, t: &Tokens) -> Result<()> {
        let window = rect(0., 0., preview::THUMBNAIL_WIDTH as f32, self.height);
        unsafe { dc.PushAxisAlignedClip(&window, D2D1_ANTIALIAS_MODE_ALIASED) };
        let result = self.paint_stack(dc, g, t);
        unsafe { dc.PopAxisAlignedClip() };
        result
    }

    fn paint_stack(&self, dc: &ID2D1DeviceContext, g: &Gfx, t: &Tokens) -> Result<()> {
        if self.collapsed() || self.progress() > 0. {
            self.paint_pile(dc, g, t)?;
            // The toolbar enters while the pile fans out and dissolves while
            // it gathers.
            self.paint_toolbar(dc, g, t)?;
            self.paint_sparkles(dc, t)?;
            return self.paint_focus(dc, t);
        }
        for index in 0..self.cards.len() {
            if let Some(card) = self.card_rect(index) {
                self.paint_listed(dc, g, t, index, card)?;
            }
        }
        self.paint_toolbar(dc, g, t)?;
        self.paint_cues(dc, g, t)?;
        self.paint_focus(dc, t)?;
        self.paint_tips(dc, g, t)
    }

    /// `.thumbnail-stack-toolbar` under its enter or leave pose: faded,
    /// lowered and shrunk about its centre (`translateY(6px) scale(0.9)`),
    /// or dissolving through `blur(var(--blur-dissolve))` as the stack
    /// collapses.
    fn paint_toolbar(&self, dc: &ID2D1DeviceContext, g: &Gfx, t: &Tokens) -> Result<()> {
        let Some(pose) = self.toolbar_pose() else {
            return Ok(());
        };
        let (clear, minimize) = (self.clear_rect(), self.minimize_rect());
        let cx = (clear.left.min(minimize.left) + clear.right.max(minimize.right)) / 2.;
        let cy = (clear.top + clear.bottom) / 2.;
        let s = pose.scale as f32;
        let mut base = Matrix3x2::identity();
        unsafe { dc.GetTransform(&mut base) };
        let posed = Matrix3x2 {
            M11: s,
            M12: 0.,
            M21: 0.,
            M22: s,
            M31: cx - s * cx + pose.translate_x as f32,
            M32: cy - s * cy + pose.translate_y as f32,
        } * base;
        unsafe { dc.SetTransform(&posed) };
        let result = faded(dc, pose.opacity as f32, || {
            blurred(dc, pose.blur as f32, |dc| {
                for (hit, r) in [(Hit::ClearAll, clear), (Hit::Minimize, minimize)] {
                    self.paint_toolbar_control(dc, g, t, hit, r)?;
                }
                Ok(())
            })
        });
        unsafe { dc.SetTransform(&base) };
        result
    }

    /// `.thumbnail-stack-control`: Clear all's `close` or Minimize's
    /// `preview-stack` on a glass chip.
    fn paint_toolbar_control(
        &self,
        dc: &ID2D1DeviceContext,
        g: &Gfx,
        t: &Tokens,
        hit: Hit,
        r: Rect,
    ) -> Result<()> {
        // Minimize's hover is the "Show less" morph, on the resting glass.
        let hot = self.hot == Some(hit) && hit == Hit::ClearAll;
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
        // `close` is a 24-unit icon at 16 px; `preview-stack` a 16-unit one,
        // so 24 here draws it at 16 px too.
        let (icon, side, swap) = if hit == Hit::ClearAll {
            ("close", 16., 0.)
        } else {
            ("preview-stack", 24., self.minimize_look()[1])
        };
        // `--thumbnail-minimize-slide`: away from the pile's screen edge.
        let slide = if self.right { -1. } else { 1. };
        let (cx, cy) = ((r.left + r.right) / 2., (r.top + r.bottom) / 2.);
        unsafe { dc.PushAxisAlignedClip(&r, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE) };
        let result = (|| {
            // The icon fades, slides back 8 DIPs and shrinks to 0.8 ...
            let mut base = Matrix3x2::identity();
            unsafe { dc.GetTransform(&mut base) };
            let s = 1. - 0.2 * swap;
            let posed = Matrix3x2 {
                M11: s,
                M12: 0.,
                M21: 0.,
                M22: s,
                M31: cx - s * cx - slide * 8. * swap,
                M32: cy - s * cy,
            } * base;
            unsafe { dc.SetTransform(&posed) };
            let drawn = faded(dc, 1. - swap, || {
                gfx::icon(
                    dc,
                    &g.factory,
                    icon,
                    &[],
                    (cx - 8., cy - 8., side),
                    if side > 16. { 1.4 * 1.5 } else { 1.8 },
                    t.color("glass-text"),
                )
            });
            unsafe { dc.SetTransform(&base) };
            drawn?;
            if swap <= 0. {
                return Ok(());
            }
            // ... as "Show less" fades in from 4 DIPs the other way.
            let label = g.text_sized(
                t,
                preview::STACK_MINIMIZE_HOVER_LABEL,
                t.n("text-2xs"),
                "weight-semibold",
                None,
            )?;
            faded(dc, swap, || {
                gfx::text(
                    dc,
                    &label,
                    cx - label.width / 2. + slide * 4. * (1. - swap),
                    cy - label.height / 2.,
                    t.color("glass-text"),
                )
            })
        })();
        unsafe { dc.PopAxisAlignedClip() };
        result
    }

    /// The sparkles run: the pile is hovered, at rest and not being carried
    /// (`:not(.thumbnail-stack-dragging)`), with motion allowed.
    fn sparkling(&self) -> bool {
        self.sparkle_since.is_some()
            && self.card_motion.is_some()
            && !self.reduced
            && self.sway.is_none()
            && self.collapsed()
            && self.progress() >= 1.
    }

    /// `.thumbnail-collapsed-hit-target::before/::after` on the hovered pile:
    /// two layers of accent and white dots over and past the pile
    /// (`preview_motion::sparkle_layer`), each rising 28 DIPs, swelling 6 %
    /// and flashing to 0.9 opacity on its own loop.
    fn paint_sparkles(&self, dc: &ID2D1DeviceContext, t: &Tokens) -> Result<()> {
        let (true, Some(m), Some(since), Some(front)) = (
            self.sparkling(),
            &self.card_motion,
            self.sparkle_since,
            self.front_rect(),
        ) else {
            return Ok(());
        };
        let layer = preview_motion::sparkle_layer(
            (
                front.left.into(),
                front.top.into(),
                (front.right - front.left).into(),
                (front.bottom - front.top).into(),
            ),
            self.top,
        );
        let (cx, cy) = (layer.0 + layer.2 / 2., layer.1 + layer.3 / 2.);
        let accent = t.color("theme-accent");
        let elapsed = now_ms() - since;
        let mut base = Matrix3x2::identity();
        unsafe { dc.GetTransform(&mut base) };
        for (animation, dots) in [
            (m.sparkles[0], &preview_motion::SPARKLES_EARLY[..]),
            (m.sparkles[1], &preview_motion::SPARKLES_LATE[..]),
        ] {
            // `infinite`: the delay runs once, then the loop repeats.
            if elapsed < animation.delay_ms || animation.duration_ms <= 0. {
                continue;
            }
            let looped =
                animation.delay_ms + (elapsed - animation.delay_ms) % animation.duration_ms;
            let pose = animation.pose_at(looped, false);
            if pose.opacity <= 0. {
                continue;
            }
            // Scale about the layer's centre, then rise.
            let s = pose.scale as f32;
            let posed = Matrix3x2 {
                M11: s,
                M12: 0.,
                M21: 0.,
                M22: s,
                M31: (cx - f64::from(s) * cx) as f32,
                M32: (cy + pose.translate_y - f64::from(s) * cy) as f32,
            } * base;
            unsafe { dc.SetTransform(&posed) };
            let result = faded(dc, pose.opacity as f32, || {
                for dot in dots {
                    let (x, y) = preview_motion::sparkle_center(dot, layer);
                    let color = if dot.accent {
                        accent
                    } else {
                        D2D1_COLOR_F {
                            r: 1.,
                            g: 1.,
                            b: 1.,
                            a: 1.,
                        }
                    };
                    sparkle_dot(dc, (x as f32, y as f32), dot, color)?;
                }
                Ok(())
            });
            unsafe { dc.SetTransform(&base) };
            result?;
        }
        Ok(())
    }

    /// `.thumbnail-overflow-cue`: a glass tab flush with its window edge,
    /// rounded away from it, popping in (`thumbnail-overflow-cue-in`), a
    /// little larger while hovered and smaller while pressed. The backdrop
    /// blur is drawn as the solid glass, like the toolbar.
    fn paint_cues(&self, dc: &ID2D1DeviceContext, g: &Gfx, t: &Tokens) -> Result<()> {
        let now = now_ms();
        let ease = captures_app::motion::CubicBezier::new(0.16, 1., 0.3, 1.)
            .unwrap_or(captures_app::motion::CubicBezier::LINEAR);
        for (hit, r) in self.cues() {
            let up = hit == Hit::CueUp;
            let since = if up {
                self.cues_shown.0
            } else {
                self.cues_shown.1
            };
            let t_in = match since {
                Some(start) if !self.reduced => ((now - start) / CUE_IN_MS).clamp(0., 1.),
                _ => 1.,
            };
            let arrive = ease.ease(t_in) as f32;
            let hot = self.hot == Some(hit);
            let scale = (0.86 + 0.14 * arrive)
                * if hot && self.pressed == Some(hit) {
                    0.95
                } else if hot {
                    1.05
                } else {
                    1.
                };
            let mut base = Matrix3x2::identity();
            unsafe { dc.GetTransform(&mut base) };
            let (cx, cy) = ((r.left + r.right) / 2., (r.top + r.bottom) / 2.);
            let posed = Matrix3x2 {
                M11: scale,
                M12: 0.,
                M21: 0.,
                M22: scale,
                M31: cx - scale * cx,
                M32: cy - scale * cy,
            } * base;
            unsafe { dc.SetTransform(&posed) };
            let result = faded(dc, arrive, || {
                let radius = t.n("r-lg");
                // `--glass-shadow`'s 44 px blur bands visibly through the
                // stepped `gfx::shadow` on a tab this small; the main
                // actions' shadow reads the same without the rings.
                gfx::shadow(dc, r, radius, t.shadow("shadow-md"))?;
                // Square on the window edge: draw the rounded shape past it
                // and clip it there, then close the flat side's border.
                let grown = if up {
                    rect(r.left, r.top - radius, r.right, r.bottom)
                } else {
                    rect(r.left, r.top, r.right, r.bottom + radius)
                };
                unsafe { dc.PushAxisAlignedClip(&r, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE) };
                let fill = if hot {
                    "glass-raised-solid"
                } else {
                    "glass-strong-solid"
                };
                let drawn = gfx::fill_round(dc, grown, radius, t.color(fill))
                    .and_then(|()| gfx::border(dc, grown, radius, 1., t.color("glass-border")));
                unsafe { dc.PopAxisAlignedClip() };
                drawn?;
                let edge = if up {
                    rect(r.left, r.top, r.right, r.top + 1.)
                } else {
                    rect(r.left, r.bottom - 1., r.right, r.bottom)
                };
                gfx::fill(dc, edge, t.color("glass-border"))?;
                // A 16-unit chevron at 16 px, stroke 2.
                let icon = if up {
                    "preview-overflow-up"
                } else {
                    "preview-overflow-down"
                };
                gfx::icon(
                    dc,
                    &g.factory,
                    icon,
                    &[],
                    (cx - 8., cy - 8., 24.),
                    2. * 1.5,
                    t.color("glass-text"),
                )
            });
            unsafe { dc.SetTransform(&base) };
            result?;
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
        let gravity = self.gravity;
        // From the list where it was scrolled to the pile at rest.
        let pile = if top {
            0.
        } else {
            f64::from(self.height) - preview::collapsed_frame_height(count)
        };
        let scrolled = f64::from(self.scroll).clamp(
            0.,
            (preview::stack_height(count) - f64::from(self.height)).max(0.),
        );
        let list = if top {
            -scrolled
        } else {
            f64::from(self.height) - preview::stack_height(count) + scrolled
        };
        let offset = lerp(list, pile, progress);
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
            // A carried pile stays fanned and its rear cards lean with the
            // carry (`thumbnail-stack-drag-sway`).
            let pose = match &self.sway {
                Some(sway) => {
                    preview::collapsed_card_sway_pose(id, depth, gravity, top, sway.offset())
                }
                None => mix(rest, fanned, fan),
            };
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
        let mut blur = 0.;
        let mut fade = Fade::NONE;
        let mut dust = None;
        if let Some(m) = &self.card_motion {
            if let Some(start) = self.arrivals.get(id) {
                let pose = m.arrive.pose_at(now - start, self.reduced);
                opacity *= pose.opacity;
                dy += pose.translate_y;
                scale *= pose.scale;
                // `thumbnail-arrive` rises in from `blur(3px)`.
                blur = pose.blur as f32;
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
        if let Some(start) = self.rejects.get(id) {
            let x = [1, 2, 3, 4].map(|n| t.n(&format!("thumbnail-drop-reject-x-{n}")));
            dx += f64::from(reject_offset(x, now - start));
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
            blurred(dc, blur, |dc| self.paint_card(dc, g, t, index, r, fade))
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
        // A shaking card shows its image undimmed (`.thumbnail-drop-rejected img`).
        let [mut filter, mut zoom, mut controls, mut meta] = self.hover_look(index);
        if self.rejects.contains_key(&card.entry.id) {
            (filter, zoom) = (0., 0.);
        }
        // A Close locks the hover look it was pressed in; Clear all keeps
        // each card's own (`--thumbnail-clear-image-filter`).
        let exit = self.exits.exiting(&card.entry.id);
        if exit.is_some_and(|exit| exit.settles) {
            (filter, zoom, meta) = (1., 1., 1.);
        }
        let mut streak = (1., 0.);
        if let (Some(exit), Some(m)) = (exit, &self.card_motion)
            && exit.kind == ExitKind::Dismiss
        {
            let pose = m
                .streak
                .pose_at(exit.elapsed_ms(now_ms()).max(0.), self.reduced);
            // The uniform hover zoom eases into the horizontal stretch.
            zoom *=
                ((pose.scale - 1.) / (preview_chrome::HOVER_MEDIA_SCALE - 1.)).clamp(0., 1.) as f32;
            let sigma = streak_blur(pose.blur);
            if sigma > 0. && exit.settles {
                filter = 0.;
            }
            streak = (pose.scale_x as f32, sigma);
        }
        if self.dust.contains_key(&card.entry.id) {
            controls = 1.;
        }
        let radius = t.n("thumbnail-card-radius");
        // `--thumbnail-card-shadow` and the 1 px white ring.
        faded(dc, fade.frame, || {
            let shadow = || gfx::shadow(dc, r, radius, t.shadow("thumbnail-card-shadow"));
            if card.dragging {
                outside(dc, (r, radius), shadow)
            } else {
                shadow()
            }
        })?;
        let white = |a: f32| D2D1_COLOR_F {
            r: 1.,
            g: 1.,
            b: 1.,
            a,
        };
        faded(dc, fade.source, || {
            // A dragged card's media hides and leaves it hollow
            // (`.thumbnail-file-dragging`; macOS clears its background).
            if card.dragging {
                return Ok(());
            }
            gfx::fill_round(dc, r, radius, t.color("glass-strong-solid"))?;
            if let Some(image) = &card.image {
                cover(dc, image, (r, radius), (filter, zoom), streak)?;
            }
            if filter > 0. {
                // `brightness(.5)` on the opaque image.
                gfx::fill_round(
                    dc,
                    r,
                    radius,
                    D2D1_COLOR_F {
                        r: 0.,
                        g: 0.,
                        b: 0.,
                        a: (1. - preview_chrome::HOVER_MEDIA_BRIGHTNESS as f32) * filter,
                    },
                )?;
            }
            Ok(())
        })?;
        faded(dc, fade.frame, || {
            gfx::ring(dc, r, radius, 0., 1., white(0.08))?;
            self.paint_editor_ring(dc, t, index, (r, radius))?;
            self.paint_highlight(dc, t, index, (r, radius))
        })?;
        faded(dc, fade.chrome, || {
            self.paint_chrome(dc, g, t, index, r, (1. - meta, controls))?;
            paint_message(dc, g, t, card.error.as_deref(), r)
        })
    }

    /// `.thumbnail-capture-highlight::after`: a 1 px ring of 85 % accent and
    /// an 18 px glow of 24 % accent around a new card, held, then faded
    /// (`thumbnail-capture-highlight`). A Delete fades it with the frame.
    fn paint_highlight(
        &self,
        dc: &ID2D1DeviceContext,
        t: &Tokens,
        index: usize,
        (r, radius): (Rect, f32),
    ) -> Result<()> {
        let (Some(m), Some(start)) = (
            &self.card_motion,
            self.highlights.get(&self.cards[index].entry.id),
        ) else {
            return Ok(());
        };
        let opacity = m.highlight.pose_at(now_ms() - start, self.reduced).opacity as f32;
        if opacity <= 0. {
            return Ok(());
        }
        let accent = t.color("theme-accent");
        let glow = Shadow {
            x: 0.,
            y: 0.,
            blur: 18.,
            spread: 0.,
            color: [accent.r, accent.g, accent.b, 0.24 * opacity],
        };
        outside(dc, (r, radius), || gfx::shadow(dc, r, radius, &[glow]))?;
        gfx::ring(
            dc,
            r,
            radius,
            0.,
            1.,
            D2D1_COLOR_F {
                a: 0.85 * opacity,
                ..accent
            },
        )
    }

    /// The bottom bar's chips and, while `controls`, the hover controls.
    fn paint_chrome(
        &self,
        dc: &ID2D1DeviceContext,
        g: &Gfx,
        t: &Tokens,
        index: usize,
        r: Rect,
        (meta, controls): (f32, f32),
    ) -> Result<()> {
        let card = &self.cards[index];
        // `.thumbnail-bottom-bar`: the meta chip (fading out on hover) and
        // the clipboard confirmation.
        let bottom = r.bottom - 8.;
        faded(dc, meta, || {
            if meta <= 0. {
                return Ok(());
            }
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
            )
        })?;
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
        // An editor's pill and the lingering Edit show without hover.
        if card.presence.phase().pinned() && !card.recording() {
            self.paint_edit(dc, g, t, index, self.edit_rect(index, r))?;
        }
        if controls <= 0. {
            return Ok(());
        }
        faded(dc, controls, || {
            self.paint_controls(dc, g, t, index, r, controls)
        })
    }

    /// `.thumbnail-editor-control` pinned by an editor: the compact Edit
    /// icon, or as it grows the `.is-present` pill, an accent-outlined
    /// `--r-pill` chip reading "In editor" in accent type, filled with
    /// accent and reading "Show in editor" while hovered or focused (not
    /// right after its own click opened the editor).
    fn paint_edit(
        &self,
        dc: &ID2D1DeviceContext,
        g: &Gfx,
        t: &Tokens,
        index: usize,
        b: Rect,
    ) -> Result<()> {
        let card = &self.cards[index];
        let (grown, _) = self.editor_look(index);
        let phase = card.presence.phase();
        let pointed = phase.interactive()
            && (self.hot == Some(Hit::Edit(index)) || self.focus == Some(Hit::Edit(index)));
        let just_opened = self.just_opened.as_deref() == Some(card.entry.id.as_str());
        let filled = phase.present() && pointed && !just_opened;
        let lerp = |a: D2D1_COLOR_F, b: D2D1_COLOR_F| D2D1_COLOR_F {
            r: a.r + (b.r - a.r) * grown,
            g: a.g + (b.g - a.g) * grown,
            b: a.b + (b.b - a.b) * grown,
            a: a.a + (b.a - a.a) * grown,
        };
        let accent = t.color("theme-accent");
        // `.thumbnail-editor-control:hover` lifts the compact face 1 DIP.
        let lift = if pointed && grown <= 0. { -1. } else { 0. };
        let b = rect(b.left, b.top + lift, b.right, b.bottom + lift);
        let radius = t.n("r-md") + (CONTROL / 2. - t.n("r-md")) * grown;
        gfx::shadow(dc, b, radius, t.shadow("shadow-sm"))?;
        let glow = Shadow {
            x: 0.,
            y: 0.,
            blur: 14.,
            spread: 0.,
            color: [accent.r, accent.g, accent.b, 0.2 * grown],
        };
        gfx::shadow(dc, b, radius, &[glow])?;
        let fill = if filled {
            accent
        } else if pointed {
            t.color("glass-raised")
        } else {
            t.color("glass-strong")
        };
        gfx::fill_round(dc, b, radius, fill)?;
        if !filled {
            let edge = lerp(t.color("glass-border"), D2D1_COLOR_F { a: 0.72, ..accent });
            gfx::border(dc, b, radius, 1., edge)?;
        }
        let ink = if filled {
            t.color("theme-accent-ink")
        } else {
            lerp(t.color("glass-text"), t.color("theme-accent-text"))
        };
        // The 16 px icon shrinks to 11 px (stroke 2.2) at the pill's
        // 7 px padding inside its 1 px border.
        let side = 16. - 5. * grown;
        let x = b.left + 6. + 2. * grown;
        let cy = (b.top + b.bottom) / 2.;
        unsafe { dc.PushAxisAlignedClip(&b, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE) };
        let result = gfx::icon(
            dc,
            &g.factory,
            "edit",
            &[],
            (x, cy - side / 2., side),
            1.8 + 0.4 * grown,
            ink,
        )
        .and_then(|()| {
            if grown <= 0. {
                return Ok(());
            }
            let label = preview_chrome::editor_pill_label(pointed, just_opened);
            let text = g.text_sized(t, label, t.n("text-2xs"), "weight-semibold", None)?;
            faded(dc, grown, || {
                gfx::text(
                    dc,
                    &text,
                    x + side + preview_chrome::EDITOR_PILL_GAP as f32,
                    cy - text.height / 2.,
                    ink,
                )
            })
        });
        unsafe { dc.PopAxisAlignedClip() };
        result
    }

    /// `.thumbnail-editor-active`: a 2 px ring of 90 % accent and a 14 px
    /// glow of 28 % accent around a card its editor shows, eased out as the
    /// editor closes.
    fn paint_editor_ring(
        &self,
        dc: &ID2D1DeviceContext,
        t: &Tokens,
        index: usize,
        (r, radius): (Rect, f32),
    ) -> Result<()> {
        let (_, strength) = self.editor_look(index);
        if strength <= 0. {
            return Ok(());
        }
        let accent = t.color("theme-accent");
        let glow = Shadow {
            x: 0.,
            y: 0.,
            blur: 14.,
            spread: 0.,
            color: [accent.r, accent.g, accent.b, 0.28 * strength],
        };
        outside(dc, (r, radius), || gfx::shadow(dc, r, radius, &[glow]))?;
        gfx::ring(
            dc,
            r,
            radius,
            0.,
            2.,
            D2D1_COLOR_F {
                a: 0.9 * strength,
                ..accent
            },
        )
    }

    /// The hover controls at fade `progress`: the top row slides down 3 DIPs
    /// as it fades in (`.thumbnail-top-actions`).
    fn paint_controls(
        &self,
        dc: &ID2D1DeviceContext,
        g: &Gfx,
        t: &Tokens,
        index: usize,
        r: Rect,
        progress: f32,
    ) -> Result<()> {
        let card = &self.cards[index];
        let white = |a: f32| D2D1_COLOR_F {
            r: 1.,
            g: 1.,
            b: 1.,
            a,
        };
        let slide = -3. * (1. - progress);
        for (hit, b) in self.top_controls(index, r) {
            // A pinned Edit draws on its own, at full strength.
            if matches!(hit, Hit::Edit(_)) && card.presence.phase().pinned() {
                continue;
            }
            let hot = self.hot == Some(hit) && !card.busy;
            let lift = slide + if hot { -1. } else { 0. };
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
    use captures_settings::MiniPreviewPlacement as P;
    let placement = if select == Some("pile-top") {
        P::TopLeft
    } else {
        P::BottomLeft
    };
    let mut stack = Stack::new(placement);
    let pile = matches!(
        select,
        Some("pile" | "pile-hover" | "pile-top" | "pile-sparkle")
    );
    let overflow = select == Some("overflow");
    let mut cards = vec![
        ("first", (1600, 1000, 842_000), true),
        ("second", (1200, 900, 432_000), false),
    ];
    if pile || overflow {
        cards.insert(0, ("zeroth", (1920, 1080, 1_204_000), false));
    }
    if overflow {
        cards.insert(0, ("older", (1280, 800, 610_000), false));
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
    stack.sync_toolbar(ToolbarCause::Other);
    stack.height = stack.content_height();
    match select {
        // The toolbar 400 ms into entering with an expanding stack, or 90 ms
        // into dissolving (on the list, to show its blur).
        Some(select @ ("toolbar-in" | "toolbar-out")) => {
            stack.motion_tokens = Some(Tokens::variant("dark", "mustard"));
            let now = now_ms();
            if select == "toolbar-in" {
                stack.toolbar = StackToolbar::default();
                stack.toolbar.set(true, ToolbarCause::Expand, now - 400.);
            } else {
                stack.toolbar.set(false, ToolbarCause::Collapse, now - 90.);
            }
        }
        Some("pile-hover") => {
            let front = stack.front_rect().expect("front card");
            stack.pointer(Some((front.left + 40., front.top + 40.)));
        }
        // The hovered pile 1.2 s into its sparkles: both layers showing.
        Some("pile-sparkle") => {
            stack.card_motion = CardMotion::resolve(&Tokens::variant("dark", "mustard"));
            let front = stack.front_rect().expect("front card");
            stack.pointer(Some((front.left + 40., front.top + 40.)));
            stack.sparkle_since = Some(now_ms() - 1_200.);
        }
        // A hovered saved recording above an unsaved card whose Save failed.
        Some("recording") => {
            stack.cards[0].entry.kind = captures_history::ArtifactKind::Video;
            stack.cards[0].clipboard = false;
            stack.cards[1].error =
                Some("Could not save: the Captures folder is not writable".into());
            stack.pointer(Some((150., 90.)));
        }
        // Four cards in a 560 DIP window, scrolled one slot from the newest:
        // both cues show.
        Some("overflow") => {
            stack.height = 560.;
            stack.scroll = preview::THUMBNAIL_CARD_SLOT as f32;
        }
        // The pointer on the hovered card's Delete, or on Clear all: their
        // tooltips at rest.
        Some(select @ ("tooltip" | "clear-tip")) => {
            stack.pointer(Some((150., 90.)));
            let at = if select == "tooltip" {
                let hovered = stack.hovered.expect("a hovered card");
                let card = stack.card_rect(hovered).expect("card");
                stack
                    .top_controls(hovered, card)
                    .into_iter()
                    .find(|(hit, _)| matches!(hit, Hit::Delete(_)))
                    .map(|(_, r)| r)
                    .expect("Delete")
            } else {
                stack.clear_rect()
            };
            stack.pointer(Some((at.left + 4., at.top + 4.)));
            stack.tips = stack.tip_targets().map(Tip::settled);
        }
        // An editor shows the saved card: its "In editor" pill and ring, or
        // with the pointer on the pill, "Show in editor".
        Some(select @ ("in-editor" | "show-in-editor")) => {
            stack.measure_pill(g, &Tokens::variant("dark", "mustard"));
            stack.cards[0].presence = EditorPresence::new(true, now_ms());
            if select == "show-in-editor" {
                let card = stack.card_rect(0).expect("card");
                let pill = stack.edit_rect(0, card);
                stack.pointer(Some((pill.left + 20., pill.top + 10.)));
            }
        }
        // The pointer on Minimize, which reads "Show less".
        Some("show-less") => {
            let at = stack.minimize_rect();
            stack.pointer(Some((at.left + 4., at.top + 4.)));
        }
        // The hovered newest card 160 ms into its Close: sliding and fading
        // out while its image streaks.
        Some("streak") => {
            stack.card_motion = CardMotion::resolve(&Tokens::variant("dark", "mustard"));
            let newest = stack.cards[last].entry.id.clone();
            stack.leave(&newest, ExitKind::Dismiss, 0., true);
            // Exits cannot start in the past.
            std::thread::sleep(std::time::Duration::from_millis(160));
        }
        // The newest card 90 ms into `thumbnail-arrive`: faded, lowered and
        // blurred.
        Some("arrive") => {
            stack.card_motion = CardMotion::resolve(&Tokens::variant("dark", "mustard"));
            let newest = stack.cards[last].entry.id.clone();
            stack.arrivals.insert(newest, now_ms() - 90.);
        }
        // The newest card inside its capture highlight's 1 s hold.
        Some("highlight") => {
            stack.card_motion = CardMotion::resolve(&Tokens::variant("dark", "mustard"));
            let newest = stack.cards[last].entry.id.clone();
            stack.highlights.insert(newest, now_ms());
        }
        // Keyboard focus on the first card's Edit, with no pointer: the card
        // shows its controls, the focus ring and Edit's tooltip.
        Some("focus") => {
            stack.focus = Some(Hit::Edit(0));
            stack.tips = stack.tip_targets().map(Tip::settled);
        }
        // Keyboard focus on the pile's Expand.
        Some("pile-focus") => {
            stack.stack.set_collapsed(true);
            stack.focus = Some(Hit::Expand);
        }
        // The hovered card's file is being dragged out: its image hides.
        Some("file-drag") => {
            stack.pointer(Some((150., 90.)));
            let hovered = stack.hovered.expect("a hovered card");
            stack.cards[hovered].dragging = true;
        }
        Some("pile" | "pile-top") | None => {}
        Some(_) => {
            stack.pointer(Some((150., 90.)));
        }
    }
    stack.sync_minimize(now_ms());
    stack.sync_cues();
    stack
}

/// `.icon-button::after` and `.thumbnail-stack-control[data-tooltip]::after`:
/// a `--glass-strong` chip with `--shadow-sm`, `--r-xs` corners and 4 × 7
/// padding around `--text-2xs` medium copy, 6 DIPs from its control, sliding
/// 2 DIPs toward it as it fades in (`preview_chrome::icon_tooltip_frame`).
fn paint_tip(
    dc: &ID2D1DeviceContext,
    g: &Gfx,
    t: &Tokens,
    label: &str,
    anchor: Rect,
    above: bool,
    progress: f32,
) -> Result<()> {
    let text = g.text_sized(t, label, t.n("text-2xs"), "weight-medium", None)?;
    let f = preview_chrome::icon_tooltip_frame(
        LogicalRect::new(
            anchor.left.into(),
            anchor.top.into(),
            (anchor.right - anchor.left).into(),
            (anchor.bottom - anchor.top).into(),
        ),
        text.width.into(),
        text.height.into(),
        above,
        progress.into(),
    );
    let r = rect(
        f.x as f32,
        f.y as f32,
        (f.x + f.width) as f32,
        (f.y + f.height) as f32,
    );
    let radius = t.n("r-xs");
    faded(dc, progress, || {
        gfx::shadow(dc, r, radius, t.shadow("shadow-sm"))?;
        gfx::fill_round(dc, r, radius, t.color("glass-strong"))?;
        gfx::text(
            dc,
            &text,
            r.left + preview_chrome::ICON_TOOLTIP_PADDING_X as f32,
            r.top + preview_chrome::ICON_TOOLTIP_PADDING_Y as f32,
            t.color("glass-text"),
        )
    })
}

/// One sparkle dot: `radial-gradient(circle, color 0 core, transparent
/// fade)`, solid to its core radius, then fading out by its fade radius.
fn sparkle_dot(
    dc: &ID2D1DeviceContext,
    (x, y): (f32, f32),
    dot: &preview_motion::SparkleDot,
    color: D2D1_COLOR_F,
) -> Result<()> {
    let (core, fade) = (dot.core as f32, dot.fade as f32);
    let solid = D2D1_COLOR_F {
        a: color.a * dot.alpha as f32,
        ..color
    };
    let clear = D2D1_COLOR_F { a: 0., ..solid };
    let target: &ID2D1RenderTarget = dc;
    unsafe {
        let stops = target.CreateGradientStopCollection(
            &[
                D2D1_GRADIENT_STOP {
                    position: 0.,
                    color: solid,
                },
                D2D1_GRADIENT_STOP {
                    position: core / fade,
                    color: solid,
                },
                D2D1_GRADIENT_STOP {
                    position: 1.,
                    color: clear,
                },
            ],
            D2D1_GAMMA_2_2,
            D2D1_EXTEND_MODE_CLAMP,
        )?;
        let center = windows_numerics::Vector2 { X: x, Y: y };
        let brush = dc.CreateRadialGradientBrush(
            &D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES {
                center,
                gradientOriginOffset: windows_numerics::Vector2 { X: 0., Y: 0. },
                radiusX: fade,
                radiusY: fade,
            },
            None,
            &stops,
        )?;
        dc.FillEllipse(
            &D2D1_ELLIPSE {
                point: center,
                radiusX: fade,
                radiusY: fade,
            },
            &brush,
        );
    }
    Ok(())
}

/// Run `paint` masked to outside the rounded `r`, so a hollow card's shadow
/// only shows around it, as a CSS `box-shadow` does.
fn outside(
    dc: &ID2D1DeviceContext,
    (r, radius): (Rect, f32),
    paint: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let pad = 64.;
    let bounds = rect(r.left - pad, r.top - pad, r.right + pad, r.bottom + pad);
    unsafe {
        let factory = dc.GetFactory()?;
        let frame: ID2D1Geometry = factory.CreateRectangleGeometry(&bounds)?.cast()?;
        let card: ID2D1Geometry = factory
            .CreateRoundedRectangleGeometry(&D2D1_ROUNDED_RECT {
                rect: r,
                radiusX: radius,
                radiusY: radius,
            })?
            .cast()?;
        let hole =
            factory.CreateGeometryGroup(D2D1_FILL_MODE_ALTERNATE, &[Some(frame), Some(card)])?;
        let mut layer = D2D1_LAYER_PARAMETERS1 {
            contentBounds: bounds,
            geometricMask: std::mem::ManuallyDrop::new(Some(hole.cast()?)),
            maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
            maskTransform: Matrix3x2::identity(),
            opacity: 1.,
            ..Default::default()
        };
        dc.PushLayer(&layer, None::<&ID2D1Layer>);
        let result = paint();
        dc.PopLayer();
        // Release the mask reference the parameters own.
        std::mem::ManuallyDrop::drop(&mut layer.geometricMask);
        result
    }
}

/// The horizontal blur a Close streak shows for its pose's `blur`: shipping
/// steps through `#thumbnail-motion-blur-a/b/c` (`stdDeviation` 3.5, 8 and
/// 14 on x), each swapping in at its keyframe's midpoint as CSS does for
/// filters it cannot interpolate (GTK's `streak_step`). 0 before the first.
fn streak_blur(blur: f64) -> f32 {
    let mut previous = preview_chrome::HOVER_MEDIA_BLUR;
    let mut shown = 0.;
    for sigma in [3.5, 8., 14.] {
        if blur >= (previous + sigma) / 2. {
            shown = sigma;
        }
        previous = sigma;
    }
    shown as f32
}

/// `object-fit: cover` into the rounded `r`; hovered cards scale by 1.015
/// and blur by 2 px (`.thumbnail-card:hover img`), `filter` and `zoom` of
/// the way there. A Close streak stretches it by `stretch` about its centre
/// under a horizontal blur of standard deviation `streak` (shipping's
/// `feGaussianBlur stdDeviation="N 0"`), in place of the hover blur.
/// Without the blur effects (Wine) the image is drawn sharp.
fn cover(
    dc: &ID2D1DeviceContext,
    image: &ID2D1Bitmap1,
    (r, radius): (Rect, f32),
    (filter, zoom): (f32, f32),
    (stretch, streak): (f32, f32),
) -> Result<()> {
    let size = unsafe { image.GetSize() };
    if size.width <= 0. || size.height <= 0. {
        return Ok(());
    }
    let (w, h) = (r.right - r.left, r.bottom - r.top);
    let mut k = (w / size.width).max(h / size.height);
    k *= 1. + (preview_chrome::HOVER_MEDIA_SCALE as f32 - 1.) * zoom;
    let kx = k * stretch;
    let hovered = filter > 0. || streak > 0.;
    let placed = Matrix3x2 {
        M11: kx,
        M12: 0.,
        M21: 0.,
        M22: k,
        M31: r.left + (w - size.width * kx) / 2.,
        M32: r.top + (h - size.height * k) / 2.,
    };
    let shape = D2D1_ROUNDED_RECT {
        rect: r,
        radiusX: radius,
        radiusY: radius,
    };
    unsafe {
        let blurred = hovered.then(|| -> Result<ID2D1Brush> {
            // Image pixels: the brush scales the effect's output.
            let (effect, deviation, border) = if streak > 0. {
                (
                    dc.CreateEffect(&CLSID_D2D1DirectionalBlur)?,
                    streak / kx,
                    D2D1_DIRECTIONALBLUR_PROP_BORDER_MODE.0,
                )
            } else {
                (
                    dc.CreateEffect(&CLSID_D2D1GaussianBlur)?,
                    preview_chrome::HOVER_MEDIA_BLUR as f32 * filter / k,
                    D2D1_GAUSSIANBLUR_PROP_BORDER_MODE.0,
                )
            };
            effect.SetInput(0, image, true);
            // Both effects' standard deviation is property 0.
            effect.SetValue(
                D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION.0 as u32,
                D2D1_PROPERTY_TYPE_FLOAT,
                &deviation.to_le_bytes(),
            )?;
            effect.SetValue(
                border as u32,
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
    DragReady(String, std::result::Result<crate::drag::Payload, String>),
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
    /// The stack's display, for carrying the pile on it.
    bounds: Option<preview::ThumbnailMonitorBounds>,
    /// Where the pile was carried this session, and the corner it now
    /// opens toward; forgotten once the stack empties (shipping's
    /// session-only dragged origin).
    dragged: Option<(
        preview::ThumbnailStackOrigin,
        captures_settings::MiniPreviewPlacement,
    )>,
    /// A press on the pile: where in the window (DIPs) and on the screen
    /// (px), and whether it moved far enough to carry instead of expand.
    carry: Option<Carry>,
    /// A press on an expanded card's image: its id and where on the screen
    /// (px), until it moves far enough to drag the file or lets go.
    drag_press: Option<(String, POINT)>,
    /// The card whose file drag is preparing or running.
    dragging: Option<String>,
    /// The prepared file, waiting for the drag loop outside the borrow.
    drag_ready: Option<crate::drag::Payload>,
}

#[derive(Clone, Copy)]
struct Carry {
    press: (f64, f64),
    screen: POINT,
    carrying: bool,
    /// The last pointer sample (px) and its time, for the lean.
    last: (POINT, f64),
}

/// Shipping `THUMBNAIL_STACK_DRAG_THRESHOLD_PX`: past it a press on the pile
/// carries it instead of expanding.
const CARRY_THRESHOLD: f64 = 8.;

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
            // Clicks never take focus (shipping's `focusable(false)` window);
            // assistive tech can still move keyboard focus here.
            focusable: true,
            items: {
                let focused = previews.stack.focus.map(Stack::uia_key);
                let mut items = previews.stack.uia_items();
                for item in &mut items {
                    item.focused = Some(item.key) == focused;
                }
                items
            },
        })
    })
}

/// Invoke a card button or Clear all through the click path, or move
/// keyboard focus to one.
pub fn uia_act(key: u32, act: panel_uia::Act) -> Option<()> {
    let hwnd = PREVIEWS.with(|p| {
        let mut p = p.try_borrow_mut().ok()?;
        let previews = p.as_mut()?;
        let Some(hit) = Stack::uia_hit(key) else {
            return Some(None);
        };
        Some(match act {
            panel_uia::Act::Invoke => {
                previews.run(hit);
                None
            }
            panel_uia::Act::Focus => {
                previews.stack.focus = Some(hit);
                let _ = previews.render();
                previews.animate();
                Some(previews.hwnd)
            }
            _ => None,
        })
    })?;
    // Outside the borrow: activating sends WM_SETFOCUS here.
    if let Some(hwnd) = hwnd {
        unsafe {
            let _ = SetForegroundWindow(hwnd);
            let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(Some(hwnd));
        }
    }
    Some(())
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
        unsafe { SetTimer(Some(hwnd), PRESENCE_TIMER, PRESENCE_INTERVAL_MS, None) };
        let (sender, results) = channel();
        Ok(Self {
            hwnd,
            stack: {
                let mut stack = Stack::new(settings.mini_preview_placement);
                stack.motion = Transition::PreviewStackFly
                    .resolve(&tokens)
                    .zip(Transition::PreviewStackFan.resolve(&tokens));
                stack.card_motion = CardMotion::resolve(&tokens);
                stack.motion_tokens = Some(tokens.clone());
                stack.measure_pill(&g, &tokens);
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
            bounds: None,
            dragged: None,
            carry: None,
            drag_press: None,
            dragging: None,
            drag_ready: None,
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
        // A carried pile stays where it was put for the session.
        if self.dragged.is_none() || self.monitor.is_invalid() {
            unsafe {
                let mut cursor = POINT::default();
                let _ = GetCursorPos(&mut cursor);
                self.monitor = MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST);
            }
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
            // A carried pile opens toward the corner it was carried to.
            let placement = match self.dragged {
                Some((_, placement)) => placement,
                None => {
                    captures_settings::load(&self.options.settings_file)
                        .unwrap_or_default()
                        .mini_preview_placement
                }
            };
            let origin = self.dragged.map(|(origin, _)| origin);
            let count = self.stack.cards.len();
            let geometry = preview::thumbnail_geometry(
                bounds,
                count,
                self.stack.collapsed(),
                origin,
                placement,
            );
            self.bounds = Some(bounds);
            // The collapsed frame reports a bottom anchor; the pile still
            // peeks and opens toward its corner.
            self.stack.top = placement.is_top();
            self.stack.right = placement.is_right();
            self.stack.gravity = preview::collapsed_stack_gravity(bounds, count, origin, placement);
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
        self.stack.sync_cues();
        self.stack.sync_focus();
        self.stack.sync_hover(now_ms());
        self.stack.sync_minimize(now_ms());
        self.stack.sync_tips(&self.tokens, crate::tooltip::now_ms());
        let (stack, g, t) = (&self.stack, &self.g, &self.tokens);
        gfx::draw_surface(surface, self.px, (self.scale, self.scale), 0., |dc| {
            stack.paint(dc, g, t)
        })?;
        unsafe { self.g.dcomp.as_ref().ok_or_else(Error::empty)?.Commit() }
    }

    /// The stack changed size: refit, or hide once it is empty.
    fn relayout(&mut self) -> Result<()> {
        if self.stack.cards.is_empty() {
            // An emptied stack forgets where its pile was carried.
            self.dragged = None;
            self.sync_visibility();
            return Ok(());
        }
        self.place()?;
        self.render()
    }

    fn run(&mut self, hit: Hit) {
        if matches!(hit, Hit::CueUp | Hit::CueDown) {
            self.stack.reduced = crate::appearance::reduced_motion();
            self.stack
                .scroll_slots(if hit == Hit::CueUp { -1 } else { 1 });
            let _ = self.render();
            self.animate();
            return;
        }
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
            // While present, the pill brings that editor forward.
            Hit::Edit(_) if !recording => match crate::editor::open(&self.options, &id) {
                Ok(()) => {
                    if !self.stack.cards[index].presence.phase().present() {
                        self.stack.just_opened = Some(id.clone());
                    }
                    self.stack.sync_presence(&crate::editor::ids(), now_ms());
                    self.animate();
                }
                Err(error) => self.stack.cards[index].error = Some(error.message()),
            },
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
            | Hit::Expand
            | Hit::CueUp
            | Hit::CueDown => {}
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

    /// Follow the open editors: a card's pill grows when one shows it and
    /// shrinks, then lingers as the Edit icon, when it closes.
    fn sync_presence(&mut self) {
        if self.stack.cards.is_empty() {
            return;
        }
        self.stack.reduced = crate::appearance::reduced_motion();
        if self.stack.sync_presence(&crate::editor::ids(), now_ms()) {
            let _ = self.render();
            self.animate();
        }
    }

    /// The stack's motion or a tooltip fade still runs.
    fn moving(&self) -> bool {
        self.stack.animating()
            || self
                .stack
                .tips_animating(&self.tokens, crate::tooltip::now_ms())
    }

    /// Tick the frame timer while the fly or fan runs.
    fn animate(&self) {
        if self.moving() {
            unsafe {
                SetTimer(Some(self.hwnd), MOTION_TIMER, 16, None);
            }
        }
    }

    /// The pointer moved while the pile is pressed: past the threshold the
    /// press carries the pile so it stays under the pointer, re-anchoring
    /// to the nearest corner (`move_collapsed_stack`) while the rear cards
    /// lean with the carry.
    fn carry_move(&mut self) {
        let (Some(mut carry), Some(bounds)) = (self.carry, self.bounds) else {
            return;
        };
        let mut cursor = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut cursor);
        }
        let scale = f64::from(self.scale);
        let travel = f64::from(cursor.x - carry.screen.x)
            .hypot(f64::from(cursor.y - carry.screen.y))
            / scale;
        if !carry.carrying {
            if travel < CARRY_THRESHOLD {
                return;
            }
            carry.carrying = true;
            self.stack.reduced = crate::appearance::reduced_motion();
            self.stack.sway = Some(DragSway::default());
        }
        let now = now_ms();
        let (last, at) = carry.last;
        if let Some(sway) = self.stack.sway.as_mut() {
            let dt = if now > at {
                now - at
            } else {
                preview_motion::DRAG_SWAY_DEFAULT_DT_MS
            };
            sway.tick(
                f64::from(cursor.x - last.x) / scale,
                f64::from(cursor.y - last.y) / scale,
                dt,
                self.stack.reduced,
            );
        }
        carry.last = (cursor, now);
        self.carry = Some(carry);
        let placement = self.dragged.map_or_else(
            || {
                captures_settings::load(&self.options.settings_file)
                    .unwrap_or_default()
                    .mini_preview_placement
            },
            |(_, placement)| placement,
        );
        let (x, y) = (f64::from(cursor.x) / scale, f64::from(cursor.y) / scale);
        let moved = preview::move_collapsed_stack(
            bounds,
            self.stack.cards.len(),
            x - carry.press.0,
            y - carry.press.1,
            placement,
        );
        self.dragged = Some(moved);
        let _ = self.place();
        let _ = self.render();
        self.animate();
    }

    /// The press on the pile ended: a carry keeps the pile where it was
    /// put (no expand); its lean settles on the frame timer.
    fn carry_end(&mut self) -> bool {
        let carried = self.carry.take().is_some_and(|carry| carry.carrying);
        if carried {
            self.animate();
        }
        carried
    }

    fn motion_tick(&mut self) {
        // The carried pile's lean eases back when the pointer holds still
        // or lets go.
        if let Some(sway) = self.stack.sway.as_mut() {
            let now = now_ms();
            let at = self.carry.map_or(now - 16., |carry| carry.last.1);
            if now - at >= 16. {
                sway.tick(0., 0., (now - at).min(48.), self.stack.reduced);
                if let Some(carry) = self.carry.as_mut() {
                    carry.last.1 = now;
                }
            }
            if self.carry.is_none() && sway.settled() {
                self.stack.sway = None;
            }
        }
        // Finished exits leave and the window shrinks to the smaller stack;
        // the pointer finds its card again in the new order.
        if self.stack.prune_exits() {
            let _ = self.relayout();
            self.stack.pointer(self.cursor());
        }
        self.stack.prune_rejects();
        let _ = self.render();
        if self.moving() {
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
                Done::DragReady(id, result) => {
                    // Shipping drops a drag released before its file was ready.
                    if self.dragging.as_deref() != Some(id.as_str()) {
                        continue;
                    }
                    match result {
                        Ok(payload) if crate::drag::button_down() => {
                            self.drag_ready = Some(payload);
                            unsafe {
                                let _ = PostMessageW(
                                    Some(self.hwnd),
                                    WM_APP_PREVIEW_DRAG,
                                    WPARAM(0),
                                    LPARAM(0),
                                );
                            }
                        }
                        Ok(_) => self.end_drag(None),
                        Err(error) => self.end_drag(Some(Err(error))),
                    }
                }
            }
        }
        let _ = self.relayout();
        self.animate();
    }

    /// The press on a card's image moved past the drag threshold: hide the
    /// image and export the file off the UI thread (shipping
    /// `beginFileDrag`).
    fn begin_drag(&mut self, id: String) {
        self.drag_press = None;
        self.stack.pressed = None;
        // The OLE drag loop takes the mouse itself.
        let _ = unsafe { ReleaseCapture() };
        let Some(card) = self.stack.cards.iter_mut().find(|c| c.entry.id == id) else {
            return;
        };
        card.dragging = true;
        card.error = None;
        self.dragging = Some(id.clone());
        let _ = self.render();
        let (root, sender, hwnd) = (self.root.clone(), self.sender.clone(), self.hwnd.0 as isize);
        std::thread::spawn(move || {
            let _ = sender.send(Done::DragReady(
                id.clone(),
                crate::drag::prepare(&root, &id),
            ));
            post(hwnd);
        });
    }

    /// The file drag ended (`Some(Ok)`), failed (`Some(Err)`) or was let
    /// go before it began (`None`). An accepted drop outside Captures
    /// dismisses the card, a drop back on this stack shakes it, and any
    /// other end keeps it (shipping `finishFileDrag`). Hover and
    /// click-through re-arm either way.
    fn end_drag(&mut self, result: Option<std::result::Result<crate::drag::Outcome, String>>) {
        let Some(id) = self.dragging.take() else {
            return;
        };
        self.drag_ready = None;
        if let Some(card) = self.stack.cards.iter_mut().find(|c| c.entry.id == id) {
            card.dragging = false;
            if let Some(Err(error)) = &result {
                card.error = Some(error.clone());
            }
        }
        // A card already leaving keeps its exit.
        let live = self.stack.stack.ids().contains(&id);
        if let Some(Ok(outcome)) = result.filter(|_| live) {
            self.stack.reduced = crate::appearance::reduced_motion();
            let landing = crate::drag::landing(outcome.point, Some(self.hwnd));
            match preview::preview_drag_outcome(outcome.accepted, landing) {
                preview::PreviewDragOutcome::Dismiss => {
                    self.stack.leave(&id, ExitKind::Dismiss, 0., true);
                    let _ = self.relayout();
                }
                preview::PreviewDragOutcome::Reject => self.stack.reject(&id),
                preview::PreviewDragOutcome::Retain => {}
            }
        }
        let at = self.cursor();
        self.stack.pointer(at);
        self.set_passthrough(!at.is_some_and(|(x, y)| self.stack.solid(x, y)));
        let _ = self.render();
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
                if let Some((id, press)) = self.drag_press.clone() {
                    let mut now = POINT::default();
                    unsafe {
                        let _ = GetCursorPos(&mut now);
                    }
                    if crate::drag::beyond_threshold(press, now) {
                        self.begin_drag(id);
                        return Some(LRESULT(0));
                    }
                }
                if self.carry.is_some() {
                    self.carry_move();
                    if self.carry.is_some_and(|carry| carry.carrying) {
                        return Some(LRESULT(0));
                    }
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
            // The wheel scrolls a stack taller than its window, like
            // shipping's scrolling `<main>`.
            WM_MOUSEWHEEL => {
                let notches = f32::from((_wp.0 >> 16) as u16 as i16) / 120.;
                self.stack.reduced = crate::appearance::reduced_motion();
                if self.stack.wheel(-notches * WHEEL_STEP) {
                    self.stack.pointer(self.cursor());
                    let _ = self.render();
                    self.animate();
                }
            }
            // Another window took the mouse mid-press (a carry ends where
            // it is).
            WM_CAPTURECHANGED => {
                self.drag_press = None;
                if self.carry_end() {
                    self.stack.pressed = None;
                    let _ = self.render();
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
                let mut screen = POINT::default();
                unsafe {
                    let _ = GetCursorPos(&mut screen);
                }
                // An expanded card's image is the file drag source.
                if let Some(Hit::Card(index)) = self.stack.pressed
                    && let Some(card) = self.stack.cards.get(index)
                    && !card.busy
                    && self.dragging.is_none()
                {
                    self.drag_press = Some((card.entry.id.clone(), screen));
                }
                if self.stack.pressed == Some(Hit::Expand) {
                    self.carry = Some(Carry {
                        press: (f64::from(x), f64::from(y)),
                        screen,
                        carrying: false,
                        last: (screen, now_ms()),
                    });
                }
                unsafe { SetCapture(self.hwnd) };
                let _ = self.render();
            }
            WM_LBUTTONUP => {
                let (x, y) = point(lp, self.scale);
                self.drag_press = None;
                // Clear the carry before releasing capture, whose
                // WM_CAPTURECHANGED would otherwise end it too.
                let carried = self.carry_end();
                let _ = unsafe { ReleaseCapture() };
                let pressed = self.stack.pressed.take();
                let hit = self.stack.hit(x, y);
                if carried {
                    // A carry is not a click: the pile stays collapsed.
                    self.stack.pointer(Some((x, y)));
                    let _ = self.render();
                } else if pressed.is_some() && pressed == hit {
                    self.run(hit.unwrap_or(Hit::ClearAll));
                } else {
                    let _ = self.render();
                }
            }
            // Keyboard focus only arrives through assistive tech (clicks never
            // activate this window): Tab walks the buttons, Enter and Space
            // press the focused one, and Escape collapses an expanded stack
            // (GTK).
            WM_KEYDOWN => {
                let key = VIRTUAL_KEY(_wp.0 as u16);
                if key == VK_TAB {
                    let back = unsafe { GetKeyState(i32::from(VK_SHIFT.0)) } < 0;
                    self.stack.move_focus(back);
                    let _ = self.render();
                    self.animate();
                } else if key == VK_RETURN || key == VK_SPACE {
                    if let Some(hit) = self.stack.focus {
                        self.run(hit);
                    }
                } else if key == VK_ESCAPE && !self.stack.collapsed() {
                    self.run(Hit::Minimize);
                } else {
                    return None;
                }
            }
            WM_SETFOCUS => {
                if self.stack.focus.is_none() {
                    self.stack.move_focus(false);
                }
                let _ = self.render();
                self.animate();
            }
            WM_KILLFOCUS => {
                self.stack.focus = None;
                let _ = self.render();
                self.animate();
            }
            WM_APP_PREVIEW_DONE => self.finish(),
            WM_TIMER if _wp.0 == MOTION_TIMER => self.motion_tick(),
            WM_TIMER if _wp.0 == POLL_TIMER => self.poll(),
            WM_TIMER if _wp.0 == PRESENCE_TIMER => self.sync_presence(),
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
    // The OLE drag loop dispatches this window's messages, so it runs
    // without the borrow.
    if msg == WM_APP_PREVIEW_DRAG {
        let with = |f: &mut dyn FnMut(&mut Previews)| {
            PREVIEWS.with(|p| {
                if let Ok(mut p) = p.try_borrow_mut()
                    && let Some(previews) = p.as_mut()
                {
                    f(previews);
                }
            })
        };
        let mut ready = None;
        with(&mut |p| ready = p.drag_ready.take());
        if let Some(payload) = ready {
            let mut result = crate::drag::button_down()
                .then(|| crate::drag::run(&payload).map_err(|e| e.message().to_string()));
            with(&mut |p| p.end_drag(result.take()));
        }
        panel_uia::raise(panel_uia::Window::Previews, before);
        return LRESULT(0);
    }
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
                dragging: false,
                presence: EditorPresence::default(),
            });
        }
        stack.sync_toolbar(ToolbarCause::Other);
        stack.height = stack.content_height();
        stack
    }

    #[test]
    fn an_open_editor_pins_the_in_editor_pill_then_lingers_as_edit() {
        use captures_settings::MiniPreviewPlacement as P;
        let mut s = stack(P::BottomLeft, &[true, false]);
        s.pill_width = 100.;
        let card = s.card_rect(0).unwrap();
        let compact = s.edit_rect(0, card);
        assert_eq!(compact.right - compact.left, CONTROL);
        // No editor: Edit is only a target on a hovered card.
        assert_eq!(
            s.hit(compact.left + 4., compact.top + 4.),
            Some(Hit::Card(0))
        );
        let open = ["a0".to_owned()];
        assert!(s.sync_presence(&open, 0.));
        // The pill grows inward from the card's far corner and takes the
        // pointer without hover, with no tooltip and the action as its name.
        let pill = s.edit_rect(0, card);
        assert_eq!((pill.right, pill.right - pill.left), (compact.right, 100.));
        assert_eq!(s.hit(pill.left + 4., pill.top + 4.), Some(Hit::Edit(0)));
        assert!(s.pointer(Some((pill.left + 4., pill.top + 4.))));
        assert_eq!(s.tip_targets(), [None, None]);
        let name = |s: &Stack| {
            s.uia_items()
                .into_iter()
                .find(|i| Stack::uia_hit(i.key) == Some(Hit::Edit(0)))
                .map(|i| i.name)
        };
        assert_eq!(name(&s).as_deref(), Some("Show in editor"));
        // The editor closes: under reduced motion the leave is instant and
        // the plain Edit lingers, a target without hover, for 3 s.
        s.pointer(None);
        s.reduced = true;
        assert!(s.sync_presence(&[], 10.));
        assert_eq!(s.cards[0].presence.phase(), EditorPhase::Lingering);
        assert_eq!(s.edit_rect(0, card), compact);
        assert_eq!(
            s.hit(compact.left + 4., compact.top + 4.),
            Some(Hit::Edit(0))
        );
        assert_eq!(name(&s).as_deref(), Some("Edit"));
        assert!(s.sync_presence(&[], 3_010.));
        assert_eq!(s.cards[0].presence.phase(), EditorPhase::Idle);
        assert_eq!(
            s.hit(compact.left + 4., compact.top + 4.),
            Some(Hit::Card(0))
        );
    }

    #[test]
    fn minimize_grows_inward_into_show_less_while_hovered() {
        use captures_settings::MiniPreviewPlacement as P;
        for placement in [P::BottomLeft, P::BottomRight] {
            let mut s = stack(placement, &[false, false]);
            let clear = s.clear_rect();
            let rest = s.minimize_rect();
            s.pointer(Some((rest.left + 4., rest.top + 4.)));
            s.sync_minimize(now_ms());
            let open = s.minimize_rect();
            assert_eq!(open.right - open.left, 92., "{placement:?}");
            // It grows away from Clear all, which stays put.
            if s.right {
                assert_eq!((open.right, open.left), (rest.right, rest.right - 92.));
            } else {
                assert_eq!(open.left, rest.left);
            }
            assert_eq!(s.clear_rect().left, clear.left);
            // The grown pill keeps the pointer it grew under.
            let far = if s.right {
                open.left + 4.
            } else {
                open.right - 4.
            };
            assert_eq!(s.hit(far, open.top + 4.), Some(Hit::Minimize));
            // Leaving shrinks it back.
            s.pointer(None);
            s.sync_minimize(now_ms());
            assert_eq!(s.minimize_rect(), rest);
        }
        // While the toolbar enters it keeps its icon.
        let mut s = stack(P::BottomLeft, &[false, false]);
        s.motion_tokens = Some(Tokens::variant("light", "mustard"));
        s.focus = Some(Hit::Minimize);
        s.sync_minimize(now_ms());
        s.toolbar = StackToolbar::default();
        s.toolbar.set(true, ToolbarCause::Expand, now_ms());
        assert_eq!(s.minimize_look(), [0., 0.]);
    }

    #[test]
    fn the_close_streak_swaps_blurs_at_keyframe_midpoints() {
        // From the 2 px hover blur to 3.5, 8 and 14 on x, each swapping in
        // halfway to it, as CSS does with `url()` filters.
        for (blur, want) in [
            (2., 0.),
            (2.74, 0.),
            (2.75, 3.5),
            (5.74, 3.5),
            (5.75, 8.),
            (10.99, 8.),
            (11., 14.),
            (14., 14.),
        ] {
            assert_eq!(streak_blur(blur), want, "{blur}");
        }
    }

    #[test]
    fn the_toolbar_enters_and_leaves_without_taking_the_pointer() {
        use captures_settings::MiniPreviewPlacement as P;
        let mut s = stack(P::BottomLeft, &[true, false, false]);
        s.motion_tokens = Some(Tokens::variant("light", "mustard"));
        let clear = s.clear_rect();
        let (x, y) = (clear.left + 4., clear.top + 4.);
        // A stack that appears with two cards shows it at rest.
        assert_eq!(s.toolbar_pose(), Some(Pose::REST));
        assert_eq!(s.hit(x, y), Some(Hit::ClearAll));
        // Collapsing dissolves it through a blur, untouchable.
        s.set_collapsed(true);
        let out = s.toolbar_pose().expect("still drawn while it leaves");
        assert!(out.opacity > 0.9 && out.blur < 1., "{out:?}");
        assert!(!s.toolbar() && s.animating());
        // Expanding brings it back faded, lowered and smaller for its first
        // 35 %, and it only takes the pointer once at rest.
        s.set_collapsed(false);
        let entering = s.toolbar_pose().expect("entering");
        assert_eq!(
            (entering.opacity, entering.translate_y, entering.scale),
            (0., 6., 0.9)
        );
        assert_eq!(s.hit(x, y), None);
        // Closing down to one card holds it, then lets it go; Clear all
        // leaves at once. Neither plays under reduced motion.
        s.toolbar = StackToolbar::default();
        s.sync_toolbar(ToolbarCause::Other);
        s.card_motion = CardMotion::resolve(&Tokens::variant("light", "mustard"));
        s.leave("a2", ExitKind::Dismiss, 0., true);
        assert_eq!(s.toolbar_pose(), Some(Pose::REST));
        s.leave("a1", ExitKind::Dismiss, 0., true);
        assert_eq!(
            s.toolbar.motion().map(|(m, _)| m),
            Some(Motion::PreviewToolbarExit)
        );
        assert!(s.toolbar_pose().is_some() && !s.toolbar());
        let mut c = stack(P::BottomLeft, &[false, false]);
        c.motion_tokens = Some(Tokens::variant("light", "mustard"));
        c.card_motion = CardMotion::resolve(&Tokens::variant("light", "mustard"));
        c.clear_all();
        assert_eq!(
            c.toolbar.motion().map(|(m, _)| m),
            Some(Motion::PreviewToolbarClear)
        );
        c.reduced = true;
        assert_eq!(c.toolbar_pose(), None);
    }

    #[test]
    fn a_stack_taller_than_its_window_scrolls_by_wheel_and_cues() {
        use captures_settings::MiniPreviewPlacement as P;
        // Four cards (768 DIPs of content) in a 560 DIP window: 208 to scroll.
        let mut s = stack(P::BottomLeft, &[false, false, false, false]);
        s.height = 560.;
        s.reduced = true;
        let cue = |s: &Stack| -> Vec<(Hit, f32, String)> {
            s.uia_items()
                .into_iter()
                .filter(|i| i.key == 4 || i.key == 5)
                .map(|i| (Stack::uia_hit(i.key).unwrap(), i.rect.top, i.name))
                .collect()
        };
        // Newest in view at the bottom: only the upper cue, for older cards.
        assert_eq!(s.card_rect(3).map(|r| r.bottom), Some(560. - GUTTER));
        assert_eq!(
            cue(&s),
            [(Hit::CueUp, 6., "Show older captures".to_owned())]
        );
        assert_eq!(s.hit(170., 10.), Some(Hit::CueUp));
        // Already at the newest end: wheeling down does nothing.
        assert!(!s.wheel(100.));
        // One notch up scrolls 100 DIPs; both cues show and the toolbar
        // stays at the window's bottom edge.
        assert!(s.wheel(-100.));
        assert_eq!(s.card_rect(3).map(|r| r.bottom), Some(560. - GUTTER + 100.));
        assert_eq!(cue(&s).len(), 2);
        assert_eq!(s.clear_rect().top, 560. - GUTTER + 8.);
        // The cue moves one 176 DIP slot, clamped at the oldest card.
        s.scroll_slots(-1);
        assert_eq!(s.card_rect(0).map(|r| r.top), Some(PAD));
        assert_eq!(
            cue(&s),
            [(
                Hit::CueDown,
                560. - 6. - 22.,
                "Show newer captures".to_owned()
            )]
        );
        // A collapsed pile never scrolls.
        s.stack.set_collapsed(true);
        assert!(!s.wheel(-100.) && cue(&s).is_empty());
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
    fn a_drop_back_on_the_stack_shakes_the_card_in_place() {
        use captures_settings::MiniPreviewPlacement as P;
        // `thumbnail-drop-reject`'s keyframes, linear between them.
        let x = [20., 16., 12., 8.];
        for (ms, want) in [
            (0., 0.),
            (42., -20.),
            (84., 0.),
            (126., 20.),
            (210., -16.),
            (273., 12.),
            (336., -8.),
            (378., 8.),
            (420., 0.),
            (900., 0.),
        ] {
            let got = reject_offset(x, ms);
            assert!((got - want).abs() < 1e-3, "{ms} ms: {got}");
        }
        let tokens = Tokens::variant("light", "mustard");
        let amounts = [1, 2, 3, 4].map(|n| tokens.n(&format!("thumbnail-drop-reject-x-{n}")));
        assert_eq!(amounts, [12., 8., 6., 4.]);
        let mut s = stack(P::BottomLeft, &[true, false]);
        s.card_motion = CardMotion::resolve(&tokens);
        s.reject("a1");
        // The card stays live and in its slot while it shakes.
        assert_eq!(s.stack.ids(), ["a0", "a1"]);
        assert!(s.animating());
        assert!(s.prune_rejects());
        // Reduced motion refuses the drop without the shake.
        s.rejects.clear();
        s.reduced = true;
        s.reject("a0");
        assert!(s.rejects.is_empty());
        assert!(!s.animating());
    }

    #[test]
    fn only_icon_controls_and_clear_all_show_tooltips() {
        use captures_settings::MiniPreviewPlacement as P;
        let mut s = stack(P::BottomLeft, &[true, false]);
        let card = s.card_rect(0).unwrap();
        s.pointer(Some((card.left + 40., card.top + 60.)));
        let controls = s.top_controls(0, card);
        let at = |r: Rect| Some((r.left + 4., r.top + 4.));
        // Close, Delete and Edit name themselves; the card body, Copy and
        // Save file carry no tooltip.
        for (hit, r) in &controls {
            s.pointer(at(*r));
            assert_eq!(s.tip_targets(), [Some(*hit), None], "{hit:?}");
        }
        let (copy, _) = s.main_actions(0, card)[0];
        assert_eq!(copy, Hit::Copy(0));
        s.pointer(at(s.main_actions(0, card)[0].1));
        assert_eq!(s.tip_targets(), [None, None]);
        s.pointer(at(s.clear_rect()));
        assert_eq!(s.tip_targets(), [None, Some(Hit::ClearAll)]);
        assert_eq!(
            s.tip(Hit::ClearAll).map(|(label, ..)| label),
            Some("Clear all")
        );
        // Bottom stacks open tips above their control, top stacks below.
        let (label, anchor, above) = s.tip(controls[1].0).unwrap();
        assert_eq!((label, above), ("Delete", true));
        assert_eq!(anchor, controls[1].1);
        assert!(!stack(P::TopRight, &[true]).tip(Hit::Close(0)).unwrap().2);
    }

    #[test]
    fn the_hover_look_eases_each_channel_and_reverses_from_where_it_is() {
        use captures_settings::MiniPreviewPlacement as P;
        let fade = HoverFade {
            on: true,
            at: 0.,
            from: [0.; 4],
        };
        // At 160 ms the controls (0.16 s) and chip (0.15 s) are in, the
        // filter (0.18 s) nearly and the zoom (0.22 s) not yet.
        let [filter, zoom, controls, chip] = fade.values(160.);
        assert_eq!((controls, chip), (1., 1.));
        assert!(filter > 0.95 && filter < 1., "{filter}");
        assert!(zoom > 0.8 && zoom < filter, "{zoom}");
        assert_eq!(fade.values(220.), [1.; 4]);
        assert!(fade.running(200.) && !fade.running(220.));

        let mut s = stack(P::BottomLeft, &[true, false]);
        s.motion = Some((
            Tween {
                duration_ms: 1.,
                easing: captures_app::motion::CubicBezier::LINEAR,
            },
            Tween {
                duration_ms: 1.,
                easing: captures_app::motion::CubicBezier::LINEAR,
            },
        ));
        let card = s.card_rect(0).unwrap();
        s.pointer(Some((card.left + 40., card.top + 60.)));
        s.sync_hover(1_000.);
        assert!(s.hover_fades["a0"].on);
        // Leaving 90 ms in reverses from the partial look, not from rest.
        s.pointer(None);
        s.sync_hover(1_090.);
        let leaving = s.hover_fades["a0"];
        assert!(!leaving.on);
        assert!(
            leaving.from[0] > 0.3 && leaving.from[0] < 0.9,
            "{:?}",
            leaving.from
        );
        assert_eq!(leaving.values(1_090.), leaving.from);
        // Settled out, it is forgotten.
        s.sync_hover(1_400.);
        assert!(s.hover_fades.is_empty());
    }

    #[test]
    fn the_hovered_pile_sparkles_until_left_carried_or_reduced() {
        use captures_settings::MiniPreviewPlacement as P;
        let mut s = stack(P::BottomLeft, &[true, false]);
        s.card_motion = CardMotion::resolve(&Tokens::variant("light", "mustard"));
        s.stack.set_collapsed(true);
        let front = s.front_rect().unwrap();
        s.pointer(Some((front.left + 40., front.top + 40.)));
        assert!(s.sparkling() && s.animating(), "hovered pile sparkles");
        s.reduced = true;
        assert!(!s.sparkling(), "reduced motion");
        s.reduced = false;
        s.sway = Some(DragSway::default());
        assert!(!s.sparkling(), "carried");
        s.sway = None;
        s.pointer(None);
        assert!(!s.sparkling() && s.sparkle_since.is_none(), "left");
        // Both loops sit inside the layer that reaches past the pile.
        let layer =
            preview_motion::sparkle_layer((front.left.into(), front.top.into(), 284., 160.), false);
        assert_eq!(
            layer.1,
            f64::from(front.top) - preview_motion::SPARKLE_REACH
        );
    }

    #[test]
    fn the_capture_highlight_holds_then_fades_and_keeps_the_frame_timer() {
        use captures_settings::MiniPreviewPlacement as P;
        let mut s = stack(P::BottomLeft, &[true]);
        s.card_motion = CardMotion::resolve(&Tokens::variant("light", "mustard"));
        let m = s.card_motion.unwrap();
        // Held through 1 s, half gone at the fade's midpoint, gone at 4.5 s.
        assert_eq!(m.highlight.pose_at(900., false).opacity, 1.);
        let mid = m.highlight.pose_at(2_750., false).opacity;
        assert!((mid - 0.5).abs() < 0.01, "{mid}");
        assert_eq!(m.highlight.pose_at(4_500., false).opacity, 0.);
        s.highlights.insert("a0".into(), now_ms() - 4_000.);
        assert!(s.animating(), "still fading");
        s.highlights.insert("a0".into(), now_ms() - 4_600.);
        assert!(!s.animating());
    }

    #[test]
    fn keyboard_focus_walks_the_buttons_and_follows_a_collapse() {
        use captures_settings::MiniPreviewPlacement as P;
        let mut s = stack(P::BottomLeft, &[true, false]);
        // DOM order: the saved card's five buttons, the unsaved card's four
        // (Delete alone in its corner), then the toolbar.
        let order = s.focus_order();
        assert_eq!(
            order,
            [
                Hit::Close(0),
                Hit::Delete(0),
                Hit::Edit(0),
                Hit::Copy(0),
                Hit::Save(0),
                Hit::Delete(1),
                Hit::Edit(1),
                Hit::Copy(1),
                Hit::Save(1),
                Hit::ClearAll,
                Hit::Minimize,
            ]
        );
        s.move_focus(true);
        assert_eq!(s.focus, Some(Hit::Minimize), "Shift+Tab from nothing");
        s.move_focus(false);
        assert_eq!(s.focus, Some(Hit::Close(0)), "Tab wraps");
        s.move_focus(false);
        s.move_focus(false);
        assert_eq!(s.focus, Some(Hit::Edit(0)));
        // A focused control shows its card's controls and its tooltip
        // without the pointer (`:focus-within`, `:focus-visible::after`).
        assert!(s.shows_controls(0) && !s.shows_controls(1));
        assert_eq!(s.tip_targets(), [Some(Hit::Edit(0)), None]);
        assert_eq!(
            s.control_rect(Hit::Edit(0)),
            Some(s.top_controls(0, s.card_rect(0).unwrap())[2].1)
        );
        // Escape's collapse leaves the pile's Expand as the one button.
        s.stack.set_collapsed(true);
        s.sync_focus();
        assert_eq!(s.focus, Some(Hit::Expand));
        assert_eq!(s.tip_targets(), [None, None]);
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
