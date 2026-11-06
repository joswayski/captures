//! Capture History, shipping `CaptureHistory` and `HistoryCard`
//! (`apps/desktop/ui/src/App.tsx`) with the `.history-*` rules in
//! `styles/windows.css` and `styles/primitives.css`, as its own Win32 window
//! beside Preferences. Copy, card actions, dates and grid geometry come from
//! `captures_app::history_view`; entries from `captures_history`.
//!
//! Retained like Preferences: one DirectComposition visual and surface per
//! node, redrawn only when its `dirty` flag is set. Hover layers, focus rings
//! and the card lift are compositor animations. The grid is virtualised:
//! card slots exist only for rows inside the viewport, and a card that
//! scrolls out releases every surface it had. Thumbnails decode through WIC on
//! one worker thread at display size, with a small cache for scrolling back.
use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use captures_app::history_view::{self, Card, CardAction, Grid, RowExtras};
use captures_app::motion::CubicBezier;
use captures_history::{ArtifactKind, HistoryEntry};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectComposition::*;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::*;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;
use windows_numerics::{Matrix3x2, Vector2};

use crate::gfx::{self, Gfx, Style, Text, rect};
use crate::scene::Rect;
use crate::tokens::Tokens;
use crate::{Options, appearance};

/// `.history-shell { max-width: 1180px }`.
const SHELL_MAX: f32 = 1180.;
/// Shipping `@media (max-width: 720px)`.
const COMPACT_MAX: f32 = 720.;
/// A classic scrollbar's width, as in Preferences.
const SCROLLBAR: f32 = 10.;
/// `.ui-btn svg` and `.history-clear-all svg`.
const ICON: f32 = 15.;
/// `.history-image-wrap > .history-delete svg`.
const DELETE_ICON: f32 = 16.;
/// `.history-empty-icon`, its svg, and `.history-empty { min-height }`.
const EMPTY_BOX: f32 = 52.;
const EMPTY_ICON: f32 = 26.;
const EMPTY_MIN: f32 = 320.;
/// `.history-card:hover { transform: translateY(-2px) }`.
const LIFT: f32 = 2.;
/// Decoded thumbnails kept for scrolling back; about two screens of cards.
const THUMB_CACHE: usize = 36;
/// Shipping's filtered-empty copy (`history_view::COPY` has an older line).
pub const FILTERED_TITLE: &str = "No captures in this filter";
pub const FILTERED_BODY: &str = "Choose All to see your other captures.";

const WM_APP_DONE: u32 = WM_APP + 11;
const WM_APP_THUMB: u32 = WM_APP + 12;
pub(crate) const WM_APP_WAKE: u32 = WM_APP + 13;
const WM_MOUSELEAVE: u32 = 0x02A3;
const TIMER_MOTION: usize = 1;
/// The nearest two-step delete deadline.
const TIMER_CONFIRM: usize = 2;
const SEGMENTS: usize = 16;

thread_local! {
    static HISTORY: RefCell<Option<History>> = const { RefCell::new(None) };
}

/// Run `f` on the History window unless it is borrowed or closed; whatever
/// `f` leaves dirty is guaranteed a frame (see `host::with_host`).
pub fn with_history<R>(f: impl FnOnce(&mut History) -> R) -> Option<R> {
    HISTORY.with(|h| {
        let mut h = h.try_borrow_mut().ok()?;
        let history = h.as_mut()?;
        let result = f(history);
        history.wake();
        Some(result)
    })
}

pub fn exists() -> bool {
    HISTORY.with(|h| h.try_borrow().map_or(true, |h| h.is_some()))
}

/// Shipping `HISTORY_FILTERS`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Filter {
    #[default]
    All,
    Screenshots,
    Video,
    Gif,
}

pub const FILTERS: [Filter; 4] = [Filter::All, Filter::Screenshots, Filter::Video, Filter::Gif];

impl Filter {
    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Screenshots => "Screenshots",
            Self::Video => "Video",
            Self::Gif => "GIF",
        }
    }

    fn matches(self, kind: ArtifactKind) -> bool {
        matches!(
            (self, kind),
            (Self::All, _)
                | (Self::Screenshots, ArtifactKind::Screenshot)
                | (Self::Video, ArtifactKind::Video)
                | (Self::Gif, ArtifactKind::Gif)
        )
    }
}

/// Something that takes focus or a click. Cards are named by their serial,
/// which survives reloads, filtering and scrolling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Back,
    Cancel,
    DeleteAll,
    Filter(usize),
    /// A card's trash button.
    Delete(u32),
    /// A card's action button, by index into `Card::actions`.
    Action(u32, usize),
}

impl Target {
    pub fn serial(self) -> Option<u32> {
        match self {
            Target::Delete(serial) | Target::Action(serial, _) => Some(serial),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Busy {
    Deleting,
    Action(CardAction),
}

pub struct Item {
    pub serial: u32,
    pub entry: HistoryEntry,
    pub card: Card,
    /// The second Delete click arms until this deadline.
    pub confirm: Option<Instant>,
    pub busy: Option<Busy>,
    pub error: Option<String>,
}

impl Item {
    /// Edit opens the screenshot editor; the recording editor and Restore's
    /// floating preview don't exist in this host yet, so those stay disabled.
    pub fn action_enabled(&self, action: CardAction) -> bool {
        self.busy.is_none()
            && match action {
                CardAction::SaveFile | CardAction::ShowInFolder => true,
                CardAction::Edit => !self.entry.kind.is_recording() && !self.card.missing,
                _ => false,
            }
    }

    pub fn action_label(&self, action: CardAction) -> &'static str {
        if self.busy == Some(Busy::Action(action)) {
            action.busy_label()
        } else {
            action.label()
        }
    }

    pub fn delete_label(&self) -> &'static str {
        if self.confirm.is_some() {
            self.card.delete_confirm_label
        } else {
            self.card.delete_label
        }
    }
}

/// Accessibility notifications, raised after the frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Focus(Target),
    /// A control's name changed (two-step deletes, busy labels).
    Name(Target),
    /// A filter's `aria-pressed` changed.
    Pressed(usize, bool),
    /// Which elements exist changed (loads, filters, deletes).
    Structure,
    /// A `role="alert"` error appeared; the card's serial, or the page's.
    Alert(Option<u32>),
}

enum Done {
    Loaded(std::result::Result<Vec<(HistoryEntry, bool)>, String>),
    Deleted(u32, std::result::Result<(), String>),
    Cleared(std::result::Result<(), String>),
    Saved(u32, std::result::Result<String, String>),
    Shown(u32, std::result::Result<(), String>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Layer {
    Base,
    Lit,
    Ring,
}

/// A button; card controls name their slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ctl {
    Back,
    Cancel,
    DeleteAll,
    Filter(usize),
    Delete(usize),
    Action(usize, usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Group,
    /// `.capture-history`'s canvas.
    Canvas,
    /// `h1` and the lede.
    Heading,
    Control(Ctl, Layer),
    /// `.history-toolbar`'s bottom border.
    Rule,
    /// `.history-error`.
    Error,
    /// `.history-empty`: loading, empty or filtered-empty.
    Empty,
    Scrollbar,
    /// A card slot's group; it lifts on hover.
    Slot(usize),
    /// `--shadow-md` under a hovered card.
    Shadow(usize),
    /// The card: shadow, fill, border and body copy.
    CardBase(usize),
    /// `--border-strong` over a hovered card.
    Outline(usize),
    /// `.history-image-wrap`: thumbnail, divider and "File missing".
    Image(usize),
}

struct Node {
    parent: Option<usize>,
    kind: Kind,
    rect: Rect,
    bleed: f32,
    opacity: f32,
    visible: bool,
    dirty: bool,
    props: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Prop {
    Opacity,
    OffsetY,
}

struct Motion {
    node: usize,
    prop: Prop,
    from: f32,
    to: f32,
    start: Instant,
    seconds: f32,
    curve: CubicBezier,
    applied: bool,
    fresh: bool,
}

impl Motion {
    fn value_at(&self, progress: f32) -> f32 {
        self.from + (self.to - self.from) * self.curve.ease(f64::from(progress)) as f32
    }

    fn progress(&self, now: Instant) -> f32 {
        (now.duration_since(self.start).as_secs_f32() / self.seconds).clamp(0., 1.)
    }
}

/// One card's nodes, bound to whichever card it shows.
struct Slot {
    serial: Option<u32>,
    group: usize,
    shadow: usize,
    base: usize,
    outline: usize,
    image: usize,
    delete: [usize; 3],
    actions: [[usize; 3]; 2],
}

impl Slot {
    fn nodes(&self) -> Vec<usize> {
        let mut nodes = vec![self.shadow, self.base, self.outline, self.image];
        nodes.extend(self.delete);
        nodes.extend(self.actions.iter().flatten());
        nodes
    }
}

#[derive(Clone, Copy)]
struct Layered {
    base: usize,
    lit: usize,
    ring: usize,
}

/// Decoded premultiplied BGRA at display size.
struct Thumb {
    w: u32,
    h: u32,
    pixels: Vec<u8>,
}

/// A thumbnail request: card serial, entry id, grayscale, pixel size.
type ThumbRequest = (u32, String, bool, (u32, u32));

type ThumbResult = (u32, (u32, u32), std::result::Result<Thumb, String>);

struct Thumbs {
    cache: VecDeque<(u32, Thumb)>,
    /// Bound cards and the pixel size they want; the worker skips the rest.
    wanted: Arc<Mutex<HashMap<u32, (u32, u32)>>>,
    pending: HashSet<u32>,
    failed: HashSet<u32>,
    requests: Option<Sender<ThumbRequest>>,
    results: Receiver<ThumbResult>,
    sender: Sender<ThumbResult>,
}

/// Laid-out positions in content coordinates (before scrolling).
#[derive(Default)]
pub struct Layout {
    pub shell: Rect,
    pub back: Rect,
    pub heading: Rect,
    pub title_h: f32,
    pub cancel: Rect,
    pub delete_all: Rect,
    pub filters: [Rect; 4],
    pub toolbar: Rect,
    pub error: Rect,
    pub empty: Rect,
    pub grid_y: f32,
}

#[derive(Default)]
struct Texts {
    back: Option<Text>,
    title: Option<Text>,
    lede: Option<Text>,
    cancel: Option<Text>,
    delete_all: Option<Text>,
    filters: Vec<(Text, Text)>,
    error: Option<Text>,
    empty: Option<(Text, Option<Text>)>,
}

pub struct History {
    pub hwnd: HWND,
    pub g: Gfx,
    pub t: Tokens,
    root: PathBuf,
    settings_file: PathBuf,
    appearance: Option<String>,
    pub scale: f32,
    reduced_motion: bool,
    pub loading: bool,
    pub items: Vec<Item>,
    /// Indices into `items` that pass the filter, in display order.
    pub shown: Vec<usize>,
    pub filter: Filter,
    pub error: Option<String>,
    /// Delete all arms until this deadline.
    pub confirm_all: Option<Instant>,
    pub deleting_all: bool,
    next_serial: u32,
    pub w: f32,
    pub h: f32,
    pub compact: bool,
    pub scroll: f32,
    content_h: f32,
    pub grid: Grid,
    extras: RowExtras,
    pub l: Layout,
    texts: Texts,
    pub hover: Option<Target>,
    hover_card: Option<u32>,
    pub focus: Option<Target>,
    focus_visible: bool,
    pressed: Option<Target>,
    pointer: Option<(f32, f32)>,
    tracking: bool,
    nodes: Vec<Node>,
    children: Vec<Vec<usize>>,
    canvas: usize,
    content: usize,
    heading: usize,
    back: Layered,
    cancel: Layered,
    delete_all: Layered,
    filters: Vec<Layered>,
    rule: usize,
    error_node: usize,
    empty: usize,
    grid_node: usize,
    scrollbar: usize,
    slots: Vec<Slot>,
    /// Nodes whose surfaces go back to the compositor this frame.
    release: Vec<usize>,
    motions: Vec<Motion>,
    dcomp: Option<IDCompositionDesktopDevice>,
    _target: Option<IDCompositionTarget>,
    visuals: Vec<Option<IDCompositionVisual3>>,
    surfaces: Vec<Option<(IDCompositionSurface, (u32, u32))>>,
    offsets: Vec<(f32, f32)>,
    thumbs: Thumbs,
    done: Receiver<Done>,
    sender: Sender<Done>,
    pub need: bool,
    relayout: bool,
    /// A `WM_APP_WAKE` was posted since the loop last checked `need`.
    pub(crate) woken: bool,
    timer: bool,
    pub events: Vec<Event>,
    /// Back to Preferences was activated; the loop opens it.
    pub open_preferences: bool,
    /// Edit was chosen: the run loop opens this artifact's editor.
    pub open_editor: Option<String>,
}

impl History {
    fn new(
        hwnd: HWND,
        g: Gfx,
        options: &Options,
        tokens: Tokens,
        root: PathBuf,
    ) -> Result<History> {
        let (sender, done) = channel();
        let (thumb_sender, thumb_results) = channel();
        let mut history = History {
            hwnd,
            g,
            t: tokens,
            root,
            settings_file: options.settings_file.clone(),
            appearance: options.appearance.clone(),
            scale: 1.,
            reduced_motion: false,
            loading: true,
            items: Vec::new(),
            shown: Vec::new(),
            filter: Filter::All,
            error: None,
            confirm_all: None,
            deleting_all: false,
            next_serial: 1,
            w: 0.,
            h: 0.,
            compact: false,
            scroll: 0.,
            content_h: 0.,
            grid: history_view::grid(0.),
            extras: RowExtras::default(),
            l: Layout::default(),
            texts: Texts::default(),
            hover: None,
            hover_card: None,
            focus: None,
            focus_visible: false,
            pressed: None,
            pointer: None,
            tracking: false,
            nodes: Vec::new(),
            children: Vec::new(),
            canvas: 0,
            content: 0,
            heading: 0,
            back: Layered {
                base: 0,
                lit: 0,
                ring: 0,
            },
            cancel: Layered {
                base: 0,
                lit: 0,
                ring: 0,
            },
            delete_all: Layered {
                base: 0,
                lit: 0,
                ring: 0,
            },
            filters: Vec::new(),
            rule: 0,
            error_node: 0,
            empty: 0,
            grid_node: 0,
            scrollbar: 0,
            slots: Vec::new(),
            release: Vec::new(),
            motions: Vec::new(),
            dcomp: None,
            _target: None,
            visuals: Vec::new(),
            surfaces: Vec::new(),
            offsets: Vec::new(),
            thumbs: Thumbs {
                cache: VecDeque::new(),
                wanted: Arc::default(),
                pending: HashSet::new(),
                failed: HashSet::new(),
                requests: None,
                results: thumb_results,
                sender: thumb_sender,
            },
            done,
            sender,
            need: true,
            relayout: true,
            woken: false,
            timer: false,
            events: Vec::new(),
            open_preferences: false,
            open_editor: None,
        };
        history.build();
        Ok(history)
    }

    fn add(&mut self, parent: Option<usize>, kind: Kind, bleed: f32) -> usize {
        let index = self.nodes.len();
        self.nodes.push(Node {
            parent,
            kind,
            rect: Rect::default(),
            bleed,
            opacity: 1.,
            visible: true,
            dirty: true,
            props: true,
        });
        self.children.push(Vec::new());
        self.visuals.push(None);
        self.surfaces.push(None);
        self.offsets.push((0., 0.));
        if let Some(parent) = parent {
            self.children[parent].push(index);
        }
        index
    }

    fn layered(&mut self, parent: usize, ctl: Ctl, bleed: f32) -> Layered {
        let base = self.add(Some(parent), Kind::Control(ctl, Layer::Base), bleed);
        let lit = self.add(Some(base), Kind::Control(ctl, Layer::Lit), 0.);
        let ring = self.add(Some(base), Kind::Control(ctl, Layer::Ring), 4.);
        self.nodes[lit].opacity = 0.;
        self.nodes[ring].visible = false;
        Layered { base, lit, ring }
    }

    fn build(&mut self) {
        let root = self.add(None, Kind::Group, 0.);
        self.canvas = self.add(Some(root), Kind::Canvas, 0.);
        self.content = self.add(Some(root), Kind::Group, 0.);
        let content = self.content;
        self.heading = self.add(Some(content), Kind::Heading, 0.);
        self.back = self.layered(content, Ctl::Back, 0.);
        self.cancel = self.layered(content, Ctl::Cancel, 0.);
        self.delete_all = self.layered(content, Ctl::DeleteAll, 0.);
        self.filters = (0..FILTERS.len())
            .map(|i| self.layered(content, Ctl::Filter(i), 2.))
            .collect();
        self.rule = self.add(Some(content), Kind::Rule, 0.);
        self.error_node = self.add(Some(content), Kind::Error, 0.);
        self.empty = self.add(Some(content), Kind::Empty, 0.);
        self.grid_node = self.add(Some(content), Kind::Group, 0.);
        self.scrollbar = self.add(Some(root), Kind::Scrollbar, 0.);
    }

    fn add_slot(&mut self) -> usize {
        let index = self.slots.len();
        let group = self.add(Some(self.grid_node), Kind::Slot(index), 0.);
        let shadow = self.add(Some(group), Kind::Shadow(index), 16.);
        let base = self.add(Some(group), Kind::CardBase(index), 4.);
        let outline = self.add(Some(group), Kind::Outline(index), 0.);
        let image = self.add(Some(group), Kind::Image(index), 0.);
        let delete = self.layered(group, Ctl::Delete(index), 4.);
        let actions = [0, 1].map(|i| self.layered(group, Ctl::Action(index, i), 2.));
        self.nodes[shadow].opacity = 0.;
        self.nodes[outline].opacity = 0.;
        self.slots.push(Slot {
            serial: None,
            group,
            shadow,
            base,
            outline,
            image,
            delete: [delete.base, delete.lit, delete.ring],
            actions: actions.map(|a| [a.base, a.lit, a.ring]),
        });
        index
    }

    /// Queue a wake message when a frame is due and none is queued.
    fn wake(&mut self) {
        if self.need && !self.woken && !self.hwnd.is_invalid() {
            self.woken = true;
            unsafe {
                let _ = PostMessageW(Some(self.hwnd), WM_APP_WAKE, WPARAM(0), LPARAM(0));
            }
        }
    }

    // ---- data ----

    pub fn item(&self, serial: u32) -> Option<&Item> {
        self.items.iter().find(|item| item.serial == serial)
    }

    fn item_mut(&mut self, serial: u32) -> Option<&mut Item> {
        self.items.iter_mut().find(|item| item.serial == serial)
    }

    /// Position of a card in the shown list.
    pub fn shown_index(&self, serial: u32) -> Option<usize> {
        self.shown
            .iter()
            .position(|&i| self.items[i].serial == serial)
    }

    pub fn count(&self, filter: Filter) -> usize {
        self.items
            .iter()
            .filter(|item| filter.matches(item.entry.kind))
            .count()
    }

    /// Shipping disables an empty filter other than All.
    pub fn filter_enabled(&self, index: usize) -> bool {
        index == 0 || self.count(FILTERS[index]) > 0
    }

    /// The header actions and filters show once entries have loaded.
    pub fn has_entries(&self) -> bool {
        !self.loading && !self.items.is_empty()
    }

    /// Load entries off the UI thread; shipping's `refresh`.
    fn reload(&mut self) {
        let (root, sender, hwnd) = (self.root.clone(), self.sender.clone(), self.hwnd.0 as isize);
        std::thread::spawn(move || {
            let _ = sender.send(Done::Loaded(load(&root)));
            post(hwnd, WM_APP_DONE);
        });
    }

    fn set_entries(&mut self, entries: Vec<(HistoryEntry, bool)>) {
        let mut old: HashMap<String, Item> = std::mem::take(&mut self.items)
            .into_iter()
            .map(|item| (item.entry.id.clone(), item))
            .collect();
        for (entry, missing) in entries {
            let card = history_view::card(&entry, missing);
            let item = match old.remove(&entry.id) {
                Some(mut item) => {
                    if item.card != card {
                        // A changed card (say, now saved) drops its stale thumbnail.
                        if item.card.missing != card.missing {
                            self.forget_thumb(item.serial);
                        }
                        item.card = card;
                    }
                    item.entry = entry;
                    item
                }
                None => {
                    let serial = self.next_serial;
                    self.next_serial += 1;
                    Item {
                        serial,
                        entry,
                        card,
                        confirm: None,
                        busy: None,
                        error: None,
                    }
                }
            };
            self.items.push(item);
        }
        for item in old.values() {
            self.forget_thumb(item.serial);
        }
        self.loading = false;
        self.refilter();
    }

    fn refilter(&mut self) {
        let filter = self.filter;
        self.shown = (0..self.items.len())
            .filter(|&i| filter.matches(self.items[i].entry.kind))
            .collect();
        if self.items.is_empty() {
            self.confirm_all = None;
        }
        // A removed control drops focus, like a removed DOM node.
        if let Some(focus) = self.focus
            && !self.target_exists(focus)
        {
            self.focus = None;
        }
        if self.hover.is_some_and(|t| !self.target_exists(t)) {
            self.hover = None;
        }
        self.events.push(Event::Structure);
        self.relayout = true;
        self.need = true;
    }

    fn target_exists(&self, target: Target) -> bool {
        match target {
            Target::Back => true,
            Target::Cancel => self.has_entries() && self.confirm_all.is_some(),
            Target::DeleteAll | Target::Filter(_) => self.has_entries(),
            Target::Delete(serial) => self.shown_index(serial).is_some(),
            Target::Action(serial, i) => self
                .shown_index(serial)
                .is_some_and(|_| i < self.item(serial).map_or(0, |it| it.card.actions.len())),
        }
    }

    pub fn enabled(&self, target: Target) -> bool {
        match target {
            Target::Back => true,
            Target::Cancel | Target::DeleteAll => !self.deleting_all,
            Target::Filter(i) => self.filter_enabled(i),
            Target::Delete(serial) => self.item(serial).is_some_and(|item| item.busy.is_none()),
            Target::Action(serial, i) => self.item(serial).is_some_and(|item| {
                item.card
                    .actions
                    .get(i)
                    .is_some_and(|&action| item.action_enabled(action))
            }),
        }
    }

    /// Accessible and visible name of a control.
    pub fn name(&self, target: Target) -> String {
        let copy = history_view::copy();
        match target {
            Target::Back => copy.back_to_preferences.into(),
            Target::Cancel => copy.cancel_label.into(),
            Target::DeleteAll if self.confirm_all.is_some() => copy.delete_all_confirm_label.into(),
            Target::DeleteAll => copy.delete_all_label.into(),
            Target::Filter(i) => FILTERS[i].label().into(),
            Target::Delete(serial) => self
                .item(serial)
                .map_or_else(String::new, |item| item.delete_label().into()),
            Target::Action(serial, i) => self
                .item(serial)
                .and_then(|item| Some(item.action_label(*item.card.actions.get(i)?).into()))
                .unwrap_or_default(),
        }
    }

    fn delete_all_label(&self) -> &'static str {
        let copy = history_view::copy();
        if self.deleting_all {
            copy.delete_all_busy
        } else if self.confirm_all.is_some() {
            copy.delete_all_confirm
        } else {
            copy.delete_all
        }
    }

    // ---- actions ----

    pub fn activate(&mut self, target: Target) {
        if !self.enabled(target) {
            return;
        }
        match target {
            Target::Back => self.open_preferences = true,
            Target::Cancel => self.disarm_all(),
            Target::DeleteAll => self.delete_all_step(),
            Target::Filter(i) => {
                if FILTERS[i] != self.filter {
                    let old = FILTERS.iter().position(|&f| f == self.filter).unwrap_or(0);
                    self.filter = FILTERS[i];
                    self.events.push(Event::Pressed(old, false));
                    self.events.push(Event::Pressed(i, true));
                    for filter in [old, i] {
                        self.nodes[self.filters[filter].base].dirty = true;
                        self.nodes[self.filters[filter].lit].dirty = true;
                    }
                    self.set_scroll(0.);
                    self.refilter();
                }
            }
            Target::Delete(serial) => self.delete_step(serial),
            Target::Action(serial, i) => {
                let Some(action) = self
                    .item(serial)
                    .and_then(|it| it.card.actions.get(i).copied())
                else {
                    return;
                };
                self.run_action(serial, action);
            }
        }
        self.need = true;
    }

    /// Shipping `deleteFromHistory`: arm, then delete; a missing recording
    /// goes at once.
    fn delete_step(&mut self, serial: u32) {
        let Some(item) = self.item_mut(serial) else {
            return;
        };
        if item.card.delete_requires_confirmation && item.confirm.is_none() {
            item.confirm =
                Some(Instant::now() + Duration::from_millis(history_view::CONFIRM_TIMEOUT_MS));
            self.events.push(Event::Name(Target::Delete(serial)));
            self.restyle(serial);
            self.arm_timer();
            return;
        }
        item.busy = Some(Busy::Deleting);
        item.error = None;
        let id = item.entry.id.clone();
        self.restyle(serial);
        let (root, sender, hwnd) = (self.root.clone(), self.sender.clone(), self.hwnd.0 as isize);
        std::thread::spawn(move || {
            let result = captures_app::execute(captures_app::Request::Delete { root, id })
                .map(|_| ())
                .map_err(|e| e.to_string());
            let _ = sender.send(Done::Deleted(serial, result));
            post(hwnd, WM_APP_DONE);
        });
    }

    /// Shipping `clearAllHistory`.
    /// Leave the Delete all confirmation. Cancel disappears with it, so focus
    /// on Cancel moves to Delete all instead of pointing at a removed control.
    fn disarm_all(&mut self) {
        self.confirm_all = None;
        if self.focus == Some(Target::Cancel) {
            self.set_focus(Some(Target::DeleteAll), self.focus_visible);
        }
        self.events.push(Event::Name(Target::DeleteAll));
        self.events.push(Event::Structure);
        self.relayout = true;
    }

    fn delete_all_step(&mut self) {
        if self.items.is_empty() {
            return;
        }
        if self.confirm_all.is_none() {
            self.confirm_all =
                Some(Instant::now() + Duration::from_millis(history_view::CONFIRM_TIMEOUT_MS));
            self.events.push(Event::Name(Target::DeleteAll));
            self.events.push(Event::Structure);
            self.arm_timer();
            self.relayout = true;
            return;
        }
        self.deleting_all = true;
        self.error = None;
        self.relayout = true;
        self.events.push(Event::Name(Target::DeleteAll));
        let (root, sender, hwnd) = (self.root.clone(), self.sender.clone(), self.hwnd.0 as isize);
        std::thread::spawn(move || {
            let result = captures_app::execute(captures_app::Request::ClearHistory { root })
                .map(|_| ())
                .map_err(|e| e.to_string());
            let _ = sender.send(Done::Cleared(result));
            post(hwnd, WM_APP_DONE);
        });
    }

    fn run_action(&mut self, serial: u32, action: CardAction) {
        if action == CardAction::Edit {
            // One window per artifact; the run loop opens or raises it.
            self.open_editor = self.item(serial).map(|item| item.entry.id.clone());
            return;
        }
        // A settings file that exists but can't be read must not send Save
        // file to the default Captures folder; only a missing file means
        // defaults.
        let directory = captures_settings::load(&self.settings_file)
            .map(|settings| PathBuf::from(settings.output_directory))
            .map_err(|error| format!("Couldn’t read settings: {error}"));
        let Some(item) = self.item_mut(serial) else {
            return;
        };
        let directory = match directory {
            Ok(directory) => directory,
            Err(error) if action == CardAction::SaveFile => {
                item.error = Some(error);
                // As a failed save: an error grows the card's row.
                self.relayout = true;
                self.restyle(serial);
                self.events.push(Event::Structure);
                self.events.push(Event::Alert(Some(serial)));
                return;
            }
            Err(_) => PathBuf::new(),
        };
        item.busy = Some(Busy::Action(action));
        item.error = None;
        let (id, saved) = (item.entry.id.clone(), item.entry.saved_path.clone());
        self.restyle(serial);
        let i = self
            .item(serial)
            .and_then(|it| it.card.actions.iter().position(|&a| a == action))
            .unwrap_or(0);
        self.events.push(Event::Name(Target::Action(serial, i)));
        let (root, sender, hwnd) = (self.root.clone(), self.sender.clone(), self.hwnd.0 as isize);
        std::thread::spawn(move || {
            let done = match action {
                // Shipping `save_recording_artifact`: a permanent copy in the
                // Captures folder.
                CardAction::SaveFile => {
                    let request = captures_app::Request::SaveRecording {
                        root,
                        id,
                        directory,
                    };
                    Done::Saved(
                        serial,
                        match captures_app::execute(request) {
                            Ok(captures_app::Response::Saved { path, .. }) => {
                                Ok(path.to_string_lossy().into_owned())
                            }
                            Ok(_) => Err("unexpected response".into()),
                            Err(error) => Err(error.to_string()),
                        },
                    )
                }
                _ => Done::Shown(
                    serial,
                    match saved {
                        Some(path) => reveal(&path),
                        None => Err(captures_app::Error::Missing.to_string()),
                    },
                ),
            };
            let _ = sender.send(done);
            post(hwnd, WM_APP_DONE);
        });
    }

    fn finish(&mut self, done: Done) {
        match done {
            Done::Loaded(Ok(entries)) => {
                self.error = None;
                self.set_entries(entries);
            }
            Done::Loaded(Err(error)) => {
                self.loading = false;
                self.error = Some(history_view::load_error(&error));
                self.events.push(Event::Alert(None));
                self.refilter();
            }
            Done::Deleted(serial, Ok(())) => {
                self.forget_thumb(serial);
                self.items.retain(|item| item.serial != serial);
                self.refilter();
            }
            Done::Cleared(Ok(())) => {
                self.deleting_all = false;
                self.confirm_all = None;
                for serial in self.items.iter().map(|i| i.serial).collect::<Vec<_>>() {
                    self.forget_thumb(serial);
                }
                self.items.clear();
                self.refilter();
            }
            Done::Cleared(Err(error)) => {
                self.deleting_all = false;
                self.error = Some(history_view::clear_error(&error));
                self.events.push(Event::Alert(None));
                self.disarm_all();
            }
            Done::Saved(serial, Ok(path)) => {
                if let Some(item) = self.item_mut(serial) {
                    item.busy = None;
                    item.entry.saved_path = Some(path);
                    item.card = history_view::card(&item.entry, item.card.missing);
                }
                self.restyle(serial);
                self.events.push(Event::Structure);
            }
            Done::Shown(serial, Ok(())) => {
                if let Some(item) = self.item_mut(serial) {
                    item.busy = None;
                }
                self.restyle(serial);
                self.events.push(Event::Name(Target::Action(serial, 1)));
            }
            Done::Deleted(serial, Err(error))
            | Done::Saved(serial, Err(error))
            | Done::Shown(serial, Err(error)) => {
                if let Some(item) = self.item_mut(serial) {
                    item.busy = None;
                    item.confirm = None;
                    item.error = Some(error);
                }
                // An error grows the card's row.
                self.relayout = true;
                self.restyle(serial);
                self.events.push(Event::Structure);
                self.events.push(Event::Alert(Some(serial)));
            }
        }
        self.need = true;
    }

    /// Expire two-step deletes and re-arm the timer for the next deadline.
    fn expire(&mut self) {
        let now = Instant::now();
        if self.confirm_all.is_some_and(|at| at <= now) {
            self.disarm_all();
        }
        let expired: Vec<u32> = self
            .items
            .iter()
            .filter(|item| item.confirm.is_some_and(|at| at <= now))
            .map(|item| item.serial)
            .collect();
        for serial in expired {
            if let Some(item) = self.item_mut(serial) {
                item.confirm = None;
            }
            self.events.push(Event::Name(Target::Delete(serial)));
            self.restyle(serial);
        }
        self.arm_timer();
        self.need = true;
    }

    fn arm_timer(&mut self) {
        let next = self
            .items
            .iter()
            .filter_map(|item| item.confirm)
            .chain(self.confirm_all)
            .min();
        unsafe {
            match next {
                Some(at) if !self.hwnd.is_invalid() => {
                    let ms = at.saturating_duration_since(Instant::now()).as_millis() as u32;
                    SetTimer(Some(self.hwnd), TIMER_CONFIRM, ms.max(1), None);
                }
                _ if !self.hwnd.is_invalid() => {
                    let _ = KillTimer(Some(self.hwnd), TIMER_CONFIRM);
                }
                _ => {}
            }
        }
    }

    /// Redraw the card showing `serial`, if it is bound.
    fn restyle(&mut self, serial: u32) {
        if let Some(slot) = self.slots.iter().find(|s| s.serial == Some(serial)) {
            for node in slot.nodes() {
                self.nodes[node].dirty = true;
            }
        }
        self.refresh(false);
        self.need = true;
    }

    fn forget_thumb(&mut self, serial: u32) {
        self.thumbs.cache.retain(|(s, _)| *s != serial);
        self.thumbs.failed.remove(&serial);
    }

    // ---- layout ----

    pub fn layout(&mut self, w: f32, h: f32) -> Result<()> {
        self.w = w;
        self.h = h;
        self.compact = w <= COMPACT_MAX;
        self.measure(w)?;
        if self.content_h > h {
            // A classic scrollbar takes its 10 px out of the page.
            self.measure(w - SCROLLBAR)?;
        }
        self.nodes[self.canvas].rect = Rect::new(0., 0., w, h);
        let scroll = self.scroll;
        self.scroll = -1.;
        self.set_scroll(scroll);
        for node in &mut self.nodes {
            node.dirty = true;
            node.props = true;
        }
        self.motions.clear();
        self.sync_slots();
        self.refresh(false);
        Ok(())
    }

    fn measure(&mut self, avail: f32) -> Result<()> {
        let t = self.t.clone();
        let g = self.g.clone();
        let n = |name: &str| t.n(name);
        let (s3, s5, s6, s8) = (n("s-3"), n("s-5"), n("s-6"), n("s-8"));
        let h_md = n("h-md");
        let copy = history_view::copy();
        let inner = (avail - 2. * s8).max(0.);
        let shell_w = inner.min(SHELL_MAX);
        let shell_x = s8 + (inner - shell_w) / 2.;
        let mut y = n("s-9");
        self.l.shell = Rect::new(shell_x, y, shell_w, 0.);

        // `.history-heading`: Back (`.ui-btn` ghost), h1, lede.
        let back = g.text(&t, Style::Control, copy.back_to_preferences, 1e4)?;
        self.l.back = Rect::new(shell_x, y, 2. + 2. * s5 + ICON + s3 + back.width, h_md);
        self.texts.back = Some(back);
        let actions = self.has_entries();
        let delete_label = g.text(&t, Style::Button, self.delete_all_label(), 1e4)?;
        let delete_w = 2. + 2. * s5 + ICON + s3 + delete_label.width;
        let cancel = g.text(&t, Style::Button, copy.cancel, 1e4)?;
        let cancel_w = 2. + 2. * s5 + cancel.width;
        let confirming = actions && self.confirm_all.is_some();
        let actions_w = if confirming {
            cancel_w + s3 + delete_w
        } else {
            delete_w
        };
        let heading_w = if self.compact || !actions {
            shell_w
        } else {
            (shell_w - actions_w - s8).max(0.)
        };
        let title = g.text(&t, Style::Heading, copy.title, 1e4)?;
        let ch = g.ch(&t, Style::Lede)?;
        let lede = g.text(&t, Style::Lede, copy.lede, (60. * ch).min(heading_w))?;
        let title_y = y + h_md + s3;
        self.l.title_h = title.height;
        self.l.heading = Rect::new(shell_x, title_y, heading_w, title.height + s3 + lede.height);
        let heading_bottom = title_y + title.height + s3 + lede.height;
        let actions_y = if self.compact {
            heading_bottom + s5
        } else {
            heading_bottom - h_md
        };
        let actions_x = if self.compact {
            shell_x
        } else {
            shell_x + shell_w - actions_w
        };
        self.l.cancel = Rect::new(actions_x, actions_y, cancel_w, h_md);
        self.l.delete_all = Rect::new(
            actions_x + if confirming { cancel_w + s3 } else { 0. },
            actions_y,
            delete_w,
            h_md,
        );
        y = if self.compact && actions {
            actions_y + h_md
        } else {
            heading_bottom
        };
        self.texts.title = Some(title);
        self.texts.lede = Some(lede);
        self.texts.cancel = Some(cancel);
        self.texts.delete_all = Some(delete_label);

        // `.history-toolbar`: pills with counts, then a 12 px pad and rule.
        let mut filters = Vec::new();
        let mut x = shell_x;
        if actions {
            y += s6;
        }
        for (i, filter) in FILTERS.iter().enumerate() {
            let label = g.text(&t, Style::Button, filter.label(), 1e4)?;
            let count = g.text(&t, Style::Button, &self.count(*filter).to_string(), 1e4)?;
            let w = 2. + 2. * n("s-4") + label.width + s3 + count.width;
            self.l.filters[i] = Rect::new(x, y, w, n("h-sm"));
            x += w + n("s-2");
            filters.push((label, count));
        }
        self.texts.filters = filters;
        self.l.toolbar = Rect::new(shell_x, y, shell_w, n("h-sm") + s5 + 1.);
        if actions {
            y += self.l.toolbar.h;
        }

        // `.history-error`.
        self.texts.error = None;
        if let Some(error) = &self.error {
            let text = g.text(&t, Style::Error, error, (shell_w - 2. * s5).max(1.))?;
            y += s6;
            self.l.error = Rect::new(shell_x, y, shell_w, 2. * n("s-4") + text.height);
            y += self.l.error.h;
            self.texts.error = Some(text);
        }

        // The grid, or the loading, empty and filtered-empty sections.
        y += s6;
        self.grid = history_view::grid_in_window(f64::from(shell_w), self.compact);
        self.extras = RowExtras::default();
        let empty = if self.loading {
            Some((copy.loading, None))
        } else if self.items.is_empty() {
            Some((copy.empty_title, Some(copy.empty_body)))
        } else if self.shown.is_empty() {
            Some((FILTERED_TITLE, Some(FILTERED_BODY)))
        } else {
            None
        };
        self.texts.empty = None;
        if let Some((title, body)) = empty {
            let title = g.text(&t, Style::CardTitle, title, 1e4)?;
            let body = body
                .map(|body| g.text(&t, Style::Body, body, 1e4))
                .transpose()?;
            let block = EMPTY_BOX + s5 + title.height + body.as_ref().map_or(0., |b| s3 + b.height);
            self.l.empty = Rect::new(shell_x, y, shell_w, block.max(EMPTY_MIN));
            y += self.l.empty.h;
            self.texts.empty = Some((title, body));
            self.l.grid_y = y;
        } else {
            let card_w = self.grid.card_width as f32;
            for (k, &i) in self.shown.iter().enumerate() {
                let item = &self.items[i];
                let mut extra = if item.card.warning.is_some() {
                    history_view::CARD_WARNING_EXTRA
                } else {
                    0.
                };
                if let Some(error) = &item.error {
                    let text = g.text(
                        &t,
                        Style::Note,
                        error,
                        (card_w - 2. * s5 - 2. - 16.).max(1.),
                    )?;
                    extra += history_view::card_error_extra(f64::from(text.height));
                }
                self.extras.grow(k / self.grid.columns, extra);
            }
            self.l.grid_y = y;
            y += self
                .grid
                .content_height_with(self.shown.len(), &self.extras) as f32;
        }
        self.content_h = y + n("s-11");
        Ok(())
    }

    /// A card's rect in content coordinates.
    pub fn card_rect(&self, k: usize) -> Rect {
        let (x, y) = self.grid.origin_with(k, &self.extras);
        let row = k / self.grid.columns;
        Rect::new(
            self.l.shell.x + x as f32,
            self.l.grid_y + y as f32,
            self.grid.card_width as f32,
            self.grid.row_card_height(row, &self.extras) as f32,
        )
    }

    /// Card-relative rects of the trash button and the two actions.
    pub fn card_parts(&self, k: usize) -> (Rect, [Rect; 2]) {
        let t = &self.t;
        let (s3, s5, h_md) = (t.n("s-3"), t.n("s-5"), t.n("h-md"));
        let w = self.grid.card_width as f32;
        let delete = Rect::new(w - 1. - s3 - h_md, 1. + s3, h_md, h_md);
        let warning = self.items[self.shown[k]].card.warning.is_some();
        let top = history_view::CARD_ACTIONS_TOP as f32
            + if warning {
                history_view::CARD_WARNING_EXTRA as f32
            } else {
                0.
            };
        let aw = ((w - 2. - 2. * s5 - s3) / 2.).max(0.);
        let actions = [0., 1.].map(|i| Rect::new(1. + s5 + i * (aw + s3), top, aw, h_md));
        (delete, actions)
    }

    /// A target's rect in client coordinates.
    pub fn target_rect(&self, target: Target) -> Rect {
        let r = match target {
            Target::Back => self.l.back,
            Target::Cancel => self.l.cancel,
            Target::DeleteAll => self.l.delete_all,
            Target::Filter(i) => self.l.filters[i],
            Target::Delete(serial) | Target::Action(serial, _) => {
                let Some(k) = self.shown_index(serial) else {
                    return Rect::default();
                };
                let card = self.card_rect(k);
                let (delete, actions) = self.card_parts(k);
                let part = match target {
                    Target::Action(_, i) => actions[i.min(1)],
                    _ => delete,
                };
                Rect::new(card.x + part.x, card.y + part.y, part.w, part.h)
            }
        };
        self.client(r)
    }

    /// Content coordinates to client coordinates.
    pub fn client(&self, r: Rect) -> Rect {
        Rect::new(r.x, r.y - self.scroll, r.w, r.h)
    }

    pub fn max_scroll(&self) -> f32 {
        (self.content_h - self.h).max(0.)
    }

    pub fn set_scroll(&mut self, value: f32) {
        let target = (value.clamp(0., self.max_scroll()) * self.scale).round() / self.scale;
        if target == self.scroll {
            return;
        }
        self.scroll = target;
        self.nodes[self.content].rect.y = -target;
        self.nodes[self.content].props = true;
        let max = self.max_scroll();
        let track = self.h - 6.;
        let bar_h = (self.h * self.h / self.content_h.max(1.) - 6.)
            .max(18.)
            .min(track);
        let bar_y = 3.
            + if max > 0. {
                (track - bar_h) * target / max
            } else {
                0.
            };
        let bar = &mut self.nodes[self.scrollbar];
        if bar.rect.h != bar_h {
            bar.dirty = true;
        }
        bar.rect = Rect::new(self.w - SCROLLBAR + 3., bar_y, 4., bar_h);
        bar.visible = max > 0.;
        bar.props = true;
        self.need = true;
    }

    pub fn scroll_by(&mut self, delta: f32) {
        self.set_scroll(self.scroll + delta);
        if let Some((x, y)) = self.pointer {
            self.set_pointer(Some((x, y)));
        }
    }

    /// Scroll the least distance that shows `r` (content coordinates).
    fn reveal(&mut self, r: Rect) {
        let margin = self.t.n("s-6");
        if r.y - margin < self.scroll {
            self.set_scroll(r.y - margin);
        } else if r.y + r.h + margin > self.scroll + self.h {
            self.set_scroll(r.y + r.h + margin - self.h);
        }
    }

    /// Bind card slots to the cards in the viewport and release the rest.
    fn sync_slots(&mut self) {
        let mut needed: Vec<(usize, u32)> = Vec::new();
        if self.texts.empty.is_none() {
            let mut top = self.scroll - self.l.grid_y;
            let mut height = self.h;
            if top < 0. {
                height += top;
                top = 0.;
            }
            if height > 0. {
                let rows = self.grid.visible_rows_with(
                    self.shown.len(),
                    f64::from(top),
                    f64::from(height),
                    &self.extras,
                );
                let columns = self.grid.columns;
                for k in rows.start * columns..(rows.end * columns).min(self.shown.len()) {
                    needed.push((k, self.items[self.shown[k]].serial));
                }
            }
        }
        let keep: HashSet<u32> = needed.iter().map(|&(_, s)| s).collect();
        for index in 0..self.slots.len() {
            if let Some(serial) = self.slots[index].serial
                && !keep.contains(&serial)
            {
                self.unbind(index);
            }
        }
        for (k, serial) in needed {
            let slot = match self.slots.iter().position(|s| s.serial == Some(serial)) {
                Some(slot) => slot,
                None => {
                    let slot = self
                        .slots
                        .iter()
                        .position(|s| s.serial.is_none())
                        .unwrap_or_else(|| self.add_slot());
                    self.bind(slot, serial);
                    slot
                }
            };
            self.place_slot(slot, k);
        }
        let s = self.scale;
        let wanted: HashMap<u32, (u32, u32)> = self
            .slots
            .iter()
            .filter_map(|slot| slot.serial)
            .map(|serial| (serial, self.thumb_size(s)))
            .collect();
        *self.thumbs.wanted.lock().expect("wanted") = wanted;
    }

    /// Thumbnail pixel size: `object-fit: contain` inside the image wrap.
    fn thumb_size(&self, s: f32) -> (u32, u32) {
        let w = (self.grid.card_width as f32 - 2.).max(1.);
        let h = history_view::CARD_IMAGE_HEIGHT as f32 - 1.;
        (
            ((w * s).round() as u32).max(1),
            ((h * s).round() as u32).max(1),
        )
    }

    fn bind(&mut self, slot: usize, serial: u32) {
        self.slots[slot].serial = Some(serial);
        let group = self.slots[slot].group;
        self.nodes[group].visible = true;
        self.nodes[group].props = true;
        for node in self.slots[slot].nodes() {
            self.nodes[node].dirty = true;
            self.nodes[node].props = true;
        }
        self.request_thumb(serial);
    }

    fn unbind(&mut self, slot: usize) {
        self.slots[slot].serial = None;
        let group = self.slots[slot].group;
        self.nodes[group].visible = false;
        self.nodes[group].props = true;
        let nodes = self.slots[slot].nodes();
        for node in nodes {
            self.release.push(node);
        }
        // Motions on released nodes end with them.
        let slot_nodes: HashSet<usize> = self.slots[slot]
            .nodes()
            .into_iter()
            .chain([group])
            .collect();
        self.motions.retain(|m| !slot_nodes.contains(&m.node));
    }

    fn set_rect(&mut self, node: usize, r: Rect) {
        let n = &mut self.nodes[node];
        if n.rect != r {
            if (n.rect.w, n.rect.h) != (r.w, r.h) {
                n.dirty = true;
            }
            n.rect = r;
            n.props = true;
        }
    }

    fn place_slot(&mut self, slot: usize, k: usize) {
        let card = self.card_rect(k);
        let serial = self.slots[slot].serial;
        let lifted = serial.is_some() && self.hover_card == serial;
        let lift = if lifted { LIFT } else { 0. };
        let moving = self
            .motions
            .iter()
            .any(|m| m.node == self.slots[slot].group && m.prop == Prop::OffsetY);
        let group = self.slots[slot].group;
        let r = Rect::new(card.x, card.y - lift, card.w, card.h);
        if !moving {
            self.set_rect(group, r);
        } else {
            // A running lift keeps going; only the size may change.
            let n = &mut self.nodes[group];
            n.rect.x = r.x;
            n.rect.w = r.w;
            n.rect.h = r.h;
        }
        let whole = Rect::new(0., 0., card.w, card.h);
        let (delete, actions) = self.card_parts(k);
        let s = &self.slots[slot];
        let (shadow, base, outline, image, del, acts) =
            (s.shadow, s.base, s.outline, s.image, s.delete, s.actions);
        for node in [shadow, base, outline] {
            self.set_rect(node, whole);
        }
        self.set_rect(
            image,
            Rect::new(1., 1., card.w - 2., history_view::CARD_IMAGE_HEIGHT as f32),
        );
        self.set_rect(del[0], delete);
        self.set_rect(del[1], Rect::new(0., 0., delete.w, delete.h));
        self.set_rect(del[2], Rect::new(0., 0., delete.w, delete.h));
        let count = serial
            .and_then(|s| self.item(s))
            .map_or(0, |it| it.card.actions.len());
        for (i, a) in acts.iter().enumerate() {
            self.set_rect(a[0], actions[i]);
            self.set_rect(a[1], Rect::new(0., 0., actions[i].w, actions[i].h));
            self.set_rect(a[2], Rect::new(0., 0., actions[i].w, actions[i].h));
            let shown = i < count;
            if self.nodes[a[0]].visible != shown {
                self.nodes[a[0]].visible = shown;
                self.nodes[a[0]].props = true;
            }
        }
    }

    // ---- thumbnails ----

    fn request_thumb(&mut self, serial: u32) {
        // The offscreen renderer decodes its visible cards itself.
        if self.hwnd.is_invalid() {
            return;
        }
        let s = self.scale;
        let size = self.thumb_size(s);
        if let Some(position) = self.thumbs.cache.iter().position(|(c, _)| *c == serial) {
            // Most recently used last.
            let entry = self.thumbs.cache.remove(position).expect("cached");
            let fits = (entry.1.w, entry.1.h) == fit(size, (entry.1.w, entry.1.h));
            self.thumbs.cache.push_back(entry);
            if fits {
                return;
            }
        }
        if self.thumbs.failed.contains(&serial) || !self.thumbs.pending.insert(serial) {
            return;
        }
        let Some(item) = self.item(serial) else {
            return;
        };
        let (id, gray) = (item.entry.id.clone(), item.card.missing);
        // Wanted before it is queued, so the worker never skips it.
        if let Ok(mut wanted) = self.thumbs.wanted.lock() {
            wanted.insert(serial, size);
        }
        if self.thumbs.requests.is_none() {
            let (requests, queue) = channel();
            let (root, wanted, results, hwnd) = (
                self.root.clone(),
                self.thumbs.wanted.clone(),
                self.thumbs.sender.clone(),
                self.hwnd.0 as isize,
            );
            std::thread::spawn(move || thumb_worker(root, queue, wanted, results, hwnd));
            self.thumbs.requests = Some(requests);
        }
        if let Some(requests) = &self.thumbs.requests {
            let _ = requests.send((serial, id, gray, size));
        }
    }

    fn thumbs_ready(&mut self) {
        let results: Vec<ThumbResult> = self.thumbs.results.try_iter().collect();
        for (serial, _size, result) in results {
            self.thumbs.pending.remove(&serial);
            match result {
                Ok(thumb) => {
                    self.thumbs.cache.retain(|(s, _)| *s != serial);
                    self.thumbs.cache.push_back((serial, thumb));
                    while self.thumbs.cache.len() > THUMB_CACHE {
                        self.thumbs.cache.pop_front();
                    }
                }
                // Skipped (scrolled away): asked again when bound again.
                Err(skipped) if skipped.is_empty() => {}
                Err(error) => {
                    eprintln!("thumbnail failed: {error}");
                    self.thumbs.failed.insert(serial);
                }
            }
            if let Some(slot) = self.slots.iter().find(|s| s.serial == Some(serial)) {
                self.nodes[slot.image].dirty = true;
                self.need = true;
            }
        }
    }

    // ---- interaction ----

    fn curve(&self, name: &str) -> CubicBezier {
        let [x1, y1, x2, y2] = self.t.easing(name);
        CubicBezier::new(x1, y1, x2, y2).unwrap_or(CubicBezier::LINEAR)
    }

    fn animate(&mut self, node: usize, prop: Prop, to: f32, duration: &str) {
        let now = Instant::now();
        let current = |n: &Node| match prop {
            Prop::Opacity => n.opacity,
            Prop::OffsetY => n.rect.y,
        };
        let from = self
            .motions
            .iter()
            .find(|m| m.node == node && m.prop == prop)
            .map_or_else(
                || current(&self.nodes[node]),
                |m| m.value_at(m.progress(now)),
            );
        self.motions.retain(|m| !(m.node == node && m.prop == prop));
        let n = &mut self.nodes[node];
        match prop {
            Prop::Opacity => n.opacity = to,
            Prop::OffsetY => n.rect.y = to,
        }
        n.props = true;
        if self.reduced_motion || (from - to).abs() < 1e-4 || self.hwnd.is_invalid() {
            return;
        }
        let seconds = self.t.n(duration) / 1000.;
        let curve = self.curve("ease-standard");
        self.motions.push(Motion {
            node,
            prop,
            from,
            to,
            start: now,
            seconds,
            curve,
            applied: false,
            fresh: true,
        });
    }

    fn ctl_target(&self, ctl: Ctl) -> Option<Target> {
        let card = |slot: usize| self.slots.get(slot).and_then(|s| s.serial);
        Some(match ctl {
            Ctl::Back => Target::Back,
            Ctl::Cancel => Target::Cancel,
            Ctl::DeleteAll => Target::DeleteAll,
            Ctl::Filter(i) => Target::Filter(i),
            Ctl::Delete(slot) => Target::Delete(card(slot)?),
            Ctl::Action(slot, i) => Target::Action(card(slot)?, i),
        })
    }

    /// Bring hover layers, rings, disabled fades and card lifts to the
    /// current state, on the compositor when `animate`.
    fn refresh(&mut self, animate: bool) {
        for node in 0..self.nodes.len() {
            let (to, duration) = match self.nodes[node].kind {
                Kind::Control(ctl, layer) => {
                    let Some(target) = self.ctl_target(ctl) else {
                        continue;
                    };
                    let enabled = self.enabled(target);
                    match layer {
                        Layer::Ring => {
                            let shown = self.focus_visible && self.focus == Some(target);
                            if self.nodes[node].visible != shown {
                                self.nodes[node].visible = shown;
                                self.nodes[node].props = true;
                            }
                            continue;
                        }
                        Layer::Base => {
                            // `:disabled` opacity, per rule.
                            let faded = match ctl {
                                Ctl::Filter(_) => 0.4,
                                Ctl::Cancel | Ctl::DeleteAll => 0.6,
                                _ => 0.45,
                            };
                            let to = if enabled { 1. } else { faded };
                            if self.nodes[node].opacity != to {
                                self.nodes[node].opacity = to;
                                self.nodes[node].props = true;
                            }
                            continue;
                        }
                        Layer::Lit => {
                            let active =
                                matches!(target, Target::Filter(i) if FILTERS[i] == self.filter);
                            let lit = enabled && !active && self.hover == Some(target);
                            (f32::from(lit), "dur-2")
                        }
                    }
                }
                Kind::Shadow(slot) | Kind::Outline(slot) => {
                    let lit = self.slots[slot].serial.is_some()
                        && self.slots[slot].serial == self.hover_card;
                    (f32::from(lit), "dur-3")
                }
                _ => continue,
            };
            if animate {
                if (self.nodes[node].opacity - to).abs() > 1e-4 {
                    self.animate(node, Prop::Opacity, to, duration);
                }
            } else if self.nodes[node].opacity != to {
                self.nodes[node].opacity = to;
                self.nodes[node].props = true;
            }
        }
    }

    /// The control and card under a client point.
    pub fn hit(&self, x: f32, y: f32) -> (Option<Target>, Option<u32>) {
        let cy = y + self.scroll;
        let inside = |r: Rect| r.contains(x, cy);
        if inside(self.l.back) {
            return (Some(Target::Back), None);
        }
        if self.has_entries() {
            if self.confirm_all.is_some() && inside(self.l.cancel) {
                return (Some(Target::Cancel), None);
            }
            if inside(self.l.delete_all) {
                return (Some(Target::DeleteAll), None);
            }
            for i in 0..FILTERS.len() {
                if inside(self.l.filters[i]) {
                    return (Some(Target::Filter(i)), None);
                }
            }
        }
        if self.texts.empty.is_some() {
            return (None, None);
        }
        for slot in &self.slots {
            let Some(serial) = slot.serial else { continue };
            let Some(k) = self.shown_index(serial) else {
                continue;
            };
            let card = self.card_rect(k);
            if !inside(card) {
                continue;
            }
            let (delete, actions) = self.card_parts(k);
            let (lx, ly) = (x - card.x, cy - card.y);
            if delete.contains(lx, ly) {
                return (Some(Target::Delete(serial)), Some(serial));
            }
            let count = self.item(serial).map_or(0, |it| it.card.actions.len());
            for (i, r) in actions.iter().enumerate().take(count) {
                if r.contains(lx, ly) {
                    return (Some(Target::Action(serial, i)), Some(serial));
                }
            }
            return (None, Some(serial));
        }
        (None, None)
    }

    fn set_pointer(&mut self, pointer: Option<(f32, f32)>) {
        self.pointer = pointer;
        let (hover, card) = pointer.map_or((None, None), |(x, y)| self.hit(x, y));
        if hover == self.hover && card == self.hover_card {
            return;
        }
        self.hover = hover;
        if card != self.hover_card {
            let old = std::mem::replace(&mut self.hover_card, card);
            for serial in [old, card].into_iter().flatten() {
                self.lift(serial);
            }
        }
        self.refresh(true);
        self.need = true;
    }

    /// Animate a card toward its hovered or resting height.
    fn lift(&mut self, serial: u32) {
        let (Some(slot), Some(k)) = (
            self.slots.iter().position(|s| s.serial == Some(serial)),
            self.shown_index(serial),
        ) else {
            return;
        };
        let rest = self.card_rect(k).y;
        let to = if self.hover_card == Some(serial) {
            rest - LIFT
        } else {
            rest
        };
        self.animate(self.slots[slot].group, Prop::OffsetY, to, "dur-3");
    }

    /// Tab order: Back, the header actions, filters, then each card's trash
    /// and actions in DOM order; disabled controls are skipped.
    pub fn focusable(&self) -> Vec<Target> {
        let mut list = vec![Target::Back];
        if self.has_entries() {
            if self.confirm_all.is_some() {
                list.push(Target::Cancel);
            }
            list.push(Target::DeleteAll);
            list.extend((0..FILTERS.len()).map(Target::Filter));
        }
        for &i in &self.shown {
            let item = &self.items[i];
            list.push(Target::Delete(item.serial));
            list.extend((0..item.card.actions.len()).map(|a| Target::Action(item.serial, a)));
        }
        list.retain(|&t| self.enabled(t));
        list
    }

    pub fn set_focus(&mut self, focus: Option<Target>, visible: bool) {
        if focus == self.focus && visible == self.focus_visible {
            return;
        }
        let changed = focus != self.focus;
        self.focus = focus;
        self.focus_visible = visible;
        if let Some(target) = focus {
            if visible {
                let r = self.target_rect(target);
                let content = match target.serial().and_then(|s| self.shown_index(s)) {
                    Some(k) => self.card_rect(k),
                    None => Rect::new(r.x, r.y + self.scroll, r.w, r.h),
                };
                self.reveal(content);
            }
            if changed {
                self.events.push(Event::Focus(target));
            }
        }
        self.refresh(true);
        self.need = true;
    }

    fn focus_step(&mut self, forward: bool) {
        let list = self.focusable();
        if list.is_empty() {
            return;
        }
        let next = match self.focus.and_then(|f| list.iter().position(|&t| t == f)) {
            Some(i) if forward => list[(i + 1) % list.len()],
            Some(i) => list[(i + list.len() - 1) % list.len()],
            None if forward => list[0],
            None => list[list.len() - 1],
        };
        self.set_focus(Some(next), true);
    }

    /// Arrow keys between cards (`Grid::step`), keeping the focused part.
    fn grid_step(&mut self, columns: isize, rows: isize) -> bool {
        let Some(focus) = self.focus else {
            return false;
        };
        let Some(k) = focus.serial().and_then(|s| self.shown_index(s)) else {
            return false;
        };
        let next = self.grid.step(k, self.shown.len(), columns, rows);
        let serial = self.items[self.shown[next]].serial;
        let same = match focus {
            Target::Action(_, i) => Target::Action(serial, i),
            _ => Target::Delete(serial),
        };
        let target = [same, Target::Delete(serial)]
            .into_iter()
            .find(|&t| self.target_exists(t) && self.enabled(t));
        if let Some(target) = target {
            self.set_focus(Some(target), true);
        }
        true
    }

    // ---- drawing ----

    fn draw(&self, dc: &ID2D1DeviceContext, node: usize) -> Result<()> {
        let n = &self.nodes[node];
        let t = &self.t;
        let g = &self.g;
        let c = |name: &str| t.color(name);
        let (w, h) = (n.rect.w, n.rect.h);
        let all = rect(0., 0., w, h);
        let (s3, s5) = (t.n("s-3"), t.n("s-5"));
        let r_md = t.n("r-md");
        let centered = |text: &Text| (h - text.height) / 2.;
        match n.kind {
            Kind::Group | Kind::Slot(_) => {}
            Kind::Canvas => gfx::fill(dc, all, c("surface-canvas"))?,
            Kind::Heading => {
                if let Some(title) = &self.texts.title {
                    gfx::text(dc, title, 0., 0., c("text"))?;
                }
                if let Some(lede) = &self.texts.lede {
                    gfx::text(dc, lede, 0., self.l.title_h + s3, c("text-subtle"))?;
                }
            }
            Kind::Rule => gfx::fill(dc, all, c("border-subtle"))?,
            Kind::Error => {
                gfx::fill_round(dc, all, r_md, c("danger-surface"))?;
                if let Some(text) = &self.texts.error {
                    gfx::text(dc, text, s5, t.n("s-4"), c("danger-text"))?;
                }
            }
            Kind::Empty => {
                if let Some((title, body)) = &self.texts.empty {
                    let block =
                        EMPTY_BOX + s5 + title.height + body.as_ref().map_or(0., |b| s3 + b.height);
                    let top = ((h - block) / 2.).round();
                    let x = ((w - EMPTY_BOX) / 2.).round();
                    let r = rect(x, top, x + EMPTY_BOX, top + EMPTY_BOX);
                    let radius = t.n("r-xl");
                    gfx::fill_round(dc, r, radius, c("surface-raised"))?;
                    gfx::border(dc, r, radius, 1., c("border"))?;
                    let inset = (EMPTY_BOX - EMPTY_ICON) / 2.;
                    gfx::icon(
                        dc,
                        &g.factory,
                        "history",
                        &[],
                        (x + inset, top + inset, EMPTY_ICON),
                        1.6,
                        c("text-subtle"),
                    )?;
                    let y = top + EMPTY_BOX + s5;
                    gfx::text(dc, title, (w - title.width) / 2., y, c("text"))?;
                    if let Some(body) = body {
                        let y = y + title.height + s3;
                        gfx::text(dc, body, (w - body.width) / 2., y, c("text-subtle"))?;
                    }
                }
            }
            Kind::Scrollbar => gfx::fill_round(dc, all, w / 2., c("border-strong"))?,
            Kind::Control(ctl, layer) => self.draw_control(dc, ctl, layer, (w, h), centered)?,
            Kind::Shadow(_) => gfx::shadow(dc, all, t.n("r-xl"), t.shadow("shadow-md"))?,
            Kind::Outline(_) => gfx::border(dc, all, t.n("r-xl"), 1., c("border-strong"))?,
            Kind::CardBase(slot) => self.draw_card(dc, slot, (w, h))?,
            Kind::Image(slot) => self.draw_image(dc, slot, (w, h))?,
        }
        Ok(())
    }

    fn draw_control(
        &self,
        dc: &ID2D1DeviceContext,
        ctl: Ctl,
        layer: Layer,
        (w, h): (f32, f32),
        centered: impl Fn(&Text) -> f32,
    ) -> Result<()> {
        let t = &self.t;
        let g = &self.g;
        let c = |name: &str| t.color(name);
        let all = rect(0., 0., w, h);
        let (s3, s5) = (t.n("s-3"), t.n("s-5"));
        let r_md = t.n("r-md");
        let white = D2D1_COLOR_F {
            r: 1.,
            g: 1.,
            b: 1.,
            a: 1.,
        };
        let lit = layer == Layer::Lit;
        if layer == Layer::Ring {
            let radius = match ctl {
                Ctl::Filter(_) => h / 2.,
                _ => r_md,
            };
            gfx::ring(dc, all, radius, 0., 2., c("surface-canvas"))?;
            return gfx::ring(dc, all, radius, 2., 4., t.color_alpha("theme-accent", 0.55));
        }
        // An icon then a label, from `x`.
        let icon_label =
            |name: &str, label: &Text, x: f32, size: f32, color: D2D1_COLOR_F| -> Result<()> {
                gfx::icon(
                    dc,
                    &g.factory,
                    name,
                    &[],
                    (x, (h - size) / 2., size),
                    1.7,
                    color,
                )?;
                gfx::text(dc, label, x + size + s3, centered(label), color)
            };
        match ctl {
            Ctl::Back => {
                if lit {
                    gfx::fill_round(dc, all, r_md, c("surface-hover"))?;
                }
                let color = c(if lit { "text" } else { "text-muted" });
                if let Some(label) = &self.texts.back {
                    icon_label("chevron-left", label, 1. + s5, ICON, color)?;
                }
            }
            Ctl::Cancel => {
                if lit {
                    gfx::fill_round(dc, all, r_md, c("surface-hover"))?;
                }
                if let Some(label) = &self.texts.cancel {
                    let color = c(if lit { "text" } else { "text-muted" });
                    gfx::text(dc, label, 1. + s5, centered(label), color)?;
                }
            }
            Ctl::DeleteAll => {
                let confirm = self.confirm_all.is_some() && !self.deleting_all;
                let (bg, ink) = match (confirm, lit) {
                    (true, false) => (Some(c("theme-signal-strong")), white),
                    (true, true) => (Some(c("theme-signal-deep")), white),
                    (false, true) => (Some(c("danger-surface")), c("danger-text")),
                    (false, false) => (None, c("text-subtle")),
                };
                if let Some(bg) = bg {
                    gfx::fill_round(dc, all, r_md, bg)?;
                }
                if let Some(label) = &self.texts.delete_all {
                    icon_label("trash", label, 1. + s5, ICON, ink)?;
                }
            }
            Ctl::Filter(i) => {
                let active = FILTERS[i] == self.filter;
                let pill = h / 2.;
                if active && !lit {
                    gfx::shadow(dc, all, pill, t.shadow("shadow-xs"))?;
                    gfx::fill_round(dc, all, pill, c("surface-raised"))?;
                    gfx::border(dc, all, pill, 1., c("border"))?;
                } else if lit {
                    gfx::fill_round(dc, all, pill, c("surface-hover"))?;
                }
                if let Some((label, count)) = self.texts.filters.get(i) {
                    let color = c(if active || lit { "text" } else { "text-subtle" });
                    let x = 1. + t.n("s-4");
                    gfx::text(dc, label, x, centered(label), color)?;
                    let x = x + label.width + s3;
                    gfx::text(dc, count, x, centered(count), c("text-faint"))?;
                }
            }
            Ctl::Delete(slot) => {
                let Some(item) = self.slots[slot].serial.and_then(|s| self.item(s)) else {
                    return Ok(());
                };
                let ink = if lit {
                    gfx::fill_round(dc, all, r_md, c("theme-signal"))?;
                    white
                } else {
                    gfx::shadow(dc, all, r_md, t.shadow("shadow-sm"))?;
                    if item.confirm.is_some() {
                        gfx::fill_round(dc, all, r_md, c("theme-signal-strong"))?;
                    } else {
                        gfx::fill_round(dc, all, r_md, t.color_alpha("surface-raised", 0.88))?;
                    }
                    gfx::border(dc, all, r_md, 1., c("border-subtle"))?;
                    if item.confirm.is_some() {
                        white
                    } else {
                        c("text")
                    }
                };
                let inset = (w - DELETE_ICON) / 2.;
                gfx::icon(
                    dc,
                    &g.factory,
                    "trash",
                    &[],
                    (inset, (h - DELETE_ICON) / 2., DELETE_ICON),
                    1.7,
                    ink,
                )?;
            }
            Ctl::Action(slot, i) => {
                let Some(item) = self.slots[slot].serial.and_then(|s| self.item(s)) else {
                    return Ok(());
                };
                let Some(&action) = item.card.actions.get(i) else {
                    return Ok(());
                };
                let primary = action == CardAction::Edit;
                let ink = if primary {
                    if !lit {
                        gfx::shadow(dc, all, r_md, t.shadow("shadow-sm"))?;
                    }
                    let bg = if lit {
                        "theme-accent-hover"
                    } else {
                        "theme-accent"
                    };
                    gfx::fill_round(dc, all, r_md, c(bg))?;
                    c("theme-accent-ink")
                } else {
                    if !lit {
                        gfx::shadow(dc, all, r_md, t.shadow("shadow-xs"))?;
                    }
                    let (bg, edge) = if lit {
                        ("control-hover", "border-strong")
                    } else {
                        ("control", "control-border")
                    };
                    gfx::fill_round(dc, all, r_md, c(bg))?;
                    gfx::border(dc, all, r_md, 1., c(edge))?;
                    c("text")
                };
                let busy = item.busy == Some(Busy::Action(action));
                let label = item.action_label(action);
                let style = if primary {
                    Style::Strong
                } else {
                    Style::Control
                };
                let text = g.text(t, style, label, 1e4)?;
                // Shipping drops the save icon while saving.
                let icon = action
                    .icon()
                    .filter(|_| !(busy && action == CardAction::SaveFile));
                let pad = 1. + t.n("s-4");
                let icon_w = icon.map_or(0., |_| ICON + s3);
                let room = (w - 2. * pad - icon_w).max(1.);
                if text.width > room {
                    g.ellipsis(&text, room)?;
                }
                let content = icon_w + text.width.min(room);
                let x = ((w - content) / 2.).max(pad);
                match icon {
                    Some(name) => icon_label(name, &text, x, ICON, ink)?,
                    None => gfx::text(dc, &text, x, centered(&text), ink)?,
                }
            }
        }
        Ok(())
    }

    fn draw_card(&self, dc: &ID2D1DeviceContext, slot: usize, (w, h): (f32, f32)) -> Result<()> {
        let Some(item) = self.slots[slot].serial.and_then(|s| self.item(s)) else {
            return Ok(());
        };
        let t = &self.t;
        let g = &self.g;
        let c = |name: &str| t.color(name);
        let all = rect(0., 0., w, h);
        let radius = t.n("r-xl");
        let (s3, s5) = (t.n("s-3"), t.n("s-5"));
        gfx::shadow(dc, all, radius, t.shadow("shadow-sm"))?;
        gfx::fill_round(dc, all, radius, c("surface-raised"))?;
        gfx::border(dc, all, radius, 1., c("border-subtle"))?;
        let x = 1. + s5;
        let width = (w - 2. * x).max(1.);
        let mut y = (history_view::CARD_IMAGE_HEIGHT + history_view::CARD_DIVIDER) as f32 + s5;
        let date = g.text(t, Style::Date, &item.card.date, 1e4)?;
        if date.width > width {
            g.ellipsis(&date, width)?;
        }
        gfx::text(dc, &date, x, y + 1., c("text"))?;
        y += history_view::CARD_DATE_HEIGHT as f32 + t.n("s-2");
        let details = g.text(t, Style::Subtitle, &item.card.details, 1e4)?;
        gfx::text(dc, &details, x, y, c("text-subtle"))?;
        if let Some(warning) = &item.card.warning {
            y += history_view::CARD_LINE_HEIGHT as f32 + t.n("s-2");
            let warning = g.text(t, Style::Subtitle, warning, 1e4)?;
            gfx::text(dc, &warning, x, y, c("caution-text"))?;
        }
        if let Some(error) = &item.error {
            let k = self.shown_index(item.serial).unwrap_or(0);
            let top = self.card_parts(k).1[0].y + t.n("h-md") + s3;
            let pad_x = history_view::CARD_ERROR_PADDING_X as f32;
            let pad_y = history_view::CARD_ERROR_PADDING_Y as f32;
            let text = g.text(t, Style::Note, error, (width - 2. * pad_x).max(1.))?;
            let r = rect(x, top, x + width, top + 2. * pad_y + text.height);
            gfx::fill_round(dc, r, t.n("r-sm"), c("danger-surface"))?;
            gfx::text(dc, &text, x + pad_x, top + pad_y, c("danger-text"))?;
        }
        Ok(())
    }

    fn draw_image(&self, dc: &ID2D1DeviceContext, slot: usize, (w, h): (f32, f32)) -> Result<()> {
        let Some(item) = self.slots[slot].serial.and_then(|s| self.item(s)) else {
            return Ok(());
        };
        let t = &self.t;
        let g = &self.g;
        let c = |name: &str| t.color(name);
        let area = rect(0., 0., w, h - 1.);
        let wrap = rounded_top(&g.factory, area, t.n("r-xl") - 1.)?;
        unsafe {
            dc.FillGeometry(&wrap, &gfx::brush(dc, c("surface-sunken"))?, None);
        }
        let thumb = self
            .thumbs
            .cache
            .iter()
            .find(|(s, _)| Some(*s) == self.slots[slot].serial)
            .map(|(_, thumb)| thumb);
        if let Some(thumb) = thumb {
            self.draw_thumb(dc, thumb, area, item.card.missing)?;
        }
        gfx::fill(dc, rect(0., h - 1., w, h), c("border-subtle"))?;
        if item.card.missing {
            // `.history-missing-label`.
            let label = g.text(t, Style::Button, history_view::copy().missing, 1e4)?;
            let (px, py) = (t.n("s-4"), t.n("s-3"));
            let (lw, lh) = (label.width + 2. * px + 2., label.height + 2. * py + 2.);
            let (x, y) = (((w - lw) / 2.).round(), ((h - 1. - lh) / 2.).round());
            let r = rect(x, y, x + lw, y + lh);
            gfx::fill_round(dc, r, t.n("r-md"), c("surface-raised"))?;
            gfx::border(dc, r, t.n("r-md"), 1., c("border"))?;
            gfx::text(dc, &label, x + 1. + px, y + 1. + py, c("text-muted"))?;
        }
        Ok(())
    }

    /// `object-fit: contain`, clipped to the wrap's rounded top corners;
    /// missing recordings at 30% (shipping `.history-image-missing img`).
    fn draw_thumb(
        &self,
        dc: &ID2D1DeviceContext,
        thumb: &Thumb,
        area: D2D_RECT_F,
        missing: bool,
    ) -> Result<()> {
        if thumb.w == 0 || thumb.h == 0 {
            return Ok(());
        }
        let (aw, ah) = (area.right - area.left, area.bottom - area.top);
        let fit = (aw / thumb.w as f32).min(ah / thumb.h as f32);
        let (dw, dh) = (thumb.w as f32 * fit, thumb.h as f32 * fit);
        let (dx, dy) = (area.left + (aw - dw) / 2., area.top + (ah - dh) / 2.);
        unsafe {
            let bitmap = dc.CreateBitmap(
                D2D_SIZE_U {
                    width: thumb.w,
                    height: thumb.h,
                },
                Some(thumb.pixels.as_ptr() as *const _),
                thumb.w * 4,
                &D2D1_BITMAP_PROPERTIES1 {
                    pixelFormat: D2D1_PIXEL_FORMAT {
                        format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
                        alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                    },
                    dpiX: 96.,
                    dpiY: 96.,
                    bitmapOptions: D2D1_BITMAP_OPTIONS_NONE,
                    colorContext: std::mem::ManuallyDrop::new(None),
                },
            )?;
            let brush = dc.CreateBitmapBrush(
                &bitmap,
                Some(&D2D1_BITMAP_BRUSH_PROPERTIES1 {
                    extendModeX: D2D1_EXTEND_MODE_CLAMP,
                    extendModeY: D2D1_EXTEND_MODE_CLAMP,
                    interpolationMode: D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC,
                }),
                None,
            )?;
            brush.SetTransform(&Matrix3x2 {
                M11: fit,
                M12: 0.,
                M21: 0.,
                M22: fit,
                M31: dx,
                M32: dy,
            });
            if missing {
                brush.SetOpacity(0.3);
            }
            // The wrap's rounded top corners clip an image that reaches them;
            // no geometry combine, which Wine lacks too.
            let image = rect(dx, dy, dx + dw, dy + dh);
            let radius = self.t.n("r-xl") - 1.;
            if dy - area.top < 0.5 && dx - area.left < radius {
                let clipped = rounded_top(&self.g.factory, image, radius)?;
                dc.FillGeometry(&clipped, &brush, None);
            } else {
                dc.FillRectangle(&image, &brush);
            }
        }
        Ok(())
    }

    /// Paint every visible node into `dc` (the `--screenshot` renderer).
    pub fn paint(&self, dc: &ID2D1DeviceContext, scale: f32) -> Result<()> {
        self.paint_node(dc, scale, 0, (0., 0.))
    }

    fn paint_node(
        &self,
        dc: &ID2D1DeviceContext,
        scale: f32,
        node: usize,
        origin: (f32, f32),
    ) -> Result<()> {
        let n = &self.nodes[node];
        if !n.visible || n.opacity <= 0. {
            return Ok(());
        }
        let (x, y) = (origin.0 + n.rect.x, origin.1 + n.rect.y);
        let layer = n.opacity < 1.;
        unsafe {
            dc.SetTransform(&Matrix3x2::identity());
            if layer {
                dc.PushLayer(
                    &D2D1_LAYER_PARAMETERS1 {
                        contentBounds: rect(-1e6, -1e6, 1e6, 1e6),
                        opacity: n.opacity,
                        maskTransform: Matrix3x2::identity(),
                        ..Default::default()
                    },
                    None::<&ID2D1Layer>,
                );
            }
            if n.kind != Kind::Group {
                dc.SetTransform(&Matrix3x2 {
                    M11: scale,
                    M12: 0.,
                    M21: 0.,
                    M22: scale,
                    M31: (x * scale).round(),
                    M32: (y * scale).round(),
                });
                self.draw(dc, node)?;
            }
            for &child in &self.children[node] {
                self.paint_node(dc, scale, child, (x, y))?;
            }
            dc.SetTransform(&Matrix3x2::identity());
            if layer {
                dc.PopLayer();
            }
        }
        Ok(())
    }

    // ---- composition ----

    fn client_dips(&self) -> (f32, f32) {
        let mut rc = RECT::default();
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::GetClientRect(self.hwnd, &mut rc);
        }
        (rc.right as f32 / self.scale, rc.bottom as f32 / self.scale)
    }

    /// Fixed nodes follow the page state: what shows, and where.
    fn place_fixed(&mut self) {
        let actions = self.has_entries();
        let confirming = actions && self.confirm_all.is_some();
        let l = &self.l;
        let mut set = vec![
            (self.heading, l.heading, true),
            (self.back.base, l.back, true),
            (self.cancel.base, l.cancel, confirming),
            (self.delete_all.base, l.delete_all, actions),
            (
                self.rule,
                Rect::new(l.toolbar.x, l.toolbar.y + l.toolbar.h - 1., l.toolbar.w, 1.),
                actions,
            ),
            (self.error_node, l.error, self.error.is_some()),
            (self.empty, l.empty, self.texts.empty.is_some()),
        ];
        for (i, f) in self.filters.iter().enumerate() {
            set.push((f.base, l.filters[i], actions));
        }
        let layered: Vec<Layered> = [self.back, self.cancel, self.delete_all]
            .into_iter()
            .chain(self.filters.iter().copied())
            .collect();
        for (node, r, visible) in set {
            self.set_rect(node, r);
            if self.nodes[node].visible != visible {
                self.nodes[node].visible = visible;
                self.nodes[node].props = true;
                self.nodes[node].dirty = true;
            }
        }
        for layer in layered {
            let r = self.nodes[layer.base].rect;
            let local = Rect::new(0., 0., r.w, r.h);
            self.set_rect(layer.lit, local);
            self.set_rect(layer.ring, local);
        }
        let grid = self.texts.empty.is_none();
        if self.nodes[self.grid_node].visible != grid {
            self.nodes[self.grid_node].visible = grid;
            self.nodes[self.grid_node].props = true;
        }
    }

    pub fn prepare(&mut self) -> Result<()> {
        if std::mem::take(&mut self.relayout) {
            let (w, h) = if self.hwnd.is_invalid() {
                (self.w, self.h)
            } else {
                self.client_dips()
            };
            self.layout(w, h)?;
        }
        self.place_fixed();
        self.sync_slots();
        Ok(())
    }

    fn frame(&mut self) -> Result<()> {
        self.prepare()?;
        let Some(dcomp) = self.dcomp.clone() else {
            return Ok(());
        };
        let s = self.scale;
        unsafe {
            for node in std::mem::take(&mut self.release) {
                if self.surfaces[node].take().is_some()
                    && let Some(visual) = &self.visuals[node]
                {
                    visual.SetContent(None::<&IUnknown>)?;
                }
            }
        }
        let count = self.nodes.len();
        // Snap edges to device pixels, as Preferences does.
        let mut abs = vec![(0f32, 0f32); count];
        let mut origin = vec![(0f32, 0f32); count];
        let mut snapped = vec![(0f32, 0f32); count];
        let mut shown = vec![true; count];
        for i in 0..count {
            let node = &self.nodes[i];
            let base = node.parent.map_or((0., 0.), |p| abs[p]);
            shown[i] = node.visible && node.parent.is_none_or(|p| shown[p]);
            abs[i] = (base.0 + node.rect.x, base.1 + node.rect.y);
            let bleed = (node.bleed * s).round();
            let (left, top) = ((abs[i].0 * s).round(), (abs[i].1 * s).round());
            snapped[i] = (
                ((abs[i].0 + node.rect.w) * s).round() - left,
                ((abs[i].1 + node.rect.h) * s).round() - top,
            );
            origin[i] = (left - bleed, top - bleed);
        }
        let mut draws = Vec::new();
        unsafe {
            for i in 0..count {
                if self.visuals[i].is_none() {
                    let visual: IDCompositionVisual3 = dcomp.CreateVisual()?.cast()?;
                    match self.nodes[i].parent {
                        // Nodes are created in paint order; with no reference,
                        // `insertAbove = false` puts each above its siblings.
                        Some(parent) => self.visuals[parent]
                            .as_ref()
                            .expect("parent visual")
                            .AddVisual(&visual, false, None::<&IDCompositionVisual>)?,
                        None => self._target.as_ref().expect("target").SetRoot(&visual)?,
                    }
                    self.visuals[i] = Some(visual);
                }
                let visual = self.visuals[i].as_ref().expect("visual");
                let node = &mut self.nodes[i];
                if node.props {
                    node.props = false;
                    let parent = node.parent.map_or((0., 0.), |p| origin[p]);
                    let offset = (origin[i].0 - parent.0, origin[i].1 - parent.1);
                    self.offsets[i] = offset;
                    visual.SetOffsetX2(offset.0)?;
                    visual.SetOffsetY2(offset.1)?;
                    visual.SetOpacity2(node.opacity)?;
                    visual.SetVisible(node.visible)?;
                }
                if matches!(node.kind, Kind::Group | Kind::Slot(_)) || !shown[i] {
                    continue;
                }
                let bleed = 2. * (node.bleed * s).round();
                let size = (
                    (snapped[i].0 + bleed).max(1.) as u32,
                    (snapped[i].1 + bleed).max(1.) as u32,
                );
                if self.surfaces[i]
                    .as_ref()
                    .is_none_or(|(_, have)| *have != size)
                {
                    let surface = self.g.surface(size.0, size.1)?;
                    visual.SetContent(&surface)?;
                    self.surfaces[i] = Some((surface, size));
                    node.dirty = true;
                }
                if node.dirty {
                    node.dirty = false;
                    draws.push(i);
                }
            }
        }
        for i in draws {
            let (surface, size) = self.surfaces[i].as_ref().expect("surface");
            let node = &self.nodes[i];
            let bleed = (node.bleed * s).round();
            let axis = |px: f32, dips: f32| if dips > 0. { px / dips } else { s };
            let scale = (
                axis(snapped[i].0, node.rect.w),
                axis(snapped[i].1, node.rect.h),
            );
            gfx::draw_surface(surface, *size, scale, bleed, |dc| self.draw(dc, i))?;
        }
        self.start_motions(&dcomp)?;
        unsafe { dcomp.Commit()? };
        Ok(())
    }

    fn start_motions(&mut self, dcomp: &IDCompositionDesktopDevice) -> Result<()> {
        let s = self.scale;
        let mut any = false;
        for motion in self.motions.iter_mut().filter(|m| !m.applied) {
            motion.applied = true;
            any = true;
            if motion.fresh {
                motion.fresh = false;
                motion.start = Instant::now();
            }
            let (base, mul) = match motion.prop {
                Prop::Opacity => (0., 1.),
                Prop::OffsetY => (self.offsets[motion.node].1 - motion.to * s, s),
            };
            let value = |p: f32| base + motion.value_at(p) * mul;
            let elapsed = motion.start.elapsed().as_secs_f32().min(motion.seconds);
            let remaining = (motion.seconds - elapsed).max(0.001);
            let start = elapsed / motion.seconds;
            let Some(visual) = &self.visuals[motion.node] else {
                continue;
            };
            unsafe {
                let animation = dcomp.CreateAnimation()?;
                let step = remaining / SEGMENTS as f32;
                for k in 0..SEGMENTS {
                    let (p0, p1) = (
                        start + (1. - start) * k as f32 / SEGMENTS as f32,
                        start + (1. - start) * (k + 1) as f32 / SEGMENTS as f32,
                    );
                    let (v0, v1) = (value(p0), value(p1));
                    animation.AddCubic(f64::from(step * k as f32), v0, (v1 - v0) / step, 0., 0.)?;
                }
                animation.End(f64::from(remaining), value(1.))?;
                match motion.prop {
                    Prop::Opacity => visual.SetOpacity(&animation)?,
                    Prop::OffsetY => visual.SetOffsetY(&animation)?,
                }
            }
        }
        if any && !self.timer {
            // Only to retire finished motions.
            unsafe { SetTimer(Some(self.hwnd), TIMER_MOTION, 16, None) };
            self.timer = true;
        }
        Ok(())
    }

    /// Draw and commit now, waiting for the compositor (resize).
    fn present(&mut self) {
        self.need = false;
        if let Err(error) = self.frame() {
            eprintln!("history frame failed: {error:?}");
        }
        if let Some(dcomp) = &self.dcomp {
            unsafe {
                let _ = dcomp.WaitForCommitCompletion();
            }
        }
    }

    /// One frame if due; the run loop calls this and clears `woken` first.
    /// Returns whether another frame is due, the accessibility events, and
    /// whether Preferences or an editor (by artifact id) should open.
    pub fn service(&mut self) -> (bool, Vec<Event>, bool, Option<String>) {
        if self.need {
            self.need = false;
            if let Err(error) = self.frame() {
                eprintln!("history frame failed: {error:?}");
            }
        }
        (
            self.need,
            std::mem::take(&mut self.events),
            std::mem::take(&mut self.open_preferences),
            self.open_editor.take(),
        )
    }

    /// Settings or the system appearance changed: re-theme every surface.
    pub fn retheme(&mut self) {
        let settings = captures_settings::load(&self.settings_file).unwrap_or_default();
        let tokens = appearance::tokens(self.appearance.as_deref(), &settings);
        appearance::title_bar(self.hwnd, &tokens);
        self.t = tokens;
        self.relayout = true;
        self.need = true;
    }

    fn handle(&mut self, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
        let s = self.scale;
        let x = (lp.0 & 0xffff) as i16 as f32 / s;
        let y = ((lp.0 >> 16) & 0xffff) as i16 as f32 / s;
        let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
        match msg {
            WM_SIZE => {
                if lp.0 & 0xffff != 0 && (lp.0 >> 16) & 0xffff != 0 {
                    self.relayout = true;
                    self.present();
                }
            }
            // Only the run loop clears `woken`.
            WM_APP_WAKE => {}
            WM_APP_DONE => {
                let done: Vec<Done> = self.done.try_iter().collect();
                for done in done {
                    self.finish(done);
                }
            }
            WM_APP_THUMB => self.thumbs_ready(),
            WM_TIMER if wp.0 == TIMER_CONFIRM => self.expire(),
            WM_TIMER if wp.0 == TIMER_MOTION => {
                let now = Instant::now();
                self.motions.retain(|m| m.progress(now) < 1.);
                if self.motions.is_empty() {
                    unsafe {
                        let _ = KillTimer(Some(self.hwnd), TIMER_MOTION);
                    }
                    self.timer = false;
                }
            }
            WM_SETTINGCHANGE => {
                self.reduced_motion = appearance::reduced_motion();
                let settings = captures_settings::load(&self.settings_file).unwrap_or_default();
                let tokens = appearance::tokens(self.appearance.as_deref(), &settings);
                if tokens.appearance != self.t.appearance {
                    self.retheme();
                }
            }
            WM_KILLFOCUS => {
                self.pressed = None;
                return None;
            }
            WM_MOUSEMOVE => {
                if !self.tracking {
                    let mut track = TRACKMOUSEEVENT {
                        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: self.hwnd,
                        dwHoverTime: 0,
                    };
                    unsafe {
                        let _ = TrackMouseEvent(&mut track);
                    }
                    self.tracking = true;
                }
                self.set_pointer(Some((x, y)));
            }
            WM_MOUSELEAVE => {
                self.tracking = false;
                self.set_pointer(None);
            }
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
                unsafe { SetCapture(self.hwnd) };
                let (hit, _) = self.hit(x, y);
                self.pressed = hit.filter(|&t| self.enabled(t));
                // A click focuses the button without showing the ring.
                self.set_focus(self.pressed, false);
            }
            WM_LBUTTONUP => {
                unsafe {
                    let _ = ReleaseCapture();
                }
                let (hit, _) = self.hit(x, y);
                if let Some(pressed) = self.pressed.take()
                    && hit == Some(pressed)
                {
                    self.activate(pressed);
                }
            }
            WM_MOUSEWHEEL => {
                let delta = ((wp.0 >> 16) & 0xffff) as i16 as f32 / 120.;
                // Chromium's 100 px per notch at the default three lines.
                self.scroll_by(-delta * appearance::wheel_lines() * 100. / 3.);
            }
            WM_SETCURSOR => {
                if (lp.0 & 0xffff) as u32 != HTCLIENT {
                    return None;
                }
                let hand = self.hover.is_some_and(|t| self.enabled(t));
                unsafe {
                    SetCursor(LoadCursorW(None, if hand { IDC_HAND } else { IDC_ARROW }).ok());
                }
                return Some(LRESULT(1));
            }
            WM_KEYDOWN => {
                let vk = VIRTUAL_KEY(wp.0 as u16);
                let page = self.h * 0.875;
                match vk {
                    VK_TAB => self.focus_step(!shift),
                    VK_SPACE | VK_RETURN => {
                        if let Some(focus) = self.focus {
                            self.set_focus(Some(focus), true);
                            self.activate(focus);
                        }
                    }
                    VK_LEFT if self.grid_step(-1, 0) => {}
                    VK_RIGHT if self.grid_step(1, 0) => {}
                    VK_UP if self.grid_step(0, -1) => {}
                    VK_DOWN if self.grid_step(0, 1) => {}
                    VK_UP => self.scroll_by(-40.),
                    VK_DOWN => self.scroll_by(40.),
                    VK_PRIOR => self.scroll_by(-page),
                    VK_NEXT => self.scroll_by(page),
                    VK_HOME => self.set_scroll(0.),
                    VK_END => self.set_scroll(f32::MAX),
                    _ => return None,
                }
                self.need = true;
            }
            WM_GETMINMAXINFO => {
                let spec = captures_app::app_windows::HISTORY;
                let size = crate::host::window_size(self.hwnd, spec.min_width, spec.min_height);
                let info = unsafe { &mut *(lp.0 as *mut MINMAXINFO) };
                info.ptMinTrackSize = POINT {
                    x: size.0,
                    y: size.1,
                };
                return Some(LRESULT(0));
            }
            _ => return None,
        }
        Some(LRESULT(0))
    }
}

fn post(hwnd: isize, msg: u32) {
    unsafe {
        let _ = PostMessageW(Some(HWND(hwnd as *mut _)), msg, WPARAM(0), LPARAM(0));
    }
}

/// Entries with whether their media is missing (I/O, off the UI thread).
fn load(root: &std::path::Path) -> std::result::Result<Vec<(HistoryEntry, bool)>, String> {
    let entries = captures_history::load(root, chrono::Utc::now()).map_err(|e| e.to_string())?;
    Ok(entries
        .into_iter()
        .map(|entry| {
            let missing = history_view::media_missing(root, &entry);
            (entry, missing)
        })
        .collect())
}

/// Shipping `reveal_recording_artifact`: select the file in Explorer.
fn reveal(path: &str) -> std::result::Result<(), String> {
    use windows::Win32::UI::Shell::{ILFree, SHOpenFolderAndSelectItems, SHParseDisplayName};
    if !std::path::Path::new(path).exists() {
        return Err(captures_app::Error::Missing.to_string());
    }
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let mut pidl = std::ptr::null_mut();
        SHParseDisplayName(&HSTRING::from(path), None, &mut pidl, 0, None)
            .map_err(|e| e.message())?;
        let result = SHOpenFolderAndSelectItems(pidl, None, 0).map_err(|e| e.message());
        ILFree(Some(pidl));
        result
    }
}

/// The size `src` takes inside `bounds` with `object-fit: contain`.
fn fit(bounds: (u32, u32), src: (u32, u32)) -> (u32, u32) {
    if src.0 == 0 || src.1 == 0 {
        return (0, 0);
    }
    let scale = (bounds.0 as f32 / src.0 as f32).min(bounds.1 as f32 / src.1 as f32);
    (
        ((src.0 as f32 * scale).round() as u32).max(1),
        ((src.1 as f32 * scale).round() as u32).max(1),
    )
}

/// Decode `preview.png` (the screenshot thumbnail or recording poster) at
/// `object-fit: contain` size, grayscale for a missing recording.
fn decode(wic: &IWICImagingFactory, bytes: &[u8], bounds: (u32, u32), gray: bool) -> Result<Thumb> {
    unsafe {
        let stream = wic.CreateStream()?;
        stream.InitializeFromMemory(bytes)?;
        let decoder =
            wic.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand)?;
        let frame = decoder.GetFrame(0)?;
        let (mut iw, mut ih) = (0, 0);
        frame.GetSize(&mut iw, &mut ih)?;
        let (w, h) = fit(bounds, (iw, ih));
        if w == 0 {
            return Err(Error::new(E_FAIL, "empty image"));
        }
        let scaler = wic.CreateBitmapScaler()?;
        scaler.Initialize(&frame, w, h, WICBitmapInterpolationModeFant)?;
        let converter = wic.CreateFormatConverter()?;
        converter.Initialize(
            &scaler,
            &GUID_WICPixelFormat32bppPBGRA,
            WICBitmapDitherTypeNone,
            None,
            0.,
            WICBitmapPaletteTypeCustom,
        )?;
        let mut pixels = vec![0u8; (w * h * 4) as usize];
        converter.CopyPixels(std::ptr::null(), w * 4, &mut pixels)?;
        if gray {
            // `filter: grayscale(1)`: Rec. 709 luma.
            for px in pixels.chunks_exact_mut(4) {
                let y = 0.0722 * f32::from(px[0])
                    + 0.7152 * f32::from(px[1])
                    + 0.2126 * f32::from(px[2]);
                let y = y.round() as u8;
                px[0] = y;
                px[1] = y;
                px[2] = y;
            }
        }
        Ok(Thumb { w, h, pixels })
    }
}

fn thumb_worker(
    root: PathBuf,
    queue: Receiver<ThumbRequest>,
    wanted: Arc<Mutex<HashMap<u32, (u32, u32)>>>,
    results: Sender<ThumbResult>,
    hwnd: isize,
) {
    let wic: Option<IWICImagingFactory> = unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()
    };
    for (serial, id, gray, size) in queue {
        let still = wanted.lock().is_ok_and(|w| w.get(&serial) == Some(&size));
        let result = if !still {
            Err(String::new())
        } else {
            match (&wic, captures_history::read_image(&root, &id, true)) {
                (Some(wic), Ok(bytes)) => {
                    decode(wic, &bytes, size, gray).map_err(|e| e.to_string())
                }
                (None, _) => Err("WIC is unavailable".into()),
                (_, Err(error)) => Err(error.to_string()),
            }
        };
        if results.send((serial, size, result)).is_err() {
            return;
        }
        post(hwnd, WM_APP_THUMB);
    }
}

/// A rounded rectangle with only its top corners rounded.
fn rounded_top(factory: &ID2D1Factory1, r: D2D_RECT_F, radius: f32) -> Result<ID2D1PathGeometry1> {
    let point = |x, y| Vector2 { X: x, Y: y };
    unsafe {
        let geometry = factory.CreatePathGeometry()?;
        let sink = geometry.Open()?;
        sink.BeginFigure(point(r.left, r.bottom), D2D1_FIGURE_BEGIN_FILLED);
        sink.AddLine(point(r.left, r.top + radius));
        let arc = |x, y| D2D1_ARC_SEGMENT {
            point: point(x, y),
            size: D2D_SIZE_F {
                width: radius,
                height: radius,
            },
            rotationAngle: 0.,
            sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
            arcSize: D2D1_ARC_SIZE_SMALL,
        };
        sink.AddArc(&arc(r.left + radius, r.top));
        sink.AddLine(point(r.right - radius, r.top));
        sink.AddArc(&arc(r.right, r.top + radius));
        sink.AddLine(point(r.right, r.bottom));
        sink.EndFigure(D2D1_FIGURE_END_CLOSED);
        sink.Close()?;
        Ok(geometry)
    }
}

fn root_of(options: &Options) -> PathBuf {
    options
        .history_root
        .clone()
        .unwrap_or_else(captures_app::default_history_root)
}

/// Open History, or bring it forward when it is already open.
pub fn open(options: &Options, size: Option<(u32, u32)>) -> Result<()> {
    if let Some(hwnd) = with_history(|h| h.hwnd) {
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
            let _ = SetForegroundWindow(hwnd);
        }
        // Shipping reloads on `capture-history-changed`; reopening refreshes.
        with_history(|h| h.reload());
        return Ok(());
    }
    let settings = captures_settings::load(&options.settings_file).unwrap_or_default();
    let tokens = appearance::tokens(options.appearance.as_deref(), &settings);
    let g = match crate::host::with_host(|h| h.g.clone()) {
        Some(g) => g,
        None => Gfx::new(&tokens, true)?,
    };
    unsafe {
        let instance = windows::Win32::System::LibraryLoader::GetModuleHandleW(None)?;
        let spec = captures_app::app_windows::HISTORY;
        let hwnd = CreateWindowExW(
            WS_EX_NOREDIRECTIONBITMAP,
            w!("CapturesHistory"),
            &HSTRING::from(spec.title),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            spec.width as i32,
            spec.height as i32,
            None,
            None,
            Some(instance.into()),
            None,
        )?;
        let (w, h) = size.map_or((spec.width, spec.height), |(w, h)| (w as f32, h as f32));
        let (ww, wh) = crate::host::window_size(hwnd, w, h);
        let _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            ww,
            wh,
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
        let mut history = History::new(hwnd, g, options, tokens, root_of(options))?;
        history.scale = GetDpiForWindow(hwnd) as f32 / 96.;
        history.reduced_motion = appearance::reduced_motion();
        let dcomp = history.g.dcomp.clone().ok_or_else(Error::empty)?;
        history._target = Some(dcomp.CreateTargetForHwnd(hwnd, true)?);
        history.dcomp = Some(dcomp);
        appearance::title_bar(hwnd, &history.t);
        history.reload();
        HISTORY.with(|h| *h.borrow_mut() = Some(history));
        let _ = ShowWindow(hwnd, SW_SHOW);
    }
    Ok(())
}

pub fn register(instance: HINSTANCE) -> Result<()> {
    unsafe {
        let class = WNDCLASSW {
            style: CS_DBLCLKS,
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: w!("CapturesHistory"),
            ..Default::default()
        };
        RegisterClassW(&class);
    }
    Ok(())
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    use windows::Win32::UI::Accessibility::{UiaReturnRawElementProvider, UiaRootObjectId};
    unsafe {
        match msg {
            WM_GETOBJECT => {
                if lp.0 as i32 == UiaRootObjectId {
                    return UiaReturnRawElementProvider(
                        hwnd,
                        wp,
                        lp,
                        &crate::history_uia::node(crate::history_uia::Id::Root),
                    );
                }
                return DefWindowProcW(hwnd, msg, wp, lp);
            }
            WM_DESTROY => {
                let _ = UiaReturnRawElementProvider(hwnd, WPARAM(0), LPARAM(0), None);
                HISTORY.with(|h| {
                    if let Ok(mut h) = h.try_borrow_mut() {
                        h.take();
                    }
                });
                if !crate::host::exists() && !crate::editor::exists() {
                    PostQuitMessage(0);
                }
                return LRESULT(0);
            }
            WM_ERASEBKGND => return LRESULT(1),
            WM_PAINT => {
                let _ = windows::Win32::Graphics::Gdi::ValidateRect(Some(hwnd), None);
                return LRESULT(0);
            }
            WM_DPICHANGED => {
                let dpi = (wp.0 & 0xffff) as f32;
                with_history(|h| {
                    h.scale = dpi / 96.;
                    h.relayout = true;
                    h.need = true;
                });
                let r = &*(lp.0 as *const RECT);
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                return LRESULT(0);
            }
            _ => {}
        }
        if let Some(Some(result)) = with_history(|h| h.handle(msg, wp, lp)) {
            return result;
        }
        DefWindowProcW(hwnd, msg, wp, lp)
    }
}

/// `--screenshot --scene history`: load, decode the visible thumbnails and
/// paint one frame without a window.
pub fn render_offscreen(
    options: &Options,
    g: Gfx,
    tokens: Tokens,
    (w, h): (f32, f32),
) -> Result<History> {
    let root = root_of(options);
    let mut history = History::new(HWND::default(), g, options, tokens, root.clone())?;
    match load(&root) {
        Ok(entries) => history.set_entries(entries),
        Err(error) => {
            history.loading = false;
            history.error = Some(history_view::load_error(&error));
        }
    }
    history.layout(w, h)?;
    history.relayout = false;
    history.place_fixed();
    let wic: IWICImagingFactory =
        unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)? };
    let size = history.thumb_size(1.);
    let bound: Vec<(u32, String, bool)> = history
        .slots
        .iter()
        .filter_map(|slot| slot.serial)
        .filter_map(|serial| {
            let item = history.item(serial)?;
            Some((serial, item.entry.id.clone(), item.card.missing))
        })
        .collect();
    for (serial, id, gray) in bound {
        match captures_history::read_image(&root, &id, true)
            .map_err(|e| Error::new(E_FAIL, e.to_string()))
            .and_then(|bytes| decode(&wic, &bytes, size, gray))
        {
            Ok(thumb) => history.thumbs.cache.push_back((serial, thumb)),
            Err(error) => eprintln!("thumbnail failed: {error}"),
        }
    }
    Ok(history)
}
