//! The Preferences window as a retained tree of nodes, following shipping
//! `.preferences` (`styles/windows.css`, `styles/primitives.css`) and the copy
//! and option lists in `captures_app::preferences`.
//!
//! Each node with content maps to one DirectComposition surface (see
//! `host.rs`) and is redrawn only when its own `dirty` flag is set. Hover,
//! active, selection and toggle transitions animate node opacity and offsets
//! on the compositor; the only CPU redraw on hover is a 32×19 switch track.
//! The same tree paints offscreen for `--screenshot`.
use std::path::PathBuf;
use std::time::Instant;

use captures_app::controls::select;
use captures_app::motion::CubicBezier;
use captures_app::preferences::{
    self, APPEARANCE_MODES, COUNTDOWN_SECONDS, GIF_FPS, GIF_MAX_WIDTHS, GIF_PALETTE_COLORS,
    RECORDING_FORMATS, RECORDING_FPS, RECORDING_TOGGLES, RESOLUTIONS, SECTIONS, SHORTCUT_ROWS,
    THEMES,
};
use captures_app::shortcuts::{
    ShortcutKeyEvent, ShortcutPlatform, ShortcutRecording, record_shortcut, shortcut_display_tokens,
};
use windows::Win32::Graphics::Direct2D::ID2D1DeviceContext;
use windows::core::Result;

use crate::gfx::{self, Gfx, Style, Text, rect};
use crate::textfield::{Change, Editor, TextField};
use crate::tokens::Tokens;

pub const PLATFORM: ShortcutPlatform = ShortcutPlatform::Windows;
pub const NAV_WIDTH: f32 = 196.;
/// `.setting-row-inline` control column minimum.
const SELECT_MIN: f32 = 160.;
/// `.custom-select-trigger svg` and its path (16-unit box).
const CHEVRON: f32 = 14.;
const CHEVRON_PATH: &str = "m4 6 4 4 4-4";
/// `.shortcut-row` control column: `minmax(180px, 260px)`.
const RECORDER: (f32, f32) = (180., 260.);
/// Room for `--shadow-lg` (0 16px 40px) around the listbox.
const LISTBOX_BLEED: f32 = 36.;
/// Type-ahead keeps extending its prefix while keys come this close together.
const TYPE_AHEAD_SECONDS: f32 = 1.;
const SWITCH: (f32, f32) = (32., 19.);
const KNOB: f32 = 13.;
const KNOB_TRAVEL: f32 = 13.;
const SECTIONS_MAX: f32 = 720.;
const SCROLLBAR: f32 = 10.;
/// `.theme-option` min-height and the swatch side.
const CHIP_H: f32 = 34.;
const SWATCH: f32 = 18.;
/// `.ui-segmented`: 1 px border plus 3 px padding around `--h-sm` buttons.
const SEGMENT_INSET: f32 = 4.;
pub const FOLDER_DIALOG_TITLE: &str = "Choose capture folder";
/// The accent grid's shipping accessible name (`aria-label`).
pub const ACCENT_GROUP_LABEL: &str = "Accent color";
/// `.preferences-find-step` and `.preferences-find-close` icons.
const FIND_ICON: f32 = 14.;
const FIND_PREVIOUS_PATH: &str = "m4 10 4-4 4 4";
const FIND_NEXT_PATH: &str = "m4 6 4 4 4-4";
/// `CloseIcon`, a 24-unit path.
const FIND_CLOSE_PATH: &str = "m6 6 12 12M18 6 6 18";
/// `.preferences-find-count` `min-width: 5.5rem`.
const FIND_COUNT_REMS: f32 = 5.5;
/// `.preference-find-current::after` glow (`0 0 20px`) and ring room.
const MATCH_BLEED: f32 = 22.;

/// The parts of Preferences Find (`.preferences-find`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FindPart {
    Input,
    Previous,
    Next,
    Close,
}

pub const FIND_BUTTONS: [FindPart; 3] = [FindPart::Previous, FindPart::Next, FindPart::Close];

impl FindPart {
    pub fn index(self) -> usize {
        self as usize
    }

    /// Shipping `aria-label`.
    pub fn label(self) -> &'static str {
        match self {
            Self::Input => preferences::FIND_PLACEHOLDER,
            Self::Previous => preferences::FIND_PREVIOUS,
            Self::Next => preferences::FIND_NEXT,
            Self::Close => preferences::FIND_CLOSE,
        }
    }
}

/// How many find targets an item has (shipping `PREFERENCE_FIND_SELECTOR`
/// matches each `.shortcut-row` and each `.setting-grid` cell on its own).
fn find_count(item: Item) -> usize {
    match item {
        Item::Shortcuts => SHORTCUT_ROWS.len(),
        Item::SelectGrid(keys) => keys.len(),
        _ => 1,
    }
}

/// One row of a settings card.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Item {
    /// A `.check-row.switch-row`, by index into [`Scene::rows`].
    Switch(usize),
    /// Interface theme: the System/Light/Dark segmented control.
    Mode,
    /// Accent color: the theme chip grid.
    Accent,
    /// Save captures to: the folder field and Choose….
    Folder,
    /// A `.settings-utility-row`: copy and an Open button.
    Utility(Utility),
    /// The `.shortcut-list` of recorders, in `SHORTCUT_ROWS` order.
    Shortcuts,
    /// A `.setting-row-inline` with a `CustomSelect`.
    Select(SelectKey),
    /// A `.setting-grid` (two or three 1fr columns) of stacked selects.
    SelectGrid(&'static [SelectKey]),
    /// Updates: build identity, the Check Now action and its status.
    Updates,
}

/// The utility rows' Open buttons.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Utility {
    /// Shortcuts: Windows screenshot-key settings.
    Keyboard,
    /// About: Send feedback.
    Feedback,
}

impl Utility {
    pub fn index(self) -> usize {
        self as usize
    }

    /// `(title, detail, action)`.
    pub fn copy(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::Keyboard => {
                let help = preferences::shortcut_help(PLATFORM);
                (help.system_title, help.system_body, help.system_action)
            }
            Self::Feedback => (
                preferences::FEEDBACK_TITLE,
                preferences::FEEDBACK_DETAIL,
                preferences::FEEDBACK_ACTION,
            ),
        }
    }
}

const RECORDING_GRID: &[SelectKey] = &[SelectKey::Fps, SelectKey::Resolution];
const GIF_GRID: &[SelectKey] = &[SelectKey::GifFps, SelectKey::GifWidth, SelectKey::GifColors];

/// The Recording card's `CustomSelect`s.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SelectKey {
    Format,
    Fps,
    Resolution,
    Countdown,
    Microphone,
    GifFps,
    GifWidth,
    GifColors,
}

/// Every select, in card order.
pub const SELECTS: [SelectKey; 8] = [
    SelectKey::Format,
    SelectKey::Fps,
    SelectKey::Resolution,
    SelectKey::Countdown,
    SelectKey::Microphone,
    SelectKey::GifFps,
    SelectKey::GifWidth,
    SelectKey::GifColors,
];

impl SelectKey {
    pub fn index(self) -> usize {
        SELECTS.iter().position(|k| *k == self).expect("select")
    }

    /// The persisted path, which keys the row copy.
    pub fn setting(self) -> &'static str {
        match self {
            Self::Format => "recording.video_format",
            Self::Fps => "recording.video_fps",
            Self::Resolution => "recording.video_max_resolution",
            Self::Countdown => "recording.countdown_seconds",
            Self::Microphone => "recording.microphone_device_id",
            Self::GifFps => "recording.gif_fps",
            Self::GifWidth => "recording.gif_max_width",
            Self::GifColors => "recording.gif_max_colors",
        }
    }

    /// Shipping `ariaLabel`.
    pub fn label(self) -> &'static str {
        preferences::select_accessibility_label(self.setting())
    }
}

/// Cards in shipping order: (section index, items).
pub const CARDS: [(usize, &[Item]); 8] = [
    (0, &[Item::Switch(0)]),
    (1, &[Item::Mode, Item::Accent]),
    (
        2,
        &[
            Item::Folder,
            Item::Switch(1),
            Item::Switch(2),
            Item::Switch(3),
            Item::Switch(4),
        ],
    ),
    (3, &[Item::Utility(Utility::Keyboard), Item::Shortcuts]),
    (
        4,
        &[
            Item::Select(SelectKey::Format),
            Item::SelectGrid(RECORDING_GRID),
            Item::Select(SelectKey::Countdown),
            Item::Select(SelectKey::Microphone),
            Item::Switch(5),
            Item::Switch(6),
            Item::Switch(7),
            Item::Switch(8),
            Item::Switch(9),
        ],
    ),
    (5, &[Item::SelectGrid(GIF_GRID)]),
    (6, &[Item::Updates, Item::Switch(10)]),
    (7, &[Item::Utility(Utility::Feedback)]),
];
pub const FLAGS: [&str; 4] = [
    "auto_copy_to_clipboard",
    "auto_start_on_selection",
    "freeze_screen",
    "show_cursor_in_screenshots",
];

#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }

    fn offset(self, dx: f32, dy: f32) -> Self {
        Self::new(self.x + dx, self.y + dy, self.w, self.h)
    }

    fn at_origin(self) -> Self {
        Self::new(0., 0., self.w, self.h)
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Ring {
    /// `--focus-ring-tight`: 2 px of accent at 55%.
    Tight(f32),
    /// `--focus-ring`: 2 px of canvas, then 2 px of accent at 55%.
    Full(f32),
    /// A field's focused or open state: an accent border inside, then
    /// `--focus-ring-tight` outside.
    Field(f32),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Target {
    Nav(usize),
    History,
    /// A switch row.
    Row(usize),
    /// An Interface theme segment, in `APPEARANCE_MODES` order.
    Mode(usize),
    /// An accent chip, in `THEMES` order.
    Theme(usize),
    /// Save captures to: Choose….
    Choose,
    /// A utility row's Open.
    Open(Utility),
    /// A shortcut recorder, in `SHORTCUT_ROWS` order.
    Shortcut(usize),
    /// A select trigger.
    Select(SelectKey),
    /// An option in the open listbox; not focusable (the trigger keeps focus).
    Option(SelectKey, usize),
    /// Preferences Find: the input and its buttons.
    Find(FindPart),
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    /// Positions children; no content of its own.
    Group,
    NavBg,
    /// A sidebar entry; `lit` is the hover/active layer cross-faded above it.
    NavItem {
        index: usize,
        lit: bool,
    },
    HeaderBg,
    History {
        lit: bool,
    },
    /// The scroller: canvas fill, clips its children.
    Viewport,
    Card(usize),
    /// An item's title and description.
    Copy(Item),
    TrackOff(usize),
    TrackOn,
    KnobOff,
    KnobOn,
    /// `.ui-segmented` and its sliding `.capture-segmented-indicator`.
    SegTrack,
    SegThumb,
    /// A segment label; resting and `lit` (hover/selected) layers cross-fade.
    SegLabel {
        index: usize,
        lit: bool,
    },
    /// `.theme-option:hover`.
    ChipHover(usize),
    /// A `.theme-option`, resting or `.active` (raised, bordered, checked).
    Chip {
        index: usize,
        active: bool,
    },
    /// The read-only `.directory-input input`.
    Field,
    /// `.directory-input button`, resting or hovered.
    Choose {
        lit: bool,
    },
    /// `.settings-utility-action`, resting or hovered.
    Open {
        which: Utility,
        lit: bool,
    },
    /// Updates' Check Now, disabled without an update worker.
    UpdateAction,
    /// `.update-settings-status`, right-aligned under Check Now.
    UpdateStatus,
    /// A `.shortcut-row > span`.
    ShortcutLabel(usize),
    /// `.shortcut-recorder`: field, key caps or the prompt.
    Recorder(usize),
    /// `.shortcut-recorder:hover` border.
    RecorderHover(usize),
    /// `.shortcut-error`.
    ShortcutError(usize),
    /// `.custom-select-trigger`: field and selected label.
    Trigger(SelectKey),
    /// `.custom-select-trigger:hover` border.
    TriggerHover(SelectKey),
    /// The trigger's chevron; turns over while open.
    Chevron(SelectKey),
    /// `.custom-select-listbox`: overlay panel above everything else.
    Listbox,
    /// Every option label of the open listbox, resting.
    Options,
    /// The active (hovered or arrowed) option, drawn over its resting label.
    OptionLit,
    Ring {
        shape: Ring,
        target: Target,
    },
    Scrollbar,
    /// `.preferences-find-input`: field and border.
    FindField,
    /// `.preferences-find-input:hover` border.
    FindFieldHover,
    /// The input's text, selection or placeholder, clipped to its padding.
    FindText,
    /// The input's caret; it moves and blinks on the compositor.
    Caret,
    /// `.preferences-find-count`.
    FindCount,
    /// `.preferences-find-step` or `.preferences-find-close`, resting or hovered.
    FindButton {
        part: FindPart,
        lit: bool,
    },
    /// `.preference-find-match::after` behind a find target's content; the
    /// current match adds the accent ring and glow.
    Match(usize),
}

impl Kind {
    pub fn has_content(self) -> bool {
        self != Kind::Group
    }
}

pub struct Node {
    pub parent: Option<usize>,
    pub kind: Kind,
    /// Relative to the parent's rect origin, in DIPs.
    pub rect: Rect,
    /// Paint overflow around `rect` (shadows, focus rings), whole DIPs.
    pub bleed: f32,
    pub opacity: f32,
    /// Clockwise rotation in degrees about the rect's centre.
    pub angle: f32,
    pub visible: bool,
    pub clip: bool,
    /// Children fade as one layer (switch containers).
    pub group_opacity: bool,
    /// Content must be redrawn.
    pub dirty: bool,
    /// Offset, opacity or visibility changed.
    pub props: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Prop {
    Opacity,
    OffsetX,
    OffsetY,
    Angle,
}

impl Prop {
    fn get(self, node: &Node) -> f32 {
        match self {
            Prop::Opacity => node.opacity,
            Prop::OffsetX => node.rect.x,
            Prop::OffsetY => node.rect.y,
            Prop::Angle => node.angle,
        }
    }

    fn set(self, node: &mut Node, value: f32) {
        match self {
            Prop::Opacity => node.opacity = value,
            Prop::OffsetX => node.rect.x = value,
            Prop::OffsetY => node.rect.y = value,
            Prop::Angle => node.angle = value,
        }
    }
}

/// A compositor animation for the host to start; values are DIPs or opacity.
pub struct Motion {
    pub node: usize,
    pub prop: Prop,
    pub from: f32,
    pub to: f32,
    pub start: Instant,
    pub seconds: f32,
    pub curve: CubicBezier,
    pub applied: bool,
    /// Not shown yet: the host restarts it from the frame that first shows it.
    pub fresh: bool,
}

impl Motion {
    pub fn value_at(&self, progress: f32) -> f32 {
        self.from + (self.to - self.from) * self.curve.ease(f64::from(progress)) as f32
    }

    fn progress(&self, now: Instant) -> f32 {
        (now.duration_since(self.start).as_secs_f32() / self.seconds).clamp(0., 1.)
    }

    fn value(&self, now: Instant) -> f32 {
        self.value_at(self.progress(now))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RowKey {
    Login,
    Flag(usize),
    /// A Recording card switch, by index into `RECORDING_TOGGLES`.
    Recording(usize),
    /// Updates: show what's new on update notices.
    Changelog,
}

pub struct RowState {
    pub key: RowKey,
    pub title: &'static str,
    pub detail: String,
    pub on: bool,
    pub enabled: bool,
}

/// What the host does after an activation.
pub enum Action {
    /// Change the login item off the UI thread.
    Login(bool),
    /// Appearance or accent changed: rebuild the tokens and redraw.
    Retheme,
    /// Show the folder picker, outside the host borrow.
    ChooseFolder,
    /// Open Windows keyboard settings.
    KeyboardSettings,
}

/// Accessibility notifications, raised by the host outside its borrow.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Event {
    Focus(Target),
    Toggled(usize),
    Detail(usize),
    /// A segment or accent chip became the selection.
    Selected(Target),
    /// The capture folder changed.
    Folder,
    /// A listbox opened or closed.
    Expanded(SelectKey, bool),
    /// The active option of the open listbox (`aria-activedescendant`).
    Active(SelectKey, usize),
    /// A select's or recorder's value changed.
    Value(Target),
    /// A recorder started or stopped recording.
    Recording(usize, bool),
    /// A recorder's error changed.
    ShortcutError(usize),
    /// Find opened or closed.
    FindOpen(bool),
    /// The find query changed.
    FindText,
    /// The find input's caret or selection moved.
    FindCaret,
    /// The match count label changed (`aria-live="polite"`).
    FindCount,
}

struct RowNodes {
    copy: usize,
    switch: usize,
    track_off: usize,
    track_on: usize,
    knob: usize,
    knob_off: usize,
    knob_on: usize,
    ring: usize,
}

/// A control with a resting layer, a lit layer above it and a focus ring.
#[derive(Clone, Copy)]
struct Layered {
    base: usize,
    lit: usize,
    ring: usize,
}

struct ShortcutNodes {
    label: usize,
    recorder: usize,
    hover: usize,
    ring: usize,
    error: usize,
}

struct SelectNodes {
    copy: usize,
    trigger: usize,
    hover: usize,
    chevron: usize,
    ring: usize,
}

/// The listbox overlay: panel, clip, scrolled content, resting labels and
/// the active option.
#[derive(Default)]
struct ListNodes {
    panel: usize,
    clip: usize,
    content: usize,
    options: usize,
    lit: usize,
}

/// Option labels of one select: resting and selected (medium) weights.
struct OptionTexts {
    regular: Vec<Text>,
    medium: Vec<Text>,
}

struct ChipNodes {
    group: usize,
    hover: usize,
    base: usize,
    active: usize,
    ring: usize,
}

/// The find bar's nodes.
#[derive(Default)]
struct FindNodes {
    field: usize,
    hover: usize,
    text: usize,
    caret: usize,
    ring: usize,
    count: usize,
    /// Previous, Next, Close.
    buttons: Vec<Layered>,
}

/// One shipping find target: a card header, row or grid cell.
struct FindTarget {
    /// Its `Kind::Match` node, a child of the card behind the content.
    node: usize,
    /// Text content, as shipping `textContent` (no separators).
    text: String,
    /// The target box in content coordinates (scrolled to the centre).
    bounds: Rect,
}

/// Preferences Find state (shipping `usePreferencesFind`).
pub struct Find {
    pub open: bool,
    pub field: TextField,
    /// The current match, by position in `matches`.
    index: usize,
    /// Matching targets, by index into `Scene::find_targets`.
    matches: Vec<usize>,
    /// The target drawn as current.
    current: Option<usize>,
    /// `scrolledKeyRef`: the query, index and text last scrolled to.
    scrolled: Option<(String, usize, String)>,
    /// The count label as laid out in `count`.
    count_label: String,
    count: Option<Text>,
    count_dirty: bool,
}

struct CopyText {
    item: Item,
    title: Text,
    detail: Option<Text>,
    /// Between title and detail: 6 px in switch rows, 3 px in `.setting-copy`.
    gap: f32,
}

#[derive(Default)]
struct Texts {
    brand: Option<Text>,
    nav: Vec<Text>,
    title: Option<Text>,
    subtitle: Option<Text>,
    history: Option<Text>,
    card_titles: Vec<Text>,
    card_descriptions: Vec<Text>,
    copies: Vec<CopyText>,
    segments: Vec<Text>,
    chips: Vec<Text>,
    check: Option<Text>,
    path: Option<Text>,
    choose: Option<Text>,
    /// Utility row actions, by `Utility::index`.
    open: [Option<Text>; 2],
    update_action: Option<Text>,
    update_status: Option<Text>,
    shortcut_labels: Vec<Text>,
    /// Key caps per recorder.
    caps: Vec<Vec<Text>>,
    prompt: Option<Text>,
    shortcut_errors: Vec<Option<Text>>,
    /// The selected label per select, in `SELECTS` order.
    triggers: Vec<Text>,
    options: Vec<OptionTexts>,
    option_check: Option<Text>,
}

pub struct Scene {
    pub t: Tokens,
    pub w: f32,
    pub h: f32,
    /// Device pixels per DIP; scroll offsets snap to device pixels.
    pub scale: f32,
    pub reduced_motion: bool,
    pub nodes: Vec<Node>,
    pub children: Vec<Vec<usize>>,
    pub motions: Vec<Motion>,
    pub events: Vec<Event>,
    pub rows: Vec<RowState>,
    pub active: usize,
    pub hover: Option<Target>,
    pub pressed: Option<Target>,
    pub focus: Option<Target>,
    pub focus_visible: bool,
    /// The open select and its listbox state.
    pub open: Option<(SelectKey, select::State)>,
    /// The recorder that is recording.
    pub recording: Option<usize>,
    /// Text changed size: the host must lay out again.
    pub relayout: bool,
    /// Live keys of the recording recorder.
    preview: Vec<String>,
    shortcut_errors: Vec<String>,
    /// Recorders whose caps must be laid out again (`prepare`).
    caps_dirty: Vec<bool>,
    /// `(id, name)` from the shared recording stack; `None` while loading.
    microphones: Option<Vec<(String, String)>>,
    type_ahead: (String, Option<Instant>),
    list_scroll: f32,
    settings: captures_settings::AppSettings,
    settings_file: PathBuf,
    texts: Texts,
    nav_items: Vec<Layered>,
    nav_bg: usize,
    header: usize,
    history: Layered,
    viewport: usize,
    content: usize,
    cards: Vec<usize>,
    row_nodes: Vec<RowNodes>,
    mode_copy: usize,
    seg_track: usize,
    seg_thumb: usize,
    /// Segment groups: resting label, lit label, focus ring.
    segments: Vec<usize>,
    accent_copy: usize,
    chips: Vec<ChipNodes>,
    folder_copy: usize,
    field: usize,
    choose: Layered,
    /// Utility rows' copy and Open, by `Utility::index`.
    utility_copy: [usize; 2],
    open_buttons: [Layered; 2],
    update_copy: usize,
    update_action: usize,
    update_status: usize,
    shortcuts: Vec<ShortcutNodes>,
    selects: Vec<SelectNodes>,
    list: ListNodes,
    scrollbar: usize,
    pub find: Find,
    find_nodes: FindNodes,
    /// Every find target in document order; nodes are fixed, text and
    /// boxes follow the layout.
    find_targets: Vec<FindTarget>,
    /// Shortcut-row and grid-cell boxes from the last `layout_item`.
    sub_boxes: Vec<Rect>,
    /// The window has keyboard focus; the caret shows only then.
    window_focused: bool,
    /// The caret moved or appeared: the host restarts its blink and moves
    /// the IME windows.
    pub caret_blink: bool,
    // Layout results, DIPs.
    title_y: f32,
    card_rects: Vec<Rect>,
    separators: Vec<Vec<f32>>,
    /// Whole items (including their top padding), in content coordinates.
    item_rects: Vec<(Item, Rect)>,
    /// Accent chip grid, in content coordinates.
    grid_rect: Rect,
    content_h: f32,
    content_w: f32,
    scroll: f32,
}

impl Scene {
    pub fn new(t: Tokens, settings_file: PathBuf, login_detail: String) -> Self {
        let settings = captures_settings::load(&settings_file).unwrap_or_default();
        let mut rows = vec![RowState {
            key: RowKey::Login,
            title: preferences::LOGIN_ITEM_TITLE,
            detail: login_detail,
            on: false,
            enabled: false,
        }];
        for (index, key) in FLAGS.iter().enumerate() {
            let row = preferences::row(key);
            rows.push(RowState {
                key: RowKey::Flag(index),
                title: row.title,
                detail: row.description.into(),
                on: flag_value(&settings, index),
                enabled: true,
            });
        }
        for (index, key) in RECORDING_TOGGLES.iter().enumerate() {
            let row = preferences::row(&format!("recording.{key}"));
            rows.push(RowState {
                key: RowKey::Recording(index),
                title: row.title,
                detail: row.description.into(),
                on: recording_value(&settings, index),
                enabled: true,
            });
        }
        let row = preferences::row("show_update_changelog");
        rows.push(RowState {
            key: RowKey::Changelog,
            title: row.title,
            detail: row.description.into(),
            on: settings.show_update_changelog,
            enabled: true,
        });
        let none = Layered {
            base: 0,
            lit: 0,
            ring: 0,
        };
        let mut scene = Self {
            t,
            w: 0.,
            h: 0.,
            scale: 1.,
            reduced_motion: false,
            nodes: Vec::new(),
            children: Vec::new(),
            motions: Vec::new(),
            events: Vec::new(),
            rows,
            active: 0,
            hover: None,
            pressed: None,
            focus: None,
            focus_visible: false,
            open: None,
            recording: None,
            relayout: false,
            preview: Vec::new(),
            shortcut_errors: vec![String::new(); SHORTCUT_ROWS.len()],
            caps_dirty: vec![false; SHORTCUT_ROWS.len()],
            microphones: None,
            type_ahead: (String::new(), None),
            list_scroll: 0.,
            settings,
            settings_file,
            texts: Texts::default(),
            nav_items: Vec::new(),
            nav_bg: 0,
            header: 0,
            history: none,
            viewport: 0,
            content: 0,
            cards: Vec::new(),
            row_nodes: Vec::new(),
            mode_copy: 0,
            seg_track: 0,
            seg_thumb: 0,
            segments: Vec::new(),
            accent_copy: 0,
            chips: Vec::new(),
            folder_copy: 0,
            field: 0,
            choose: none,
            utility_copy: [0; 2],
            open_buttons: [none; 2],
            update_copy: 0,
            update_action: 0,
            update_status: 0,
            shortcuts: Vec::new(),
            selects: Vec::new(),
            list: ListNodes::default(),
            scrollbar: 0,
            find: Find {
                open: false,
                field: TextField::new(Style::Input, preferences::FIND_PLACEHOLDER),
                index: 0,
                matches: Vec::new(),
                current: None,
                scrolled: None,
                count_label: String::new(),
                count: None,
                count_dirty: false,
            },
            find_nodes: FindNodes::default(),
            find_targets: Vec::new(),
            sub_boxes: Vec::new(),
            window_focused: true,
            caret_blink: false,
            title_y: 0.,
            card_rects: Vec::new(),
            separators: Vec::new(),
            item_rects: Vec::new(),
            grid_rect: Rect::default(),
            content_h: 0.,
            content_w: 0.,
            scroll: 0.,
        };
        scene.build();
        scene
    }

    fn add(&mut self, parent: Option<usize>, kind: Kind, bleed: f32) -> usize {
        let index = self.nodes.len();
        self.nodes.push(Node {
            parent,
            kind,
            rect: Rect::default(),
            bleed,
            opacity: 1.,
            angle: 0.,
            visible: true,
            clip: kind == Kind::Viewport,
            group_opacity: false,
            dirty: true,
            props: true,
        });
        self.children.push(Vec::new());
        if let Some(parent) = parent {
            self.children[parent].push(index);
        }
        index
    }

    fn ring(&mut self, parent: usize, shape: Ring, target: Target) -> usize {
        let bleed = match shape {
            Ring::Tight(_) | Ring::Field(_) => 2.,
            Ring::Full(_) => 4.,
        };
        self.add(Some(parent), Kind::Ring { shape, target }, bleed)
    }

    fn build(&mut self) {
        let root = self.add(None, Kind::Group, 0.);
        self.nav_bg = self.add(Some(root), Kind::NavBg, 0.);
        for index in 0..SECTIONS.len() {
            let base = self.add(Some(self.nav_bg), Kind::NavItem { index, lit: false }, 0.);
            let lit = self.add(Some(base), Kind::NavItem { index, lit: true }, 0.);
            let ring = self.ring(base, Ring::Tight(8.), Target::Nav(index));
            self.nav_items.push(Layered { base, lit, ring });
        }
        self.header = self.add(Some(root), Kind::HeaderBg, 0.);
        let base = self.add(Some(self.header), Kind::History { lit: false }, 0.);
        let lit = self.add(Some(base), Kind::History { lit: true }, 0.);
        let ring = self.ring(base, Ring::Full(8.), Target::History);
        self.history = Layered { base, lit, ring };
        self.build_find();
        self.viewport = self.add(Some(root), Kind::Viewport, 0.);
        self.content = self.add(Some(self.viewport), Kind::Group, 0.);
        let mut row_nodes = Vec::new();
        for (card, (_, items)) in CARDS.iter().enumerate() {
            let node = self.add(Some(self.content), Kind::Card(card), 8.);
            self.cards.push(node);
            // Highlights sit above the card and below its content.
            let targets = 1 + items.iter().map(|&item| find_count(item)).sum::<usize>();
            for _ in 0..targets {
                let index = self.find_targets.len();
                let highlight = self.add(Some(node), Kind::Match(index), MATCH_BLEED);
                self.nodes[highlight].visible = false;
                self.find_targets.push(FindTarget {
                    node: highlight,
                    text: String::new(),
                    bounds: Rect::default(),
                });
            }
            for &item in *items {
                let copy = self.add(Some(node), Kind::Copy(item), 0.);
                match item {
                    Item::Switch(row) => {
                        let switch = self.add(Some(node), Kind::Group, 0.);
                        self.nodes[switch].group_opacity = true;
                        let track_off = self.add(Some(switch), Kind::TrackOff(row), 0.);
                        let track_on = self.add(Some(switch), Kind::TrackOn, 0.);
                        let knob = self.add(Some(switch), Kind::Group, 0.);
                        let knob_off = self.add(Some(knob), Kind::KnobOff, 3.);
                        let knob_on = self.add(Some(knob), Kind::KnobOn, 0.);
                        let ring = self.ring(switch, Ring::Full(SWITCH.1 / 2.), Target::Row(row));
                        row_nodes.push((
                            row,
                            RowNodes {
                                copy,
                                switch,
                                track_off,
                                track_on,
                                knob,
                                knob_off,
                                knob_on,
                                ring,
                            },
                        ));
                    }
                    Item::Mode => {
                        self.mode_copy = copy;
                        self.seg_track = self.add(Some(node), Kind::SegTrack, 0.);
                        self.seg_thumb = self.add(Some(self.seg_track), Kind::SegThumb, 2.);
                        let radius = self.t.n("r-sm");
                        for index in 0..APPEARANCE_MODES.len() {
                            // Both label layers fade, so they are siblings.
                            let group = self.add(Some(self.seg_track), Kind::Group, 0.);
                            self.add(Some(group), Kind::SegLabel { index, lit: false }, 0.);
                            self.add(Some(group), Kind::SegLabel { index, lit: true }, 0.);
                            self.ring(group, Ring::Tight(radius), Target::Mode(index));
                            self.segments.push(group);
                        }
                    }
                    Item::Accent => {
                        self.accent_copy = copy;
                        let radius = self.t.n("r-md");
                        for index in 0..THEMES.len() {
                            let group = self.add(Some(node), Kind::Group, 0.);
                            let hover = self.add(Some(group), Kind::ChipHover(index), 0.);
                            let base = self.add(
                                Some(group),
                                Kind::Chip {
                                    index,
                                    active: false,
                                },
                                0.,
                            );
                            let active = self.add(
                                Some(group),
                                Kind::Chip {
                                    index,
                                    active: true,
                                },
                                2.,
                            );
                            let ring = self.ring(group, Ring::Full(radius), Target::Theme(index));
                            self.chips.push(ChipNodes {
                                group,
                                hover,
                                base,
                                active,
                                ring,
                            });
                        }
                    }
                    Item::Folder => {
                        self.folder_copy = copy;
                        self.field = self.add(Some(node), Kind::Field, 0.);
                        let base = self.add(Some(node), Kind::Choose { lit: false }, 2.);
                        let lit = self.add(Some(base), Kind::Choose { lit: true }, 2.);
                        let radius = self.t.n("r-md");
                        let ring = self.ring(base, Ring::Full(radius), Target::Choose);
                        self.choose = Layered { base, lit, ring };
                    }
                    Item::Utility(which) => {
                        self.utility_copy[which.index()] = copy;
                        let open = |lit| Kind::Open { which, lit };
                        let base = self.add(Some(node), open(false), 2.);
                        let lit = self.add(Some(base), open(true), 2.);
                        let radius = self.t.n("r-md");
                        let ring = self.ring(base, Ring::Full(radius), Target::Open(which));
                        self.open_buttons[which.index()] = Layered { base, lit, ring };
                    }
                    Item::Updates => {
                        self.update_copy = copy;
                        self.update_action = self.add(Some(node), Kind::UpdateAction, 2.);
                        // `.ui-btn:disabled`.
                        self.nodes[self.update_action].opacity = 0.45;
                        self.update_status = self.add(Some(node), Kind::UpdateStatus, 0.);
                    }
                    Item::Shortcuts => {
                        let radius = self.t.n("r-md");
                        for index in 0..SHORTCUT_ROWS.len() {
                            let label = self.add(Some(node), Kind::ShortcutLabel(index), 0.);
                            let recorder = self.add(Some(node), Kind::Recorder(index), 0.);
                            let hover = self.add(Some(recorder), Kind::RecorderHover(index), 0.);
                            let ring =
                                self.ring(recorder, Ring::Field(radius), Target::Shortcut(index));
                            let error = self.add(Some(node), Kind::ShortcutError(index), 0.);
                            self.shortcuts.push(ShortcutNodes {
                                label,
                                recorder,
                                hover,
                                ring,
                                error,
                            });
                        }
                    }
                    Item::Select(key) => self.add_select(node, key, copy),
                    Item::SelectGrid(keys) => {
                        for &key in keys {
                            let copy = self.add(Some(node), Kind::Copy(Item::Select(key)), 0.);
                            self.add_select(node, key, copy);
                        }
                    }
                }
            }
        }
        row_nodes.sort_by_key(|(row, _)| *row);
        self.row_nodes = row_nodes.into_iter().map(|(_, nodes)| nodes).collect();
        self.scrollbar = self.add(Some(self.viewport), Kind::Scrollbar, 0.);
        // `.custom-select-listbox` is `position: fixed` above everything.
        let panel = self.add(Some(root), Kind::Listbox, LISTBOX_BLEED);
        self.nodes[panel].visible = false;
        self.nodes[panel].group_opacity = true;
        let clip = self.add(Some(panel), Kind::Group, 0.);
        self.nodes[clip].clip = true;
        let content = self.add(Some(clip), Kind::Group, 0.);
        let options = self.add(Some(content), Kind::Options, 0.);
        let lit = self.add(Some(content), Kind::OptionLit, 0.);
        self.list = ListNodes {
            panel,
            clip,
            content,
            options,
            lit,
        };
        self.sync_switches();
        self.refresh(false);
    }

    /// The find bar in the header: input (field, hover border, text, caret,
    /// focus ring), count and the three buttons. Hidden until opened.
    fn build_find(&mut self) {
        let radius = self.t.n("r-md");
        let header = self.header;
        let field = self.add(Some(header), Kind::FindField, 0.);
        let hover = self.add(Some(field), Kind::FindFieldHover, 0.);
        let text = self.add(Some(field), Kind::FindText, 0.);
        let caret = self.add(Some(field), Kind::Caret, 0.);
        self.nodes[caret].visible = false;
        let ring = self.ring(field, Ring::Field(radius), Target::Find(FindPart::Input));
        let count = self.add(Some(header), Kind::FindCount, 0.);
        let mut buttons = Vec::new();
        for part in FIND_BUTTONS {
            let base = self.add(Some(header), Kind::FindButton { part, lit: false }, 2.);
            self.nodes[base].group_opacity = true;
            let lit = self.add(Some(base), Kind::FindButton { part, lit: true }, 0.);
            let ring = self.ring(base, Ring::Field(radius), Target::Find(part));
            buttons.push(Layered { base, lit, ring });
        }
        for node in [field, count]
            .into_iter()
            .chain(buttons.iter().map(|b| b.base))
        {
            self.nodes[node].visible = false;
        }
        self.find_nodes = FindNodes {
            field,
            hover,
            text,
            caret,
            ring,
            count,
            buttons,
        };
    }

    /// A select trigger with its hover border, chevron and focus ring; in
    /// `SELECTS` order, which is also card order.
    fn add_select(&mut self, card: usize, key: SelectKey, copy: usize) {
        debug_assert_eq!(self.selects.len(), key.index());
        let radius = self.t.n("r-md");
        let trigger = self.add(Some(card), Kind::Trigger(key), 0.);
        let hover = self.add(Some(trigger), Kind::TriggerHover(key), 0.);
        let chevron = self.add(Some(trigger), Kind::Chevron(key), 0.);
        let ring = self.ring(trigger, Ring::Field(radius), Target::Select(key));
        self.selects.push(SelectNodes {
            copy,
            trigger,
            hover,
            chevron,
            ring,
        });
    }

    /// Switch knobs, tracks and disabled opacity that follow from state.
    fn sync_switches(&mut self) {
        for (row, nodes) in self.row_nodes.iter().enumerate() {
            let state = &self.rows[row];
            let on = f32::from(state.on);
            self.nodes[nodes.track_on].opacity = on;
            self.nodes[nodes.knob_on].opacity = on;
            self.nodes[nodes.knob].rect.x = 3. + KNOB_TRAVEL * on;
            self.nodes[nodes.switch].opacity = if state.enabled { 1. } else { 0.4 };
            self.nodes[nodes.copy].opacity = if state.enabled { 1. } else { 0.55 };
        }
        for node in &mut self.nodes {
            node.props = true;
        }
    }

    /// The opacity a state layer should have, with its transition token.
    fn layer_target(&self, kind: Kind) -> Option<(f32, &'static str)> {
        let on = |b: bool| f32::from(b);
        Some(match kind {
            Kind::NavItem { index, lit: true } => (self.nav_lit(index), "dur-2"),
            Kind::History { lit: true } => (on(self.hover == Some(Target::History)), "dur-2"),
            Kind::SegLabel { index, lit } => {
                let shown = index == self.mode_index() || self.hover == Some(Target::Mode(index));
                (if lit { on(shown) } else { 1. - on(shown) }, "dur-3")
            }
            Kind::ChipHover(index) => (
                on(self.hover == Some(Target::Theme(index)) && index != self.theme_index()),
                "dur-2",
            ),
            Kind::Chip {
                index,
                active: true,
            } => (on(index == self.theme_index()), "dur-2"),
            Kind::Choose { lit: true } => (on(self.hover == Some(Target::Choose)), "dur-2"),
            Kind::Open { which, lit: true } => {
                (on(self.hover == Some(Target::Open(which))), "dur-2")
            }
            Kind::RecorderHover(index) => {
                (on(self.hover == Some(Target::Shortcut(index))), "dur-2")
            }
            Kind::TriggerHover(key) => (on(self.hover == Some(Target::Select(key))), "dur-2"),
            Kind::FindFieldHover => (
                on(self.hover == Some(Target::Find(FindPart::Input))),
                "dur-2",
            ),
            Kind::FindButton { part, lit: true } => (
                on(self.hover == Some(Target::Find(part)) && self.find_enabled(part)),
                "dur-2",
            ),
            _ => return None,
        })
    }

    /// Bring every state layer and focus ring to the current state, on the
    /// compositor when `animate`.
    fn refresh(&mut self, animate: bool) {
        for node in 0..self.nodes.len() {
            let kind = self.nodes[node].kind;
            if let Kind::Ring { target, .. } = kind {
                let shown = self.ring_shown(target);
                if self.nodes[node].visible != shown {
                    self.nodes[node].visible = shown;
                    self.nodes[node].props = true;
                }
            }
            let Some((to, duration)) = self.layer_target(kind) else {
                continue;
            };
            if animate {
                if (self.nodes[node].opacity - to).abs() > 1e-4 {
                    self.animate(node, Prop::Opacity, to, duration);
                }
            } else {
                self.nodes[node].opacity = to;
                self.nodes[node].props = true;
            }
        }
    }

    fn nav_lit(&self, index: usize) -> f32 {
        f32::from(index == self.active || self.hover == Some(Target::Nav(index)))
    }

    fn ring_shown(&self, target: Target) -> bool {
        match target {
            // An open select and a recording recorder keep the accent field.
            Target::Select(key) if self.open.is_some_and(|(open, _)| open == key) => true,
            Target::Shortcut(index) if self.recording == Some(index) => true,
            // `.preferences-find-input:focus`, not only `:focus-visible`.
            Target::Find(FindPart::Input) => self.focus == Some(target),
            _ => self.focus_visible && self.focus == Some(target),
        }
    }

    /// The selected Interface theme segment.
    pub fn mode_index(&self) -> usize {
        let id = serde_json::to_value(self.settings.appearance).unwrap_or_default();
        APPEARANCE_MODES
            .iter()
            .position(|(mode, _)| id.as_str() == Some(*mode))
            .unwrap_or(0)
    }

    /// The selected accent chip.
    pub fn theme_index(&self) -> usize {
        let id = serde_json::to_value(self.settings.theme).unwrap_or_default();
        THEMES
            .iter()
            .position(|theme| id.as_str() == Some(theme.id))
            .unwrap_or(0)
    }

    pub fn settings(&self) -> &captures_settings::AppSettings {
        &self.settings
    }

    // ---- layout ----

    /// Lay out every node for a `w`×`h` DIP client area.
    pub fn layout(&mut self, g: &Gfx, w: f32, h: f32) -> Result<()> {
        self.w = w;
        self.h = h;
        let t = &self.t;
        let n = |name: &str| t.n(name);
        let (s5, s6, s8) = (n("s-5"), n("s-6"), n("s-8"));

        // `.preferences-nav`: 16/12 padding, brand, 16 gap, the 1 px-gap list.
        self.texts.brand = Some(g.text(t, Style::Brand, "Captures", 1e4)?);
        self.texts.nav = SECTIONS
            .iter()
            .map(|section| g.text(t, Style::Nav, section.title, 1e4))
            .collect::<Result<_>>()?;
        self.nodes[self.nav_bg].rect = Rect::new(0., 0., NAV_WIDTH, h);
        let list_w = NAV_WIDTH - 2. * s5 - 1.;
        let list_y = s6 + 26. + s6;
        let item_h = n("h-md");
        for (index, item) in self.nav_items.iter().enumerate() {
            let r = Rect::new(s5, list_y + (item_h + 1.) * index as f32, list_w, item_h);
            self.nodes[item.base].rect = r;
            self.nodes[item.lit].rect = r.at_origin();
            self.nodes[item.ring].rect = r.at_origin();
        }

        // `.preferences-header`: 16/24 padding, min-height 64, 1 px rule.
        let body_w = (w - NAV_WIDTH).max(0.);
        let title = g.text(t, Style::Title, preferences::TITLE, 1e4)?;
        let subtitle = g.text(t, Style::Subtitle, preferences::SUBTITLE, 1e4)?;
        let history = g.text(t, Style::Button, preferences::HISTORY_ACTION, 1e4)?;
        let block = title.height + 2. + subtitle.height;
        let row = block.max(item_h);
        // The open find bar is a second grid row, `--s-6` below the first.
        let header_h = if self.find.open {
            s6 + row + s6 + item_h + s6 + 1.
        } else {
            (s6 + row + s6 + 1.).max(64.)
        };
        let row_y = if self.find.open {
            s6
        } else {
            (header_h - 1. - row) / 2.
        };
        self.title_y = row_y + (row - block) / 2.;
        self.nodes[self.header].rect = Rect::new(NAV_WIDTH, 0., body_w, header_h);
        let button_w = (history.width + 2. * s5 + 2.).ceil();
        let button = Rect::new(
            body_w - s8 - button_w,
            (row_y + (row - item_h) / 2.).round(),
            button_w,
            item_h,
        );
        self.nodes[self.history.base].rect = button;
        self.nodes[self.history.lit].rect = button.at_origin();
        self.nodes[self.history.ring].rect = button.at_origin();
        self.texts.title = Some(title);
        self.texts.subtitle = Some(subtitle);
        self.texts.history = Some(history);
        let bar = Rect::new(s8, s6 + row + s6, (body_w - 2. * s8).max(0.), item_h);
        self.layout_find(g, bar)?;

        // `.preferences-scroller` and `.preferences-sections`.
        let viewport = Rect::new(NAV_WIDTH, header_h, body_w, (h - header_h).max(0.));
        self.nodes[self.viewport].rect = viewport;
        self.measure_cards(g, viewport.w)?;
        if self.content_h > viewport.h {
            // A classic scrollbar takes its 10 px out of the column.
            self.measure_cards(g, viewport.w - SCROLLBAR)?;
        }
        for (card, &node) in self.cards.iter().enumerate() {
            self.nodes[node].rect = self.card_rects[card];
        }
        let scroll = self.scroll.max(0.);
        // Running motions keep going from where they are, retargeted to the
        // new geometry; the host re-issues them after this layout.
        let motions = std::mem::take(&mut self.motions);
        // Force the offsets and thumb to be recomputed for the new layout.
        self.scroll = -1.;
        self.set_scroll(scroll, false);
        self.place_list();
        for node in &mut self.nodes {
            node.dirty = true;
            node.props = true;
        }
        self.sync_switches();
        self.refresh(false);
        for mut motion in motions {
            motion.to = motion.prop.get(&self.nodes[motion.node]);
            motion.applied = false;
            self.motions.push(motion);
        }
        // Content changes (a new select label, recorder keys) re-match like
        // shipping's `contentRevision`; a just-opened bar scrolls here, in
        // its final viewport.
        self.find_refresh();
        self.sync_caret();
        Ok(())
    }

    fn measure_cards(&mut self, g: &Gfx, viewport_w: f32) -> Result<()> {
        let t = &self.t;
        let n = |name: &str| t.n(name);
        let (s1, s2, s5, s6, s8, s12) =
            (n("s-1"), n("s-2"), n("s-5"), n("s-6"), n("s-8"), n("s-12"));
        self.content_w = viewport_w.max(0.);
        let card_w = (self.content_w.min(SECTIONS_MAX) - 2. * s8).max(80.);
        let inner_w = card_w - 2. * (s6 + 1.);
        // `max-width: 52ch` of card and `.setting-copy` descriptions.
        let measure = 52. * g.ch(t, Style::CardDescription)?;
        self.card_rects.clear();
        self.separators.clear();
        self.item_rects.clear();
        self.texts.card_titles.clear();
        self.texts.card_descriptions.clear();
        self.texts.copies.clear();
        self.select_texts(g)?;
        let inset = s6 + 1.;
        let mut y = s8;
        let mut target = 0;
        for (index, items) in CARDS.iter() {
            let section = &SECTIONS[*index];
            let title = g.text(&self.t, Style::CardTitle, section.title, inner_w)?;
            let description = g.text(
                &self.t,
                Style::CardDescription,
                &card_description(*index),
                inner_w.min(measure),
            )?;
            let mut cy = inset + title.height + s2 + description.height;
            let header = Rect::new(inset, inset, inner_w, cy - inset);
            let text = format!("{}{}", section.title, card_description(*index));
            self.place_find_target(target, (s8, y), header, false, text);
            target += 1;
            let mut separators = Vec::new();
            for (position, &item) in items.iter().enumerate() {
                cy += s5;
                let top = cy;
                // `.settings-card > * + *` rules; the first item after the
                // header has only `--s-1` of padding.
                let pad = if position == 0 {
                    s1
                } else {
                    separators.push(top);
                    1. + s5
                };
                let body = top + pad;
                self.sub_boxes.clear();
                let h = self.layout_item(g, item, (inset, body, inner_w), measure)?;
                // Find targets: the row's padding box (below its rule), or
                // each shortcut row and grid cell.
                let border = if position == 0 { 0. } else { 1. };
                let padding = Rect::new(inset, top + border, inner_w, pad - border + h);
                let boxes = match item {
                    Item::Shortcuts | Item::SelectGrid(_) => std::mem::take(&mut self.sub_boxes),
                    _ => vec![padding],
                };
                let check = matches!(item, Item::Switch(_));
                for (b, text) in boxes.into_iter().zip(self.find_texts(item)) {
                    self.place_find_target(target, (s8, y), b, check, text);
                    target += 1;
                }
                let rect = Rect::new(s8 + inset, y + top, inner_w, pad + h);
                self.item_rects.push((item, rect));
                if item == Item::Accent {
                    self.grid_rect = Rect::new(
                        rect.x,
                        rect.y + rect.h - 2. * CHIP_H - s2,
                        inner_w,
                        2. * CHIP_H + s2,
                    );
                }
                cy = body + h;
            }
            let card_h = cy + inset;
            self.card_rects.push(Rect::new(s8, y, card_w, card_h));
            self.separators.push(separators);
            self.texts.card_titles.push(title);
            self.texts.card_descriptions.push(description);
            y += card_h + s6;
        }
        self.content_h = y - s6 + s12;
        Ok(())
    }

    /// Trigger labels, option labels and recorder copy for this layout.
    fn select_texts(&mut self, g: &Gfx) -> Result<()> {
        let t = &self.t;
        self.texts.triggers.clear();
        self.texts.options.clear();
        for key in SELECTS {
            let options = self.select_options(key);
            let selected = self.select_index(key);
            let label = options
                .get(selected)
                .map_or("", |(label, _)| label.as_str());
            self.texts
                .triggers
                .push(g.text(t, Style::Option, label, 1e4)?);
            let mut texts = OptionTexts {
                regular: Vec::new(),
                medium: Vec::new(),
            };
            for (label, _) in &options {
                texts.regular.push(g.text(t, Style::Option, label, 1e4)?);
                // `[aria-selected="true"]` is `--weight-medium`.
                texts.medium.push(g.text(t, Style::Segment, label, 1e4)?);
            }
            self.texts.options.push(texts);
        }
        self.texts.option_check = Some(g.text(t, Style::Option, "✓", 1e4)?);
        self.texts.prompt = Some(g.text(t, Style::Option, preferences::SHORTCUT_PROMPT, 1e4)?);
        Ok(())
    }

    /// The key caps a recorder shows: live keys while recording, else the
    /// saved shortcut.
    fn recorder_keys(&self, index: usize) -> Vec<String> {
        if self.recording == Some(index) {
            self.preview.clone()
        } else {
            shortcut_display_tokens(shortcut_ref(&self.settings, index), PLATFORM)
        }
    }

    fn cap_texts(&self, g: &Gfx, index: usize) -> Result<Vec<Text>> {
        self.recorder_keys(index)
            .iter()
            .map(|key| g.text(&self.t, Style::Kbd, key, 1e4))
            .collect()
    }

    /// Lay out recorder caps that changed since the last frame; their
    /// recorders redraw, nothing else moves.
    pub fn prepare(&mut self, g: &Gfx) -> Result<()> {
        for index in 0..SHORTCUT_ROWS.len() {
            if std::mem::take(&mut self.caps_dirty[index]) && index < self.texts.caps.len() {
                self.texts.caps[index] = self.cap_texts(g, index)?;
                self.nodes[self.shortcuts[index].recorder].dirty = true;
            }
        }
        if std::mem::take(&mut self.find.count_dirty) {
            let count = g.text(&self.t, Style::Subtitle, &self.find.count_label, 1e4)?;
            let node = self.find_nodes.count;
            // A label wider than `min-width` narrows the input.
            if count.width.ceil() > self.nodes[node].rect.w {
                self.relayout = true;
            }
            self.find.count = Some(count);
            self.nodes[node].dirty = true;
        }
        Ok(())
    }

    /// Place a select trigger and its layers at `r` (card coordinates).
    fn place_trigger(&mut self, key: SelectKey, r: Rect) {
        let (trigger, hover, chevron, ring) = {
            let nodes = &self.selects[key.index()];
            (nodes.trigger, nodes.hover, nodes.chevron, nodes.ring)
        };
        self.nodes[trigger].rect = r;
        self.nodes[hover].rect = r.at_origin();
        self.nodes[ring].rect = r.at_origin();
        let right = 1. + self.t.n("s-4");
        self.nodes[chevron].rect = Rect::new(
            r.w - right - CHEVRON,
            ((r.h - CHEVRON) / 2.).round(),
            CHEVRON,
            CHEVRON,
        );
    }

    /// Lay out one item's nodes inside its card at `(x, y)` (card coordinates)
    /// across `width`; returns its height.
    fn layout_item(
        &mut self,
        g: &Gfx,
        item: Item,
        (x, y, width): (f32, f32, f32),
        measure: f32,
    ) -> Result<f32> {
        let n = |name: &str| self.t.n(name);
        let (s2, s3, s4, s5, s6) = (n("s-2"), n("s-3"), n("s-4"), n("s-5"), n("s-6"));
        let (h_sm, h_md, h_lg) = (n("h-sm"), n("h-md"), n("h-lg"));
        match item {
            Item::Switch(row) => {
                let copy_w = (width - SWITCH.0 - s6).max(40.);
                let state = &self.rows[row];
                let title = g.text(&self.t, Style::RowTitle, state.title, copy_w)?;
                let detail = if state.detail.is_empty() {
                    None
                } else {
                    Some(g.text(&self.t, Style::RowDescription, &state.detail, copy_w)?)
                };
                // `.check-row small` adds 3 px to the span's 3 px grid gap.
                let copy_h = title.height + detail.as_ref().map_or(0., |d| 6. + d.height);
                let content_h = copy_h.max(SWITCH.1);
                self.texts.copies.push(CopyText {
                    item,
                    title,
                    detail,
                    gap: 6.,
                });
                let nodes = &self.row_nodes[row];
                let ids = (
                    nodes.copy,
                    nodes.switch,
                    nodes.track_off,
                    nodes.track_on,
                    nodes.ring,
                    nodes.knob,
                    nodes.knob_off,
                    nodes.knob_on,
                );
                let (copy, switch, track_off, track_on, ring, knob, knob_off, knob_on) = ids;
                self.nodes[copy].rect = Rect::new(x, y + (content_h - copy_h) / 2., copy_w, copy_h);
                self.nodes[switch].rect = Rect::new(
                    x + width - SWITCH.0,
                    (y + (content_h - SWITCH.1) / 2.).round(),
                    SWITCH.0,
                    SWITCH.1,
                );
                let track = Rect::new(0., 0., SWITCH.0, SWITCH.1);
                for node in [track_off, track_on, ring] {
                    self.nodes[node].rect = track;
                }
                self.nodes[knob].rect = Rect::new(3., 3., KNOB, KNOB);
                self.nodes[knob_off].rect = Rect::new(0., 0., KNOB, KNOB);
                self.nodes[knob_on].rect = Rect::new(0., 0., KNOB, KNOB);
                Ok(content_h)
            }
            Item::Mode => {
                // `.setting-row-inline`: copy, then the control at max-content.
                self.texts.segments = APPEARANCE_MODES
                    .iter()
                    .map(|(_, name)| g.text(&self.t, Style::Segment, name, 1e4))
                    .collect::<Result<_>>()?;
                let widths: Vec<f32> = self
                    .texts
                    .segments
                    .iter()
                    .map(|label| label.width + 2. * s5)
                    .collect();
                let track_w = widths.iter().sum::<f32>() + 2. * SEGMENT_INSET;
                let track_h = h_sm + 2. * SEGMENT_INSET;
                let copy_w = (width - track_w - s6).max(40.);
                let row = preferences::row("appearance");
                let title = g.text(&self.t, Style::RowTitle, row.title, copy_w)?;
                let detail = g.text(
                    &self.t,
                    Style::RowDescription,
                    row.description,
                    copy_w.min(measure),
                )?;
                let copy_h = title.height + 3. + detail.height;
                let content_h = copy_h.max(track_h);
                self.texts.copies.push(CopyText {
                    item,
                    title,
                    detail: Some(detail),
                    gap: 3.,
                });
                self.nodes[self.mode_copy].rect =
                    Rect::new(x, y + (content_h - copy_h) / 2., copy_w, copy_h);
                self.nodes[self.seg_track].rect = Rect::new(
                    x + width - track_w,
                    (y + (content_h - track_h) / 2.).round(),
                    track_w,
                    track_h,
                );
                let mut left = SEGMENT_INSET;
                for (index, &group) in self.segments.iter().enumerate() {
                    let r = Rect::new(left, SEGMENT_INSET, widths[index], h_sm);
                    self.nodes[group].rect = r;
                    for &child in &self.children[group] {
                        self.nodes[child].rect = r.at_origin();
                    }
                    left += widths[index];
                }
                let selected = self.nodes[self.segments[self.mode_index()]].rect;
                self.nodes[self.seg_thumb].rect = selected;
                Ok(content_h)
            }
            Item::Accent => {
                // `.setting-row-stack`: copy, 8 px, then the 5-column grid.
                let row = preferences::row("theme");
                let title = g.text(&self.t, Style::RowTitle, row.title, width)?;
                let detail = g.text(
                    &self.t,
                    Style::RowDescription,
                    row.description,
                    width.min(measure),
                )?;
                let copy_h = title.height + 3. + detail.height;
                self.texts.copies.push(CopyText {
                    item,
                    title,
                    detail: Some(detail),
                    gap: 3.,
                });
                self.nodes[self.accent_copy].rect = Rect::new(x, y, width, copy_h);
                self.texts.chips = THEMES
                    .iter()
                    .map(|theme| g.text(&self.t, Style::Chip, theme.name, 1e4))
                    .collect::<Result<_>>()?;
                self.texts.check = Some(g.text(&self.t, Style::Check, "✓", 1e4)?);
                let columns = preferences::THEME_COLUMNS;
                let col_w = (width - (columns - 1) as f32 * s2) / columns as f32;
                let grid_y = y + copy_h + s4;
                for (index, chip) in self.chips.iter().enumerate() {
                    let (col, line) = ((index % columns) as f32, (index / columns) as f32);
                    let r = Rect::new(
                        x + col * (col_w + s2),
                        grid_y + line * (CHIP_H + s2),
                        col_w,
                        CHIP_H,
                    );
                    self.nodes[chip.group].rect = r;
                    for node in [chip.hover, chip.base, chip.active, chip.ring] {
                        self.nodes[node].rect = r.at_origin();
                    }
                }
                let lines = THEMES.len().div_ceil(columns) as f32;
                Ok(copy_h + s4 + lines * CHIP_H + (lines - 1.) * s2)
            }
            Item::Folder => {
                let row = preferences::row("output_directory");
                let title = g.text(&self.t, Style::RowTitle, row.title, width)?;
                let copy_h = title.height;
                self.texts.copies.push(CopyText {
                    item,
                    title,
                    detail: None,
                    gap: 3.,
                });
                self.nodes[self.folder_copy].rect = Rect::new(x, y, width, copy_h);
                let choose = g.text(&self.t, Style::Control, "Choose…", 1e4)?;
                let button_w = (choose.width + 2. * s5 + 2.).ceil();
                let field_y = y + copy_h + s4;
                self.nodes[self.field].rect = Rect::new(x, field_y, width - s4 - button_w, h_lg);
                let button = Rect::new(x + width - button_w, field_y, button_w, h_lg);
                self.nodes[self.choose.base].rect = button;
                self.nodes[self.choose.lit].rect = button.at_origin();
                self.nodes[self.choose.ring].rect = button.at_origin();
                self.texts.choose = Some(choose);
                self.texts.path =
                    Some(g.text(&self.t, Style::Mono, &self.settings.output_directory, 1e4)?);
                Ok(copy_h + s4 + h_lg)
            }
            Item::Utility(which) => {
                // `.settings-utility-row`: copy, then the `auto` action.
                let (title, body, action) = which.copy();
                let open = g.text(&self.t, Style::Control, action, 1e4)?;
                let button_w = (open.width + 2. * s5 + 2.).ceil();
                let copy_w = (width - button_w - s6).max(40.);
                let title = g.text(&self.t, Style::RowTitle, title, copy_w)?;
                let detail = g.text(&self.t, Style::RowDescription, body, copy_w.min(measure))?;
                let copy_h = title.height + 3. + detail.height;
                let content_h = copy_h.max(h_md);
                self.texts.copies.push(CopyText {
                    item,
                    title,
                    detail: Some(detail),
                    gap: 3.,
                });
                self.nodes[self.utility_copy[which.index()]].rect =
                    Rect::new(x, y + (content_h - copy_h) / 2., copy_w, copy_h);
                let button = Rect::new(
                    x + width - button_w,
                    (y + (content_h - h_md) / 2.).round(),
                    button_w,
                    h_md,
                );
                let Layered { base, lit, ring } = self.open_buttons[which.index()];
                self.nodes[base].rect = button;
                self.nodes[lit].rect = button.at_origin();
                self.nodes[ring].rect = button.at_origin();
                self.texts.open[which.index()] = Some(open);
                Ok(content_h)
            }
            Item::Updates => {
                // Like the other native hosts without an update worker: build
                // identity on the left; a fixed-width Check Now with the
                // right-aligned `--text-xs` status 4 px below it.
                let control_w = preferences::UPDATES_ACTION_WIDTH.min(width / 2.);
                let copy_w = (width - control_w - s6).max(40.);
                let title = g.text(
                    &self.t,
                    Style::RowTitle,
                    preferences::UPDATES_VERSION,
                    copy_w,
                )?;
                let detail = g.text(
                    &self.t,
                    Style::RowDescription,
                    preferences::UPDATES_CHANNEL,
                    copy_w,
                )?;
                let copy_h = title.height + 3. + detail.height;
                self.texts.copies.push(CopyText {
                    item,
                    title,
                    detail: Some(detail),
                    gap: 3.,
                });
                let status =
                    g.text(&self.t, Style::Small, preferences::UPDATES_TITLE, control_w)?;
                unsafe {
                    use windows::Win32::Graphics::DirectWrite::DWRITE_TEXT_ALIGNMENT_TRAILING;
                    status
                        .layout
                        .SetTextAlignment(DWRITE_TEXT_ALIGNMENT_TRAILING)?;
                }
                let control_h = h_md + s2 + status.height;
                let content_h = copy_h.max(control_h);
                self.nodes[self.update_copy].rect =
                    Rect::new(x, y + (content_h - copy_h) / 2., copy_w, copy_h);
                let top = (y + (content_h - control_h) / 2.).round();
                let left = x + width - control_w;
                self.nodes[self.update_action].rect = Rect::new(left, top, control_w, h_md);
                self.nodes[self.update_status].rect =
                    Rect::new(left, top + h_md + s2, control_w, status.height);
                self.texts.update_action =
                    Some(g.text(&self.t, Style::Control, preferences::UPDATES_ACTION, 1e4)?);
                self.texts.update_status = Some(status);
                Ok(content_h)
            }
            Item::Shortcuts => {
                // `.shortcut-list` (4 px gaps) of `.shortcut-row`s:
                // `minmax(0, 1fr) minmax(180px, 260px)` with a 16 px gap.
                let control_w = (width - s6).clamp(RECORDER.0, RECORDER.1);
                let label_w = (width - s6 - control_w).max(1.);
                self.texts.shortcut_labels.clear();
                self.texts.shortcut_errors.clear();
                let mut caps = Vec::new();
                let mut top = y;
                for (index, (label, _)) in SHORTCUT_ROWS.iter().enumerate() {
                    let label = g.text(&self.t, Style::RowTitle, label, label_w)?;
                    let message = &self.shortcut_errors[index];
                    let error = if message.is_empty() {
                        None
                    } else {
                        Some(g.text(&self.t, Style::Small, message, control_w)?)
                    };
                    let control_h = h_md + error.as_ref().map_or(0., |e| s2 + e.height);
                    let row_h = label.height.max(control_h);
                    let nodes = &self.shortcuts[index];
                    let ids = (
                        nodes.label,
                        nodes.recorder,
                        nodes.hover,
                        nodes.ring,
                        nodes.error,
                    );
                    let (label_node, recorder, hover, ring, error_node) = ids;
                    self.nodes[label_node].rect =
                        Rect::new(x, top + (row_h - label.height) / 2., label_w, label.height);
                    let control_y = top + (row_h - control_h) / 2.;
                    let r = Rect::new(x + width - control_w, control_y.round(), control_w, h_md);
                    self.nodes[recorder].rect = r;
                    self.nodes[hover].rect = r.at_origin();
                    self.nodes[ring].rect = r.at_origin();
                    let error_h = error.as_ref().map_or(0., |e| e.height);
                    self.nodes[error_node].rect =
                        Rect::new(r.x, control_y + h_md + s2, control_w, error_h);
                    self.nodes[error_node].visible = error.is_some();
                    self.texts.shortcut_labels.push(label);
                    self.texts.shortcut_errors.push(error);
                    caps.push(self.cap_texts(g, index)?);
                    self.sub_boxes.push(Rect::new(x, top, width, row_h));
                    top += row_h + s2;
                }
                self.texts.caps = caps;
                self.caps_dirty.fill(false);
                Ok(top - s2 - y)
            }
            Item::Select(key) => {
                // `.setting-row-inline`: `minmax(160px, max-content)` control.
                let label_w = self.texts.triggers[key.index()].width;
                let trigger_w = (label_w + 2. * (s4 + 1.) + s5 + CHEVRON)
                    .ceil()
                    .max(SELECT_MIN)
                    .min((width - s6 - 40.).max(SELECT_MIN));
                let copy_w = (width - trigger_w - s6).max(40.);
                let row = preferences::row(key.setting());
                let title = g.text(&self.t, Style::RowTitle, row.title, copy_w)?;
                let detail = if row.description.is_empty() {
                    None
                } else {
                    let w = copy_w.min(measure);
                    Some(g.text(&self.t, Style::RowDescription, row.description, w)?)
                };
                let copy_h = title.height + detail.as_ref().map_or(0., |d| 3. + d.height);
                let content_h = copy_h.max(h_md);
                self.texts.copies.push(CopyText {
                    item,
                    title,
                    detail,
                    gap: 3.,
                });
                let copy = self.selects[key.index()].copy;
                self.nodes[copy].rect = Rect::new(x, y + (content_h - copy_h) / 2., copy_w, copy_h);
                let r = Rect::new(
                    x + width - trigger_w,
                    (y + (content_h - h_md) / 2.).round(),
                    trigger_w,
                    h_md,
                );
                self.place_trigger(key, r);
                Ok(content_h)
            }
            Item::SelectGrid(keys) => {
                // `.setting-grid` (`-three`): 1fr columns 12 px apart of
                // stacked rows, title then the full-width select 6 px below.
                let columns = keys.len() as f32;
                let col_w = (width - (columns - 1.) * s5) / columns;
                let mut height: f32 = 0.;
                for (col, &key) in keys.iter().enumerate() {
                    let cx = x + col as f32 * (col_w + s5);
                    let title = g.text(
                        &self.t,
                        Style::RowTitle,
                        preferences::row(key.setting()).title,
                        col_w,
                    )?;
                    let title_h = title.height;
                    self.texts.copies.push(CopyText {
                        item: Item::Select(key),
                        title,
                        detail: None,
                        gap: 3.,
                    });
                    let copy = self.selects[key.index()].copy;
                    self.nodes[copy].rect = Rect::new(cx, y, col_w, title_h);
                    let r = Rect::new(cx, (y + title_h + s3).round(), col_w, h_md);
                    self.place_trigger(key, r);
                    self.sub_boxes
                        .push(Rect::new(cx, y, col_w, title_h + s3 + h_md));
                    height = height.max(title_h + s3 + h_md);
                }
                Ok(height)
            }
        }
    }

    fn viewport_rect(&self) -> Rect {
        self.nodes[self.viewport].rect
    }

    pub fn max_scroll(&self) -> f32 {
        (self.content_h - self.viewport_rect().h).max(0.)
    }

    /// The scroll offset now, including a running smooth scroll.
    pub fn scroll_now(&self) -> f32 {
        let now = Instant::now();
        self.motions
            .iter()
            .find(|m| m.node == self.content && m.prop == Prop::OffsetY)
            .map_or(self.scroll, |m| -m.value(now))
    }

    fn scrollbar_rect(&self, scroll: f32) -> Rect {
        let viewport = self.viewport_rect();
        let max = self.max_scroll();
        let track = viewport.h - 6.;
        let h = (viewport.h * viewport.h / self.content_h.max(1.) - 6.)
            .max(18.)
            .min(track);
        let y = 3.
            + if max > 0. {
                (track - h) * scroll / max
            } else {
                0.
            };
        Rect::new(viewport.w - SCROLLBAR + 3., y, 4., h)
    }

    /// Scroll the cards, smoothly (shipping `scroll-behavior: smooth`) or at once.
    pub fn set_scroll(&mut self, value: f32, smooth: bool) {
        let target = (value.clamp(0., self.max_scroll()) * self.scale).round() / self.scale;
        if target == self.scroll {
            return;
        }
        let from = self.scroll_now();
        self.scroll = target;
        let bar = self.scrollbar_rect(target);
        self.nodes[self.scrollbar].rect = bar;
        self.nodes[self.scrollbar].visible = self.max_scroll() > 0.;
        let from_bar = self.scrollbar_rect(from).y;
        let (content, scrollbar) = (self.content, self.scrollbar);
        self.motions
            .retain(|m| !(m.prop == Prop::OffsetY && (m.node == content || m.node == scrollbar)));
        self.nodes[content].rect.y = -target;
        self.nodes[content].props = true;
        self.nodes[scrollbar].props = true;
        if smooth && !self.reduced_motion && from >= 0. {
            let start = Instant::now();
            let curve = self.curve("ease-standard");
            let seconds = self.t.n("dur-5") / 1000.;
            for (node, from, to) in [(content, -from, -target), (scrollbar, from_bar, bar.y)] {
                self.motions.push(Motion {
                    node,
                    prop: Prop::OffsetY,
                    from,
                    to,
                    start,
                    seconds,
                    curve,
                    applied: false,
                    fresh: true,
                });
            }
        } else {
            self.follow();
        }
    }

    pub fn scroll_by(&mut self, delta: f32) {
        let now = self.scroll_now();
        self.set_scroll(now + delta, false);
    }

    /// The sidebar entry follows the card under the probe line.
    pub fn follow(&mut self) {
        let scroll = self.scroll_now();
        let margin = self.t.n("s-6");
        let tops: Vec<f32> = self.card_rects.iter().map(|r| r.y).collect();
        if tops.is_empty() {
            return;
        }
        // A column that fits without scrolling is at its top, not its end.
        let at_end = self.max_scroll() > 0. && scroll >= self.max_scroll() - 1.;
        let card = preferences::visible_section(&tops, scroll + margin + 1., at_end);
        self.set_active(CARDS[card].0);
    }

    /// Drop finished motions; true while any still runs.
    pub fn tick(&mut self) -> bool {
        let now = Instant::now();
        self.follow();
        self.motions.retain(|m| m.progress(now) < 1.);
        !self.motions.is_empty()
    }

    fn curve(&self, name: &str) -> CubicBezier {
        let [x1, y1, x2, y2] = self.t.easing(name);
        CubicBezier::new(x1, y1, x2, y2).unwrap_or(CubicBezier::LINEAR)
    }

    /// Animate `prop` of `node` from where it is now to `to`.
    fn animate(&mut self, node: usize, prop: Prop, to: f32, duration: &str) {
        let now = Instant::now();
        let from = self
            .motions
            .iter()
            .find(|m| m.node == node && m.prop == prop)
            .map_or_else(|| prop.get(&self.nodes[node]), |m| m.value(now));
        self.animate_from(node, prop, from, to, duration);
    }

    /// Animate `prop` of `node` from `from` to `to` over the `duration` token.
    fn animate_from(&mut self, node: usize, prop: Prop, from: f32, to: f32, duration: &str) {
        self.animate_curve(node, prop, (from, to), duration, "ease-standard");
    }

    /// Animate along the `easing` token curve.
    fn animate_curve(
        &mut self,
        node: usize,
        prop: Prop,
        (from, to): (f32, f32),
        duration: &str,
        easing: &str,
    ) {
        self.motions.retain(|m| !(m.node == node && m.prop == prop));
        let target = &mut self.nodes[node];
        prop.set(target, to);
        target.props = true;
        if self.reduced_motion || (from - to).abs() < 1e-4 {
            return;
        }
        let seconds = self.t.n(duration) / 1000.;
        let curve = self.curve(easing);
        self.motions.push(Motion {
            node,
            prop,
            from,
            to,
            start: Instant::now(),
            seconds,
            curve,
            applied: false,
            fresh: true,
        });
    }

    // ---- state ----

    fn set_active(&mut self, index: usize) {
        if index != self.active {
            self.active = index;
            self.refresh(true);
            // Shipping `aria-current` on the section in view.
            self.events.push(Event::Selected(Target::Nav(index)));
        }
    }

    pub fn set_hover(&mut self, hover: Option<Target>) {
        if hover == self.hover {
            return;
        }
        let old = std::mem::replace(&mut self.hover, hover);
        // `onPointerEnter` makes the option active.
        if let Some(Target::Option(_, index)) = hover {
            self.set_list_active(index);
        }
        // The off track's border lightens; the only hover repaint.
        for target in [old, hover].into_iter().flatten() {
            if let Target::Row(row) = target
                && self.rows[row].enabled
                && !self.rows[row].on
            {
                self.nodes[self.row_nodes[row].track_off].dirty = true;
            }
        }
        self.refresh(true);
    }

    pub fn set_focus(&mut self, focus: Option<Target>, visible: bool) {
        if focus == self.focus && visible == self.focus_visible {
            return;
        }
        let old = self.focus;
        // Moving focus blurs a recording recorder and an open select.
        if let Some(index) = self.recording
            && focus != Some(Target::Shortcut(index))
        {
            self.stop_recording();
        }
        if let Some((key, _)) = self.open
            && focus != Some(Target::Select(key))
        {
            self.close_select();
        }
        self.focus = focus;
        self.focus_visible = visible;
        self.refresh(true);
        let input = Some(Target::Find(FindPart::Input));
        if (old == input) != (focus == input) {
            // The selection shows only while the input has focus.
            self.nodes[self.find_nodes.text].dirty = true;
        }
        self.sync_caret();
        if let Some(target) = focus
            && focus != old
        {
            self.events.push(Event::Focus(target));
        }
        if visible && let Some(target) = focus {
            self.reveal(target);
        }
    }

    /// Tab order: sidebar, header action, then card controls in order.
    pub fn focusable(&self) -> Vec<Target> {
        let mut list: Vec<Target> = (0..SECTIONS.len()).map(Target::Nav).collect();
        list.push(Target::History);
        if self.find.open {
            list.push(Target::Find(FindPart::Input));
            // Disabled step buttons are not Tab stops.
            list.extend(
                FIND_BUTTONS
                    .into_iter()
                    .filter(|&part| self.find_enabled(part))
                    .map(Target::Find),
            );
        }
        for (_, items) in CARDS {
            for item in items {
                match *item {
                    Item::Switch(row) if self.rows[row].enabled => list.push(Target::Row(row)),
                    Item::Switch(_) => {}
                    // Radio groups are one Tab stop, at the selection.
                    Item::Mode => list.push(Target::Mode(self.mode_index())),
                    Item::Accent => list.push(Target::Theme(self.theme_index())),
                    Item::Folder => list.push(Target::Choose),
                    Item::Utility(which) => list.push(Target::Open(which)),
                    // Check Now is disabled, so it is not a Tab stop.
                    Item::Updates => {}
                    Item::Shortcuts => {
                        list.extend((0..SHORTCUT_ROWS.len()).map(Target::Shortcut));
                    }
                    Item::Select(key) => list.push(Target::Select(key)),
                    Item::SelectGrid(keys) => list.extend(keys.iter().map(|&k| Target::Select(k))),
                }
            }
        }
        list
    }

    /// A control that takes focus when clicked: Tab stops, and any member of
    /// a radio group (arrows move between them).
    pub fn takes_focus(&self, target: Target) -> bool {
        matches!(target, Target::Mode(_) | Target::Theme(_)) || self.focusable().contains(&target)
    }

    /// Left/Up or Right/Down inside a radio group: the neighbouring choice,
    /// wrapping, which the host focuses and selects.
    pub fn choice_step(&self, forward: bool) -> Option<Target> {
        let step = |index: usize, count: usize| {
            if forward {
                (index + 1) % count
            } else {
                (index + count - 1) % count
            }
        };
        match self.focus? {
            Target::Mode(index) => Some(Target::Mode(step(index, APPEARANCE_MODES.len()))),
            Target::Theme(index) => Some(Target::Theme(step(index, THEMES.len()))),
            _ => None,
        }
    }

    pub fn focus_step(&mut self, forward: bool) {
        let list = self.focusable();
        // Any member of a radio group stands for the group's Tab stop.
        let group = |target: Target| match target {
            Target::Mode(_) => Target::Mode(0),
            Target::Theme(_) => Target::Theme(0),
            other => other,
        };
        let index = self
            .focus
            .and_then(|f| list.iter().position(|t| group(*t) == group(f)));
        let next = match (index, forward) {
            (None, true) => 0,
            (None, false) => list.len() - 1,
            (Some(i), true) => (i + 1) % list.len(),
            (Some(i), false) => (i + list.len() - 1) % list.len(),
        };
        self.set_focus(Some(list[next]), true);
    }

    /// Keep a focused control inside the viewport.
    fn reveal(&mut self, target: Target) {
        if matches!(target, Target::Nav(_) | Target::History | Target::Find(_)) {
            return;
        }
        let r = match target {
            Target::Row(row) => self.row_rect(row),
            _ => self.target_rect(target),
        };
        let v = self.viewport_rect();
        let margin = self.t.n("s-6");
        if r.y - margin < v.y {
            self.scroll_by(r.y - margin - v.y);
        } else if r.y + r.h + margin > v.y + v.h {
            self.scroll_by(r.y + r.h + margin - v.y - v.h);
        }
    }

    pub fn activate(&mut self, target: Target) -> Option<Action> {
        match target {
            Target::Nav(index) => {
                // Shipping scrolls the card to `scroll-margin-top: var(--s-6)`.
                if let Some(card) = CARDS.iter().position(|(section, _)| *section == index) {
                    let y = self.card_rects[card].y - self.t.n("s-6");
                    self.set_scroll(y, true);
                }
                None
            }
            // Capture History is not part of this host yet.
            Target::History => None,
            Target::Row(row) => self.toggle(row),
            Target::Mode(index) => self.select_mode(index),
            Target::Theme(index) => self.select_theme(index),
            Target::Choose => Some(Action::ChooseFolder),
            Target::Open(Utility::Keyboard) => Some(Action::KeyboardSettings),
            // The Send Feedback window is not part of this host yet.
            Target::Open(Utility::Feedback) => None,
            Target::Shortcut(index) => {
                self.start_recording(index);
                None
            }
            Target::Select(key) => {
                if self.open.is_some_and(|(open, _)| open == key) {
                    self.close_select();
                } else {
                    self.open_select(key);
                }
                None
            }
            Target::Option(key, index) => {
                self.choose(key, index);
                None
            }
            Target::Find(FindPart::Input) => None,
            Target::Find(FindPart::Close) => {
                self.find_close();
                None
            }
            Target::Find(part) => {
                self.find_step(part == FindPart::Next);
                None
            }
        }
    }

    fn save(&mut self) {
        match captures_settings::save(&self.settings_file, &self.settings) {
            Ok(saved) => self.settings = saved,
            Err(error) => eprintln!("{}", preferences::save_error(&error.to_string())),
        }
    }

    fn select_mode(&mut self, index: usize) -> Option<Action> {
        let old = self.mode_index();
        if index == old {
            return None;
        }
        let id = APPEARANCE_MODES[index].0;
        self.settings.appearance = serde_json::from_value(serde_json::json!(id)).ok()?;
        self.save();
        // The indicator takes the new segment's width and slides over
        // `--dur-4` from the old one (centred on it).
        let (from, to) = (
            self.nodes[self.segments[old]].rect,
            self.nodes[self.segments[index]].rect,
        );
        let thumb = self.seg_thumb;
        self.nodes[thumb].rect = to;
        self.nodes[thumb].dirty = true;
        let start = from.x + (from.w - to.w) / 2.;
        self.animate_from(thumb, Prop::OffsetX, start, to.x, "dur-4");
        self.refresh(true);
        self.events.push(Event::Selected(Target::Mode(index)));
        Some(Action::Retheme)
    }

    fn select_theme(&mut self, index: usize) -> Option<Action> {
        if index == self.theme_index() {
            return None;
        }
        self.settings.theme = serde_json::from_value(serde_json::json!(THEMES[index].id)).ok()?;
        self.save();
        self.refresh(true);
        self.events.push(Event::Selected(Target::Theme(index)));
        Some(Action::Retheme)
    }

    /// Save a folder chosen in the picker; true when the layout must change.
    pub fn set_output_directory(&mut self, path: String) -> bool {
        if path == self.settings.output_directory {
            return false;
        }
        self.settings.output_directory = path;
        self.save();
        self.events.push(Event::Folder);
        true
    }

    fn toggle(&mut self, row: usize) -> Option<Action> {
        if !self.rows[row].enabled {
            return None;
        }
        let on = !self.rows[row].on;
        match self.rows[row].key {
            RowKey::Login => {
                self.set_row(row, on);
                self.set_enabled(row, false);
                Some(Action::Login(on))
            }
            RowKey::Flag(flag) => {
                self.set_row(row, on);
                set_flag(&mut self.settings, flag, on);
                self.save();
                None
            }
            RowKey::Recording(toggle) => {
                self.set_row(row, on);
                set_recording(&mut self.settings, toggle, on);
                self.save();
                None
            }
            RowKey::Changelog => {
                self.set_row(row, on);
                self.settings.show_update_changelog = on;
                self.save();
                None
            }
        }
    }

    /// Move a switch, animating the knob over `--dur-3` and the track over `--dur-2`.
    pub fn set_row(&mut self, row: usize, on: bool) {
        if self.rows[row].on == on {
            return;
        }
        self.rows[row].on = on;
        let nodes = &self.row_nodes[row];
        let (track_on, knob, knob_on, track_off) =
            (nodes.track_on, nodes.knob, nodes.knob_on, nodes.track_off);
        let value = f32::from(on);
        self.animate(track_on, Prop::Opacity, value, "dur-2");
        self.animate(knob_on, Prop::Opacity, value, "dur-3");
        self.animate(knob, Prop::OffsetX, 3. + KNOB_TRAVEL * value, "dur-3");
        self.nodes[track_off].dirty = true;
        self.events.push(Event::Toggled(row));
    }

    pub fn set_enabled(&mut self, row: usize, enabled: bool) {
        if self.rows[row].enabled == enabled {
            return;
        }
        self.rows[row].enabled = enabled;
        let nodes = &self.row_nodes[row];
        let (switch, copy, track_off) = (nodes.switch, nodes.copy, nodes.track_off);
        self.nodes[switch].opacity = if enabled { 1. } else { 0.4 };
        self.nodes[copy].opacity = if enabled { 1. } else { 0.55 };
        self.nodes[switch].props = true;
        self.nodes[copy].props = true;
        self.nodes[track_off].dirty = true;
        if !enabled && self.focus == Some(Target::Row(row)) {
            self.set_focus(None, false);
        }
    }

    /// Replace a row's description; true when the layout must be rebuilt.
    pub fn set_detail(&mut self, row: usize, detail: String) -> bool {
        if self.rows[row].detail == detail {
            return false;
        }
        self.rows[row].detail = detail;
        self.events.push(Event::Detail(row));
        true
    }

    // ---- selects ----

    /// `(label, disabled)` per option, in shipping order.
    pub fn select_options(&self, key: SelectKey) -> Vec<(String, bool)> {
        let plain = |labels: Vec<String>| labels.into_iter().map(|l| (l, false)).collect();
        match key {
            SelectKey::Format => {
                plain(RECORDING_FORMATS.iter().map(|(_, l)| (*l).into()).collect())
            }
            SelectKey::Fps => plain(
                RECORDING_FPS
                    .iter()
                    .map(|f| preferences::fps_label(*f))
                    .collect(),
            ),
            SelectKey::Resolution => plain(RESOLUTIONS.iter().map(|(_, l)| (*l).into()).collect()),
            SelectKey::Countdown => plain(
                COUNTDOWN_SECONDS
                    .map(preferences::countdown_label)
                    .collect(),
            ),
            SelectKey::Microphone => self
                .microphone_entries()
                .into_iter()
                .map(|entry| (entry.label, !entry.enabled))
                .collect(),
            SelectKey::GifFps => {
                plain(GIF_FPS.iter().map(|f| preferences::fps_label(*f)).collect())
            }
            SelectKey::GifWidth => plain(
                GIF_MAX_WIDTHS
                    .iter()
                    .map(|w| preferences::width_label(*w))
                    .collect(),
            ),
            SelectKey::GifColors => plain(GIF_PALETTE_COLORS.iter().map(u16::to_string).collect()),
        }
    }

    /// Shared microphone options: Off, a loading row while devices
    /// enumerate, a saved device that is missing, then the devices.
    fn microphone_entries(&self) -> Vec<captures_app::capture_menu::MicrophoneEntry> {
        let devices: Vec<(&str, &str)> = self
            .microphones
            .iter()
            .flatten()
            .map(|(id, name)| (id.as_str(), name.as_str()))
            .collect();
        captures_app::capture_menu::microphone_entries(
            true,
            self.microphones.is_none(),
            self.settings.recording.microphone_device_id.as_deref(),
            &devices,
        )
    }

    /// The selected option.
    pub fn select_index(&self, key: SelectKey) -> usize {
        let recording = &self.settings.recording;
        let id = |value: serde_json::Value, ids: &[&str]| {
            ids.iter()
                .position(|id| value.as_str() == Some(*id))
                .unwrap_or(0)
        };
        match key {
            SelectKey::Format => id(
                serde_json::to_value(recording.video_format).unwrap_or_default(),
                &RECORDING_FORMATS.map(|(id, _)| id),
            ),
            SelectKey::Fps => RECORDING_FPS
                .iter()
                .position(|fps| *fps == recording.video_fps)
                .unwrap_or(0),
            SelectKey::Resolution => id(
                serde_json::to_value(recording.video_max_resolution).unwrap_or_default(),
                &RESOLUTIONS.map(|(id, _)| id),
            ),
            SelectKey::Countdown => COUNTDOWN_SECONDS
                .clone()
                .position(|seconds| seconds == recording.countdown_seconds)
                .unwrap_or(0),
            SelectKey::Microphone => {
                let selected = recording.microphone_device_id.as_deref();
                self.microphone_entries()
                    .iter()
                    .position(|entry| entry.enabled && entry.id.as_deref() == selected)
                    .unwrap_or(0)
            }
            // Like `CustomSelect`, an unlisted saved value shows the first.
            SelectKey::GifFps => GIF_FPS
                .iter()
                .position(|fps| *fps == recording.gif_fps)
                .unwrap_or(0),
            SelectKey::GifWidth => GIF_MAX_WIDTHS
                .iter()
                .position(|w| u32::from(*w) == recording.gif_max_width)
                .unwrap_or(0),
            SelectKey::GifColors => GIF_PALETTE_COLORS
                .iter()
                .position(|c| *c == recording.gif_max_colors)
                .unwrap_or(0),
        }
    }

    /// The selected label (the trigger's text and the UIA value).
    pub fn select_value(&self, key: SelectKey) -> String {
        self.select_options(key)
            .into_iter()
            .nth(self.select_index(key))
            .map_or_else(String::new, |(label, _)| label)
    }

    /// Microphones from the shared recording stack arrived.
    pub fn set_microphones(&mut self, devices: Vec<(String, String)>) {
        self.microphones = Some(devices);
        self.relayout = true;
        self.events
            .push(Event::Value(Target::Select(SelectKey::Microphone)));
    }

    fn choose(&mut self, key: SelectKey, index: usize) {
        let options = self.select_options(key);
        if options.get(index).is_none_or(|(_, disabled)| *disabled) {
            return;
        }
        self.close_select();
        if index == self.select_index(key) {
            return;
        }
        let recording = &mut self.settings.recording;
        match key {
            SelectKey::Format => {
                if let Ok(format) =
                    serde_json::from_value(serde_json::json!(RECORDING_FORMATS[index].0))
                {
                    recording.video_format = format;
                }
            }
            SelectKey::Fps => recording.video_fps = RECORDING_FPS[index],
            SelectKey::Resolution => {
                if let Ok(resolution) =
                    serde_json::from_value(serde_json::json!(RESOLUTIONS[index].0))
                {
                    recording.video_max_resolution = resolution;
                }
            }
            SelectKey::Countdown => {
                recording.countdown_seconds = COUNTDOWN_SECONDS.clone().nth(index).unwrap_or(0);
            }
            SelectKey::Microphone => {
                let entry = self.microphone_entries().into_iter().nth(index);
                self.settings.recording.microphone_device_id = entry.and_then(|e| e.id);
            }
            SelectKey::GifFps => recording.gif_fps = GIF_FPS[index],
            SelectKey::GifWidth => recording.gif_max_width = u32::from(GIF_MAX_WIDTHS[index]),
            SelectKey::GifColors => recording.gif_max_colors = GIF_PALETTE_COLORS[index],
        }
        self.save();
        // The trigger is `max-content` wide, so its label can move the row.
        self.relayout = true;
        self.events.push(Event::Value(Target::Select(key)));
    }

    /// Open a select's listbox at its selection (`openMenu`).
    pub fn open_select(&mut self, key: SelectKey) {
        if self.open.is_some_and(|(open, _)| open == key) {
            return;
        }
        self.close_select();
        self.stop_recording();
        let disabled: Vec<bool> = self.select_options(key).iter().map(|(_, d)| *d).collect();
        let state = select::open(&disabled, self.select_index(key));
        self.open = Some((key, state));
        self.list_scroll = 0.;
        self.place_list();
        self.reveal_option();
        let panel = self.list.panel;
        self.nodes[panel].visible = true;
        // `ui-pop-in`: fade and drop 4 px over `--dur-2` `--ease-out`.
        let y = self.nodes[panel].rect.y;
        self.animate_curve(panel, Prop::Opacity, (0., 1.), "dur-2", "ease-out");
        self.animate_curve(panel, Prop::OffsetY, (y - 4., y), "dur-2", "ease-out");
        let chevron = self.selects[key.index()].chevron;
        self.animate(chevron, Prop::Angle, 180., "dur-2");
        self.refresh(true);
        self.events.push(Event::Expanded(key, true));
        self.events.push(Event::Active(key, state.active));
    }

    /// Close the open listbox; it unmounts at once, like shipping.
    pub fn close_select(&mut self) {
        let Some((key, _)) = self.open.take() else {
            return;
        };
        let panel = self.list.panel;
        self.motions.retain(|m| m.node != panel);
        self.nodes[panel].visible = false;
        self.nodes[panel].opacity = 1.;
        self.nodes[panel].props = true;
        let chevron = self.selects[key.index()].chevron;
        self.animate(chevron, Prop::Angle, 0., "dur-2");
        self.refresh(true);
        self.events.push(Event::Expanded(key, false));
    }

    fn option_h(&self) -> f32 {
        // `min-height: --h-sm`, `--s-3` padding, 1 px `padding-block` copy.
        let text = self.texts.options.first().and_then(|o| o.regular.first());
        let n = |name: &str| self.t.n(name);
        text.map_or(n("h-sm"), |t| {
            (t.height + 2. + 2. * n("s-3")).max(n("h-sm"))
        })
    }

    /// `(content height, visible height)` of the open listbox's options.
    fn list_heights(&self) -> (f32, f32) {
        let panel = self.nodes[self.list.panel].rect;
        (self.nodes[self.list.options].rect.h, (panel.h - 2.).max(0.))
    }

    /// Position the open listbox with shipping `placeCustomSelectMenu`.
    pub fn place_list(&mut self) {
        let Some((key, mut state)) = self.open else {
            return;
        };
        let Some(texts) = self.texts.options.get(key.index()) else {
            return;
        };
        let n = |name: &str| self.t.n(name);
        let (s2, s4, s6) = (n("s-2"), n("s-4"), n("s-6"));
        let count = texts.regular.len();
        state.active = state.active.min(count.saturating_sub(1));
        self.open = Some((key, state));
        let option_h = self.option_h();
        let label_w = texts.medium.iter().map(|t| t.width).fold(0., f32::max);
        let check_w = self.texts.option_check.as_ref().map_or(0., |t| t.width);
        let content_w = (label_w + s6 + check_w + 2. * s4 + 2. * s2 + 2.).ceil();
        let content_h = count as f32 * option_h + 2. * s2;
        let trigger = self.absolute(self.selects[key.index()].trigger);
        let layout = select::layout(
            select::Rect {
                left: f64::from(trigger.x),
                top: f64::from(trigger.y),
                width: f64::from(trigger.w),
                height: f64::from(trigger.h),
            },
            f64::from(content_w),
            f64::from(content_h + 2.),
            f64::from(self.w),
            f64::from(self.h),
            count,
        );
        let h = (layout.max_height as f32).min(content_h + 2.);
        let w = layout.width as f32;
        let panel = Rect::new(layout.left as f32, layout.top as f32, w, h);
        let ListNodes {
            panel: panel_node,
            clip,
            content,
            options,
            lit,
        } = self.list;
        self.nodes[panel_node].rect = panel;
        self.nodes[clip].rect = Rect::new(1., 1., w - 2., h - 2.);
        self.list_scroll = self.list_scroll.clamp(0., (content_h - (h - 2.)).max(0.));
        self.nodes[content].rect = Rect::new(0., -self.list_scroll, w - 2., content_h);
        self.nodes[options].rect = Rect::new(0., 0., w - 2., content_h);
        self.nodes[lit].rect = Rect::new(
            s2,
            s2 + state.active as f32 * option_h,
            w - 2. - 2. * s2,
            option_h,
        );
        for node in [panel_node, clip, content, options, lit] {
            self.nodes[node].dirty = true;
            self.nodes[node].props = true;
        }
    }

    /// Keep the active option in view (`scrollIntoView({ block: "nearest" })`).
    fn reveal_option(&mut self) {
        let Some((_, state)) = self.open else {
            return;
        };
        let option_h = self.option_h();
        let s2 = self.t.n("s-2");
        let (content_h, view_h) = self.list_heights();
        let top = s2 + state.active as f32 * option_h;
        let mut scroll = self.list_scroll;
        if top < scroll {
            scroll = top;
        } else if top + option_h > scroll + view_h {
            scroll = top + option_h - view_h;
        }
        self.set_list_scroll(scroll.clamp(0., (content_h - view_h).max(0.)));
    }

    fn set_list_scroll(&mut self, scroll: f32) {
        let (content_h, view_h) = self.list_heights();
        let scroll =
            (scroll.clamp(0., (content_h - view_h).max(0.)) * self.scale).round() / self.scale;
        if scroll != self.list_scroll {
            self.list_scroll = scroll;
            // A compositor offset; the options do not redraw.
            self.nodes[self.list.content].rect.y = -scroll;
            self.nodes[self.list.content].props = true;
        }
    }

    /// Scroll the open listbox by `delta` DIPs.
    pub fn scroll_list(&mut self, delta: f32) {
        self.set_list_scroll(self.list_scroll + delta);
    }

    fn set_list_active(&mut self, active: usize) {
        let Some((key, state)) = self.open else {
            return;
        };
        if state.active == active {
            return;
        }
        self.open = Some((key, select::State { active, ..state }));
        let lit = self.list.lit;
        let y = self.t.n("s-2") + active as f32 * self.option_h();
        self.nodes[lit].rect.y = y;
        self.nodes[lit].props = true;
        // Only the 30 px active row repaints.
        self.nodes[lit].dirty = true;
        self.reveal_option();
        self.events.push(Event::Active(key, active));
    }

    /// The listbox panel contains `(x, y)` (client DIPs).
    pub fn list_contains(&self, x: f32, y: f32) -> bool {
        self.open.is_some() && self.nodes[self.list.panel].rect.contains(x, y)
    }

    /// A key for the focused select (shipping trigger `onKeyDown`); true
    /// when it was consumed.
    pub fn select_key(&mut self, key: select::Key) -> bool {
        let Some(Target::Select(select_key)) = self.focus else {
            return false;
        };
        let state = self
            .open
            .filter(|(open, _)| *open == select_key)
            .map_or_else(select::State::default, |(_, state)| state);
        let disabled: Vec<bool> = self
            .select_options(select_key)
            .iter()
            .map(|(_, d)| *d)
            .collect();
        let outcome = select::key(state, &disabled, self.select_index(select_key), key);
        if !outcome.handled {
            return false;
        }
        if !self.focus_visible {
            self.set_focus(self.focus, true);
        }
        if let Some(index) = outcome.chosen {
            self.choose(select_key, index);
        } else if outcome.state.open && !state.open {
            self.open_select(select_key);
        } else if !outcome.state.open && state.open {
            self.close_select();
        } else if outcome.state.open {
            self.set_list_active(outcome.state.active);
        }
        true
    }

    /// Type-ahead in an open listbox: the next enabled option whose label
    /// starts with the keys typed within the last second.
    pub fn type_ahead(&mut self, ch: char) -> bool {
        let Some((key, state)) = self.open else {
            return false;
        };
        let now = Instant::now();
        let (prefix, last) = &mut self.type_ahead;
        if last.is_none_or(|t| now.duration_since(t).as_secs_f32() > TYPE_AHEAD_SECONDS) {
            prefix.clear();
        }
        if prefix.is_empty() && ch == ' ' {
            return false;
        }
        prefix.extend(ch.to_lowercase());
        *last = Some(now);
        let prefix = prefix.clone();
        let options = self.select_options(key);
        let count = options.len();
        // A new search starts after the active option; a longer prefix may
        // still match it.
        let start = if prefix.chars().count() == 1 {
            state.active + 1
        } else {
            state.active
        };
        let found = (0..count).map(|k| (start + k) % count).find(|&index| {
            let (label, disabled) = &options[index];
            !disabled && label.to_lowercase().starts_with(&prefix)
        });
        if let Some(index) = found {
            self.set_list_active(index);
        }
        true
    }

    // ---- shortcut recorders ----

    /// Start recording a shortcut (the recorder's click).
    pub fn start_recording(&mut self, index: usize) {
        self.close_select();
        if self.recording == Some(index) {
            return;
        }
        self.stop_recording();
        self.recording = Some(index);
        self.preview.clear();
        self.set_shortcut_error(index, String::new());
        self.caps_dirty[index] = true;
        self.refresh(true);
        self.events.push(Event::Recording(index, true));
    }

    /// Stop recording without a change (Esc, blur).
    pub fn stop_recording(&mut self) {
        let Some(index) = self.recording.take() else {
            return;
        };
        self.preview.clear();
        self.set_shortcut_error(index, String::new());
        self.caps_dirty[index] = true;
        self.refresh(true);
        self.events.push(Event::Recording(index, false));
    }

    /// A key chord while recording, through the shared recorder policy.
    pub fn record_key(&mut self, event: &ShortcutKeyEvent) {
        let Some(index) = self.recording else {
            return;
        };
        match record_shortcut(event, PLATFORM) {
            ShortcutRecording::Cancel => self.stop_recording(),
            ShortcutRecording::Waiting { keys } => {
                self.preview = keys;
                self.caps_dirty[index] = true;
                self.set_shortcut_error(index, String::new());
            }
            ShortcutRecording::Invalid { keys, message } => {
                self.preview = keys;
                self.caps_dirty[index] = true;
                self.set_shortcut_error(index, message);
            }
            ShortcutRecording::Complete { shortcut, .. } => {
                self.stop_recording();
                if let Err(message) = self.set_shortcut(index, shortcut) {
                    self.set_shortcut_error(index, message);
                }
            }
        }
    }

    /// Restore a recorder's default shortcut from `captures-settings`.
    pub fn reset_shortcut(&mut self, index: usize) {
        self.stop_recording();
        let result = self.set_shortcut(index, shortcut_default(index));
        self.set_shortcut_error(index, result.err().unwrap_or_default());
    }

    /// Save a recorded shortcut; settings validation rejects empty and
    /// duplicate shortcuts, which keeps the old one and reports why.
    fn set_shortcut(&mut self, index: usize, value: String) -> std::result::Result<(), String> {
        if shortcut_ref(&self.settings, index) == value {
            return Ok(());
        }
        let mut next = self.settings.clone();
        *shortcut_mut(&mut next, index) = value;
        let saved = captures_settings::save(&self.settings_file, &next)
            .map_err(|error| preferences::save_error(&error.to_string()))?;
        self.settings = saved;
        self.caps_dirty[index] = true;
        self.events.push(Event::Value(Target::Shortcut(index)));
        Ok(())
    }

    fn set_shortcut_error(&mut self, index: usize, message: String) {
        if self.shortcut_errors[index] != message {
            self.shortcut_errors[index] = message;
            self.relayout = true;
            self.events.push(Event::ShortcutError(index));
        }
    }

    /// The recorder's error (`.shortcut-error`, `role="status"`).
    pub fn shortcut_error(&self, index: usize) -> &str {
        &self.shortcut_errors[index]
    }

    /// The recorder's keys for accessibility, like the AppKit host.
    pub fn shortcut_value(&self, index: usize) -> String {
        let keys = self.recorder_keys(index);
        if keys.is_empty() {
            if self.recording == Some(index) {
                "Press shortcut".into()
            } else {
                "None".into()
            }
        } else {
            keys.join(" + ")
        }
    }

    /// The window lost focus: like a DOM blur, recording stops and the
    /// listbox closes.
    pub fn blur(&mut self) {
        self.stop_recording();
        self.close_select();
        self.window_focus(false);
    }

    // ---- find ----

    /// The find query as typed, including an open IME composition (React's
    /// `onChange` sees composing text too).
    pub fn find_query(&self) -> String {
        String::from_utf16_lossy(&self.find.field.editor.display())
    }

    /// Shipping `preferenceFindCountLabel`.
    pub fn find_count_label(&self) -> String {
        preferences::find_count_label(&self.find_query(), self.find.matches.len(), self.find.index)
    }

    /// Previous and Next are `disabled` without matches.
    pub fn find_enabled(&self, part: FindPart) -> bool {
        match part {
            FindPart::Previous | FindPart::Next => !self.find.matches.is_empty(),
            FindPart::Input | FindPart::Close => true,
        }
    }

    /// The find input has keyboard focus.
    pub fn text_focused(&self) -> bool {
        self.find.open && self.focus == Some(Target::Find(FindPart::Input))
    }

    /// Text content of an item's find targets, like shipping `textContent`:
    /// copy and control text run together without separators.
    fn find_texts(&self, item: Item) -> Vec<String> {
        let row = preferences::row;
        match item {
            Item::Switch(r) => vec![format!("{}{}", self.rows[r].title, self.rows[r].detail)],
            Item::Mode => {
                let r = row("appearance");
                let modes: String = APPEARANCE_MODES.iter().map(|(_, name)| *name).collect();
                vec![format!("{}{}{modes}", r.title, r.description)]
            }
            Item::Accent => {
                let r = row("theme");
                let names: String = THEMES.iter().map(|theme| theme.name).collect();
                vec![format!("{}{}{names}", r.title, r.description)]
            }
            Item::Folder => vec![format!("{}Choose…", row("output_directory").title)],
            Item::Utility(which) => {
                let (title, body, action) = which.copy();
                vec![format!("{title}{body}{action}")]
            }
            Item::Updates => vec![format!(
                "{}{}{}{}",
                preferences::UPDATES_VERSION,
                preferences::UPDATES_CHANNEL,
                preferences::UPDATES_ACTION,
                preferences::UPDATES_TITLE
            )],
            Item::Shortcuts => SHORTCUT_ROWS
                .iter()
                .enumerate()
                .map(|(index, (label, _))| {
                    let keys = self.recorder_keys(index);
                    let shown = if keys.is_empty() {
                        preferences::SHORTCUT_PROMPT.to_owned()
                    } else {
                        keys.concat()
                    };
                    format!("{label}{shown}")
                })
                .collect(),
            Item::Select(key) => {
                let r = row(key.setting());
                vec![format!(
                    "{}{}{}",
                    r.title,
                    r.description,
                    self.select_value(key)
                )]
            }
            Item::SelectGrid(keys) => keys
                .iter()
                .map(|&key| format!("{}{}", row(key.setting()).title, self.select_value(key)))
                .collect(),
        }
    }

    /// Place target `index`'s highlight around `b` (its padding box in card
    /// coordinates, card origin `(ox, oy)` in the content): `inset: -s-3`,
    /// or the check-row variant below the row's rule.
    fn place_find_target(
        &mut self,
        index: usize,
        (ox, oy): (f32, f32),
        b: Rect,
        check: bool,
        text: String,
    ) {
        let n = |name: &str| self.t.n(name);
        let (s3, s4, s5) = (n("s-3"), n("s-4"), n("s-5"));
        let r = if check {
            let top = s5 - s4;
            Rect::new(b.x - s4, b.y + top, b.w + 2. * s4, b.h - top + s4)
        } else {
            Rect::new(b.x - s3, b.y - s3, b.w + 2. * s3, b.h + 2. * s3)
        };
        let target = &mut self.find_targets[index];
        target.text = text;
        target.bounds = b.offset(ox, oy);
        let node = target.node;
        self.nodes[node].rect = r;
    }

    /// Lay out the find bar across `bar` (header coordinates): the input
    /// takes what the count and three square buttons leave, 6 px apart.
    fn layout_find(&mut self, g: &Gfx, bar: Rect) -> Result<()> {
        let (s3, s4) = (self.t.n("s-3"), self.t.n("s-4"));
        let h = bar.h;
        self.find.count_label = self.find_count_label();
        let count = g.text(&self.t, Style::Subtitle, &self.find.count_label, 1e4)?;
        let count_w = (FIND_COUNT_REMS * self.t.n("text-md")).max(count.width.ceil());
        let mut right = bar.x + bar.w;
        for index in (0..FIND_BUTTONS.len()).rev() {
            right -= h;
            let r = Rect::new(right, bar.y, h, h);
            let Layered { base, lit, ring } = self.find_nodes.buttons[index];
            self.nodes[base].rect = r;
            self.nodes[lit].rect = r.at_origin();
            self.nodes[ring].rect = r.at_origin();
            right -= s3;
        }
        let FindNodes {
            field,
            hover,
            text,
            ring,
            count: count_node,
            ..
        } = self.find_nodes;
        self.nodes[count_node].rect = Rect::new(right - count_w, bar.y, count_w, h);
        right -= count_w + s3;
        let r = Rect::new(bar.x, bar.y, (right - bar.x).max(0.), h);
        self.nodes[field].rect = r;
        self.nodes[hover].rect = r.at_origin();
        self.nodes[ring].rect = r.at_origin();
        // `padding: 0 var(--s-4)` inside the 1 px border.
        let inset = 1. + s4;
        let content = Rect::new(inset, 0., (r.w - 2. * inset).max(1.), h);
        self.nodes[text].rect = content;
        self.find.field.relayout(g, &self.t, content.w)?;
        self.find.count = Some(count);
        self.find.count_dirty = false;
        let open = self.find.open;
        for node in [field, count_node]
            .into_iter()
            .chain(self.find_nodes.buttons.iter().map(|b| b.base))
        {
            self.nodes[node].visible = open;
        }
        self.sync_find_buttons();
        Ok(())
    }

    /// `.preferences-find-step:disabled` fades to 0.45.
    fn sync_find_buttons(&mut self) {
        for (index, part) in FIND_BUTTONS.into_iter().enumerate() {
            let base = self.find_nodes.buttons[index].base;
            let opacity = if self.find_enabled(part) { 1. } else { 0.45 };
            if self.nodes[base].opacity != opacity {
                self.nodes[base].opacity = opacity;
                self.nodes[base].props = true;
            }
        }
    }

    /// Recompute matches, highlights, the count and the scroll position
    /// (shipping's layout effect): the current match scrolls to the centre
    /// once per query, index and text.
    fn find_refresh(&mut self) {
        let query = self.find_query();
        let matches: Vec<usize> = if self.find.open {
            (0..self.find_targets.len())
                .filter(|&i| preferences::find_matches(&self.find_targets[i].text, &query))
                .collect()
        } else {
            Vec::new()
        };
        let had = !self.find.matches.is_empty();
        self.find.matches = matches;
        self.find.index = self
            .find
            .index
            .min(self.find.matches.len().saturating_sub(1));
        for i in 0..self.find_targets.len() {
            let node = self.find_targets[i].node;
            let shown = self.find.matches.contains(&i);
            if self.nodes[node].visible != shown {
                self.nodes[node].visible = shown;
                self.nodes[node].props = true;
            }
        }
        let current = self.find.matches.get(self.find.index).copied();
        if current != self.find.current {
            for target in [self.find.current, current].into_iter().flatten() {
                let node = self.find_targets[target].node;
                self.nodes[node].dirty = true;
            }
            self.find.current = current;
        }
        let label = self.find_count_label();
        if label != self.find.count_label {
            self.find.count_label = label;
            self.find.count_dirty = true;
            self.events.push(Event::FindCount);
        }
        if had == self.find.matches.is_empty() {
            self.sync_find_buttons();
            // A step button that becomes disabled loses focus.
            if matches!(
                self.focus,
                Some(Target::Find(FindPart::Previous | FindPart::Next))
            ) && self.find.matches.is_empty()
            {
                self.set_focus(None, false);
            }
            self.refresh(true);
        }
        let Some(current) = current else {
            return;
        };
        let target = &self.find_targets[current];
        let key = (query, self.find.index, target.text.clone());
        if self.find.scrolled.as_ref() == Some(&key) {
            return;
        }
        let bounds = target.bounds;
        self.find.scrolled = Some(key);
        // `scrollIntoView({ behavior: "smooth", block: "center" })`.
        let view = self.viewport_rect().h;
        self.set_scroll(bounds.y + bounds.h / 2. - view / 2., true);
    }

    /// Ctrl+F (`revealFind`): open the bar, focus the input, select its text.
    pub fn find_reveal(&mut self, g: &Gfx) {
        if !self.find.open {
            self.find.open = true;
            self.find.scrolled = None;
            // The header grows a row; matches and scrolling follow the layout.
            self.relayout = true;
            self.events.push(Event::FindOpen(true));
        }
        self.set_focus(Some(Target::Find(FindPart::Input)), false);
        self.field_edit(g, Editor::select_all);
    }

    /// Escape or Close (`closeFind`): highlights clear, the query stays.
    pub fn find_close(&mut self) {
        if !self.find.open {
            return;
        }
        self.find.open = false;
        self.find.scrolled = None;
        self.relayout = true;
        if matches!(self.focus, Some(Target::Find(_))) {
            self.set_focus(None, false);
        }
        self.find_refresh();
        self.events.push(Event::FindOpen(false));
        self.sync_caret();
    }

    /// Next or previous match, wrapping (`goToMatch`).
    pub fn find_step(&mut self, forward: bool) {
        let count = self.find.matches.len();
        if count == 0 {
            return;
        }
        self.find.index = if forward {
            (self.find.index + 1) % count
        } else {
            (self.find.index + count - 1) % count
        };
        self.find_refresh();
    }

    /// Edit the find input; text changes restart matching at the first
    /// match (`updateQuery`).
    pub fn field_edit(&mut self, g: &Gfx, edit: impl FnOnce(&mut Editor) -> Change) {
        let change = edit(&mut self.find.field.editor);
        if !change.any() {
            return;
        }
        match self.find.field.apply(g, &self.t, change) {
            Ok(redraw) => {
                if redraw {
                    self.nodes[self.find_nodes.text].dirty = true;
                }
            }
            Err(error) => eprintln!("text layout failed: {error:?}"),
        }
        if change.text {
            self.find.index = 0;
            self.find.scrolled = None;
            self.find_refresh();
            self.events.push(Event::FindText);
        }
        self.events.push(Event::FindCaret);
        self.sync_caret();
    }

    /// The text position under client x `x` in the find input.
    pub fn field_hit(&self, x: f32) -> usize {
        let r = self.absolute(self.find_nodes.text);
        self.find.field.hit(x - r.x)
    }

    /// Place the caret node: shown while the input has focus in an active
    /// window and nothing is selected (inside a composition too).
    fn sync_caret(&mut self) {
        let editor = &self.find.field.editor;
        let (a, b) = editor.selection();
        let shown = self.text_focused() && self.window_focused && (a == b || editor.composing());
        let text = self.nodes[self.find_nodes.text].rect;
        let (x, y, h) = self.find.field.caret_box(text.x, text.h);
        let r = Rect::new(x, y, 1., h);
        let node = &mut self.nodes[self.find_nodes.caret];
        if node.visible != shown || node.rect != r {
            node.visible = shown;
            node.rect = r;
            node.props = true;
        }
        if shown {
            self.caret_blink = true;
        }
    }

    /// The window gained or lost keyboard focus.
    pub fn window_focus(&mut self, focused: bool) {
        if self.window_focused != focused {
            self.window_focused = focused;
            self.nodes[self.find_nodes.text].dirty = true;
            self.sync_caret();
        }
    }

    /// The caret node, when shown.
    pub fn caret_node(&self) -> Option<usize> {
        let caret = self.find_nodes.caret;
        self.nodes[caret].visible.then_some(caret)
    }

    /// The find bar, its count and its input's text box, in client DIPs.
    pub fn find_rects(&self) -> (Rect, Rect, Rect) {
        let field = self.absolute(self.find_nodes.field);
        let close = self.absolute(self.find_nodes.buttons[FIND_BUTTONS.len() - 1].base);
        let bar = Rect::new(field.x, field.y, close.x + close.w - field.x, field.h);
        (
            bar,
            self.absolute(self.find_nodes.count),
            self.absolute(self.find_nodes.text),
        )
    }

    pub fn find_field(&self) -> &TextField {
        &self.find.field
    }

    // ---- geometry for input and accessibility (client DIPs) ----

    /// A node's rect in client DIPs, at the live (possibly animating) scroll.
    pub fn absolute(&self, node: usize) -> Rect {
        let mut r = self.nodes[node].rect;
        let mut parent = self.nodes[node].parent;
        while let Some(p) = parent {
            let pr = self.nodes[p].rect;
            r = r.offset(pr.x, pr.y);
            if p == self.content {
                r = r.offset(0., self.scroll - self.scroll_now());
            }
            parent = self.nodes[p].parent;
        }
        r
    }

    fn content_rect(&self, r: Rect) -> Rect {
        let viewport = self.viewport_rect();
        r.offset(viewport.x, viewport.y - self.scroll_now())
    }

    pub fn target_rect(&self, target: Target) -> Rect {
        self.absolute(match target {
            Target::Nav(i) => self.nav_items[i].base,
            Target::History => self.history.base,
            Target::Row(row) => self.row_nodes[row].switch,
            Target::Mode(i) => self.segments[i],
            Target::Theme(i) => self.chips[i].group,
            Target::Choose => self.choose.base,
            Target::Open(which) => self.open_buttons[which.index()].base,
            Target::Shortcut(i) => self.shortcuts[i].recorder,
            Target::Select(key) => self.selects[key.index()].trigger,
            Target::Option(key, i) => return self.option_rect(key, i),
            Target::Find(FindPart::Input) => self.find_nodes.field,
            Target::Find(part) => self.find_nodes.buttons[part.index() - 1].base,
        })
    }

    /// Updates' Check Now and status, in client DIPs.
    pub fn update_rects(&self) -> (Rect, Rect) {
        (
            self.absolute(self.update_action),
            self.absolute(self.update_status),
        )
    }

    /// A node's copy block (title and detail), in client DIPs.
    pub fn update_copy_rect(&self) -> Rect {
        self.absolute(self.update_copy)
    }

    /// An option row of the open listbox, in client DIPs.
    fn option_rect(&self, key: SelectKey, index: usize) -> Rect {
        if self.open.is_none_or(|(open, _)| open != key) {
            return Rect::default();
        }
        let s2 = self.t.n("s-2");
        let panel = self.nodes[self.list.panel].rect;
        let h = self.option_h();
        Rect::new(
            panel.x + 1. + s2,
            panel.y + 1. + s2 + index as f32 * h - self.list_scroll,
            panel.w - 2. - 2. * s2,
            h,
        )
    }

    /// The visible inside of the open listbox, in client DIPs.
    pub fn list_rect(&self) -> Rect {
        let panel = self.nodes[self.list.panel].rect;
        Rect::new(panel.x + 1., panel.y + 1., panel.w - 2., panel.h - 2.)
    }

    pub fn item_rect(&self, item: Item) -> Rect {
        self.item_rects
            .iter()
            .find(|(i, _)| *i == item)
            .map_or_else(Rect::default, |(_, r)| self.content_rect(*r))
    }

    pub fn row_rect(&self, row: usize) -> Rect {
        self.item_rect(Item::Switch(row))
    }

    pub fn card_rect(&self, card: usize) -> Rect {
        self.content_rect(self.card_rects[card])
    }

    /// The Interface theme control (`role="group"`).
    pub fn mode_rect(&self) -> Rect {
        self.absolute(self.seg_track)
    }

    /// The accent chip grid (`role="radiogroup"`).
    pub fn accent_rect(&self) -> Rect {
        self.content_rect(self.grid_rect)
    }

    pub fn field_rect(&self) -> Rect {
        self.absolute(self.field)
    }

    pub fn viewport(&self) -> Rect {
        self.viewport_rect()
    }

    pub fn header_text_rect(&self, subtitle: bool) -> Rect {
        let (title, sub) = (self.texts.title.as_ref(), self.texts.subtitle.as_ref());
        let (th, sw, sh, tw) = (
            title.map_or(0., |t| t.height),
            sub.map_or(0., |t| t.width),
            sub.map_or(0., |t| t.height),
            title.map_or(0., |t| t.width),
        );
        let x = NAV_WIDTH + self.t.n("s-8");
        if subtitle {
            Rect::new(x, self.title_y + th + 2., sw, sh)
        } else {
            Rect::new(x, self.title_y, tw, th)
        }
    }

    pub fn hit(&self, x: f32, y: f32) -> Option<Target> {
        if let Some((key, _)) = self.open
            && self.list_contains(x, y)
        {
            // The listbox covers whatever is beneath it.
            let inside = self.list_rect().contains(x, y);
            let count = self
                .texts
                .options
                .get(key.index())
                .map_or(0, |o| o.regular.len());
            let disabled: Vec<bool> = self.select_options(key).iter().map(|(_, d)| *d).collect();
            return (0..count)
                .filter(|&i| !disabled.get(i).copied().unwrap_or(true))
                .find(|&i| inside && self.option_rect(key, i).contains(x, y))
                .map(|i| Target::Option(key, i));
        }
        for (i, item) in self.nav_items.iter().enumerate() {
            if self.absolute(item.base).contains(x, y) {
                return Some(Target::Nav(i));
            }
        }
        if self.absolute(self.history.base).contains(x, y) {
            return Some(Target::History);
        }
        if self.find.open {
            let parts = [FindPart::Input].into_iter().chain(FIND_BUTTONS);
            for part in parts {
                if self.target_rect(Target::Find(part)).contains(x, y) {
                    return Some(Target::Find(part));
                }
            }
        }
        if !self.viewport_rect().contains(x, y) {
            return None;
        }
        let controls = (0..APPEARANCE_MODES.len())
            .map(Target::Mode)
            .chain((0..THEMES.len()).map(Target::Theme))
            .chain([
                Target::Choose,
                Target::Open(Utility::Keyboard),
                Target::Open(Utility::Feedback),
            ])
            .chain((0..SHORTCUT_ROWS.len()).map(Target::Shortcut))
            .chain(SELECTS.map(Target::Select));
        for target in controls {
            if self.target_rect(target).contains(x, y) {
                return Some(target);
            }
        }
        (0..self.rows.len())
            .find(|&row| self.row_rect(row).contains(x, y))
            .map(Target::Row)
    }

    // ---- drawing ----

    /// Draw `node`'s own content in its local DIPs (origin at its rect).
    pub fn draw(&self, dc: &ID2D1DeviceContext, g: &Gfx, node: usize) -> Result<()> {
        let t = &self.t;
        let c = |name: &str| t.color(name);
        let n = &self.nodes[node];
        let (w, h) = (n.rect.w, n.rect.h);
        let all = rect(0., 0., w, h);
        let accent_ring = t.color_alpha("theme-accent", 0.55);
        let centered = |text: &Text, x: f32| (x, (h - text.height) / 2.);
        match n.kind {
            Kind::Group => {}
            Kind::NavBg => {
                gfx::fill(dc, all, c("surface-sunken"))?;
                gfx::fill(dc, rect(w - 1., 0., w, h), c("border-subtle"))?;
                // `.preferences-nav-brand`: 26 px accent tile, 16 px glyph.
                let (s3, s5, s6) = (t.n("s-3"), t.n("s-5"), t.n("s-6"));
                let x = s5 + s3;
                gfx::fill_round(
                    dc,
                    rect(x, s6, x + 26., s6 + 26.),
                    t.n("r-md"),
                    c("theme-accent"),
                )?;
                gfx::icon(
                    dc,
                    &g.factory,
                    "capture",
                    &[1],
                    (x + 5., s6 + 5., 16.),
                    1.8,
                    c("theme-accent-ink"),
                )?;
                if let Some(brand) = &self.texts.brand {
                    let y = s6 + (26. - brand.height) / 2.;
                    gfx::text(dc, brand, x + 26. + t.n("s-4"), y, c("text"))?;
                }
            }
            Kind::NavItem { index, lit } => {
                let label = &self.texts.nav[index];
                let (x, y) = centered(label, t.n("s-4"));
                if lit {
                    // Opaque so the resting label never shows through.
                    gfx::fill(dc, all, c("surface-sunken"))?;
                    let bg = if index == self.active {
                        "surface-active"
                    } else {
                        "surface-hover"
                    };
                    gfx::fill_round(dc, all, t.n("r-md"), c(bg))?;
                    gfx::text(dc, label, x, y, c("text"))?;
                } else {
                    gfx::text(dc, label, x, y, c("text-subtle"))?;
                }
            }
            Kind::HeaderBg => {
                gfx::fill(dc, all, c("surface-canvas"))?;
                gfx::fill(dc, rect(0., h - 1., w, h), c("border-subtle"))?;
                let x = t.n("s-8");
                if let (Some(title), Some(sub)) = (&self.texts.title, &self.texts.subtitle) {
                    gfx::text(dc, title, x, self.title_y, c("text"))?;
                    let y = self.title_y + title.height + 2.;
                    gfx::text(dc, sub, x, y, c("text-subtle"))?;
                }
            }
            Kind::History { lit } => {
                let radius = t.n("r-md");
                gfx::fill_round(dc, all, radius, c("surface-raised"))?;
                if lit {
                    gfx::fill_round(dc, all, radius, c("surface-hover"))?;
                }
                gfx::border(dc, all, radius, 1., c("border"))?;
                if let Some(label) = &self.texts.history {
                    let (x, y) = centered(label, t.n("s-5") + 1.);
                    gfx::text(dc, label, x, y, c("text"))?;
                }
            }
            Kind::Viewport => gfx::fill(dc, all, c("surface-canvas"))?,
            Kind::Card(card) => {
                let radius = t.n("r-xl");
                gfx::shadow(dc, all, radius, t.shadow("shadow-sm"))?;
                gfx::fill_round(dc, all, radius, c("surface-raised"))?;
                gfx::border(dc, all, radius, 1., c("border-subtle"))?;
                let inset = t.n("s-6") + 1.;
                let title = &self.texts.card_titles[card];
                gfx::text(dc, title, inset, inset, c("text"))?;
                let y = inset + title.height + t.n("s-2");
                gfx::text(
                    dc,
                    &self.texts.card_descriptions[card],
                    inset,
                    y,
                    c("text-subtle"),
                )?;
                for &y in &self.separators[card] {
                    gfx::fill(dc, rect(inset, y, w - inset, y + 1.), c("border-subtle"))?;
                }
            }
            Kind::Copy(item) => {
                if let Some(copy) = self.texts.copies.iter().find(|copy| copy.item == item) {
                    gfx::text(dc, &copy.title, 0., 0., c("text"))?;
                    if let Some(detail) = &copy.detail {
                        let y = copy.title.height + copy.gap;
                        gfx::text(dc, detail, 0., y, c("text-subtle"))?;
                    }
                }
            }
            Kind::TrackOff(row) => {
                let state = &self.rows[row];
                let hovered = state.enabled && self.hover == Some(Target::Row(row));
                gfx::fill_round(dc, all, h / 2., c("surface-sunken"))?;
                let edge = if hovered {
                    "text-faint"
                } else {
                    "border-strong"
                };
                gfx::border(dc, all, h / 2., 1., c(edge))?;
            }
            Kind::TrackOn => gfx::fill_round(dc, all, h / 2., c("theme-accent"))?,
            Kind::KnobOff => {
                gfx::shadow(dc, all, w / 2., t.shadow("shadow-xs"))?;
                gfx::fill_round(dc, all, w / 2., c("text-subtle"))?;
            }
            Kind::KnobOn => gfx::fill_round(dc, all, w / 2., c("theme-accent-ink"))?,
            Kind::SegTrack => {
                let radius = t.n("r-lg");
                gfx::fill_round(dc, all, radius, c("surface-sunken"))?;
                gfx::border(dc, all, radius, 1., c("border-subtle"))?;
            }
            Kind::SegThumb => {
                let radius = t.n("r-sm");
                gfx::shadow(dc, all, radius, t.shadow("shadow-xs"))?;
                gfx::fill_round(dc, all, radius, c("surface-raised"))?;
                gfx::border(dc, all, radius, 1., c("border-subtle"))?;
            }
            Kind::SegLabel { index, lit } => {
                let label = &self.texts.segments[index];
                let (x, y) = centered(label, (w - label.width) / 2.);
                let color = if lit { "text" } else { "text-subtle" };
                gfx::text(dc, label, x, y, c(color))?;
            }
            Kind::ChipHover(_) => gfx::fill_round(dc, all, t.n("r-md"), c("surface-hover"))?,
            Kind::Chip { index, active } => {
                let radius = t.n("r-md");
                if active {
                    gfx::shadow(dc, all, radius, t.shadow("shadow-xs"))?;
                    gfx::fill_round(dc, all, radius, c("surface-raised"))?;
                    gfx::border(dc, all, radius, 1., c("border"))?;
                }
                // 1 px border, `0 var(--s-3)` padding, `18px 1fr 10px` columns.
                let s3 = t.n("s-3");
                let left = 1. + s3;
                let top = (h - SWATCH) / 2.;
                let theme = &THEMES[index];
                let colors = (theme.id != "custom").then(|| t.swatch(theme.id));
                let tile = rect(left, top, left + SWATCH, top + SWATCH);
                gfx::swatch(dc, &g.factory, tile, t.n("r-sm"), colors)?;
                let label = &self.texts.chips[index];
                let name_x = left + SWATCH + s3;
                let check_x = w - 1. - s3 - 10.;
                let color = if active { "text" } else { "text-muted" };
                unsafe {
                    dc.PushAxisAlignedClip(
                        &rect(name_x, 0., check_x - s3, h),
                        windows::Win32::Graphics::Direct2D::D2D1_ANTIALIAS_MODE_ALIASED,
                    );
                }
                gfx::text(dc, label, name_x, (h - label.height) / 2., c(color))?;
                unsafe { dc.PopAxisAlignedClip() };
                if active && let Some(check) = &self.texts.check {
                    let mark = if t.appearance == "dark" {
                        "theme-accent"
                    } else {
                        "theme-accent-readable"
                    };
                    let x = check_x + (10. - check.width) / 2.;
                    gfx::text(dc, check, x, (h - check.height) / 2., c(mark))?;
                }
            }
            Kind::Field => {
                let radius = t.n("r-md");
                gfx::fill_round(dc, all, radius, c("surface-field"))?;
                gfx::border(dc, all, radius, 1., c("control-border"))?;
                if let Some(path) = &self.texts.path {
                    let pad = t.n("s-5") + 1.;
                    unsafe {
                        dc.PushAxisAlignedClip(
                            &rect(pad, 0., w - pad, h),
                            windows::Win32::Graphics::Direct2D::D2D1_ANTIALIAS_MODE_ALIASED,
                        );
                    }
                    gfx::text(dc, path, pad, (h - path.height) / 2., c("text"))?;
                    unsafe { dc.PopAxisAlignedClip() };
                }
            }
            Kind::Choose { lit } | Kind::Open { lit, .. } => {
                let radius = t.n("r-md");
                let (bg, edge) = if lit {
                    ("control-hover", "border-strong")
                } else {
                    ("control", "control-border")
                };
                if !lit {
                    gfx::shadow(dc, all, radius, t.shadow("shadow-xs"))?;
                }
                gfx::fill_round(dc, all, radius, c(bg))?;
                gfx::border(dc, all, radius, 1., c(edge))?;
                let label = match n.kind {
                    Kind::Open { which, .. } => &self.texts.open[which.index()],
                    _ => &self.texts.choose,
                };
                if let Some(label) = label {
                    let (x, y) = centered(label, (w - label.width) / 2.);
                    gfx::text(dc, label, x, y, c("text"))?;
                }
            }
            Kind::UpdateAction => {
                // The resting `.settings-utility-action`; the node fades it.
                let radius = t.n("r-md");
                gfx::shadow(dc, all, radius, t.shadow("shadow-xs"))?;
                gfx::fill_round(dc, all, radius, c("control"))?;
                gfx::border(dc, all, radius, 1., c("control-border"))?;
                if let Some(label) = &self.texts.update_action {
                    let (x, y) = centered(label, (w - label.width) / 2.);
                    gfx::text(dc, label, x, y, c("text"))?;
                }
            }
            Kind::UpdateStatus => {
                if let Some(status) = &self.texts.update_status {
                    gfx::text(dc, status, 0., 0., c("text-subtle"))?;
                }
            }
            Kind::ShortcutLabel(index) => {
                if let Some(label) = self.texts.shortcut_labels.get(index) {
                    gfx::text(dc, label, 0., 0., c("text"))?;
                }
            }
            Kind::ShortcutError(index) => {
                if let Some(Some(error)) = self.texts.shortcut_errors.get(index) {
                    gfx::text(dc, error, 0., 0., c("danger-text"))?;
                }
            }
            Kind::Recorder(index) => {
                let radius = t.n("r-md");
                gfx::fill_round(dc, all, radius, c("surface-field"))?;
                gfx::border(dc, all, radius, 1., c("control-border"))?;
                // `padding: 2px var(--s-4)`, caps `--s-3` apart.
                let left = 1. + t.n("s-4");
                let caps = self.texts.caps.get(index).filter(|caps| !caps.is_empty());
                let clip = rect(left, 0., w - left, h);
                unsafe {
                    dc.PushAxisAlignedClip(
                        &clip,
                        windows::Win32::Graphics::Direct2D::D2D1_ANTIALIAS_MODE_ALIASED,
                    );
                }
                match caps {
                    Some(caps) => {
                        let mut x = left;
                        for cap in caps {
                            // `kbd`: 2/5 padding, 1 px border with a 2 px
                            // bottom, `--r-xs`, min-width 20.
                            let cap_w = (cap.width + 12.).max(20.).ceil();
                            let cap_h = (cap.height + 7.).round();
                            let top = ((h - cap_h) / 2.).round();
                            let outer = rect(x, top, x + cap_w, top + cap_h);
                            let inner = rect(x + 1., top + 1., x + cap_w - 1., top + cap_h - 2.);
                            let r_xs = t.n("r-xs");
                            gfx::fill_round(dc, outer, r_xs, c("border"))?;
                            gfx::fill_round(dc, inner, r_xs - 1., c("surface-raised"))?;
                            let tx = x + (cap_w - cap.width) / 2.;
                            gfx::text(dc, cap, tx, top + 3., c("text-muted"))?;
                            x += cap_w + t.n("s-3");
                        }
                    }
                    None => {
                        if let Some(prompt) = &self.texts.prompt {
                            let y = (h - prompt.height) / 2.;
                            gfx::text(dc, prompt, left, y, c("text-faint"))?;
                        }
                    }
                }
                unsafe { dc.PopAxisAlignedClip() };
            }
            Kind::RecorderHover(_) | Kind::TriggerHover(_) => {
                // Cover the resting border, then the hover one.
                let radius = t.n("r-md");
                gfx::border(dc, all, radius, 1., c("surface-field"))?;
                gfx::border(dc, all, radius, 1., c("border-strong"))?;
            }
            Kind::Trigger(key) => {
                let radius = t.n("r-md");
                gfx::fill_round(dc, all, radius, c("surface-field"))?;
                gfx::border(dc, all, radius, 1., c("control-border"))?;
                if let Some(label) = self.texts.triggers.get(key.index()) {
                    let left = 1. + t.n("s-4");
                    let right = w - left - CHEVRON - t.n("s-5");
                    unsafe {
                        dc.PushAxisAlignedClip(
                            &rect(left, 0., right.max(left), h),
                            windows::Win32::Graphics::Direct2D::D2D1_ANTIALIAS_MODE_ALIASED,
                        );
                    }
                    gfx::text(dc, label, left, (h - label.height) / 2., c("text"))?;
                    unsafe { dc.PopAxisAlignedClip() };
                }
            }
            Kind::Chevron(_) => {
                // `viewBox="0 0 16 16"` at 14 px, stroke 1.7 units.
                let scale = CHEVRON / 16.;
                gfx::path(
                    dc,
                    &g.factory,
                    CHEVRON_PATH,
                    false,
                    (0., 0., scale),
                    1.7 * scale,
                    c("text-subtle"),
                )?;
            }
            Kind::Listbox => {
                let radius = t.n("r-lg");
                gfx::shadow(dc, all, radius, t.shadow("shadow-lg"))?;
                gfx::fill_round(dc, all, radius, c("surface-overlay"))?;
                gfx::border(dc, all, radius, 1., c("border"))?;
            }
            Kind::Options => {
                if let Some((key, _)) = self.open {
                    let options = self.select_options(key);
                    let selected = self.select_index(key);
                    let (s2, h) = (t.n("s-2"), self.option_h());
                    for (index, (_, disabled)) in options.iter().enumerate() {
                        let y = s2 + index as f32 * h;
                        let color = if *disabled {
                            "text-faint"
                        } else if index == selected {
                            "text"
                        } else {
                            "text-muted"
                        };
                        self.draw_option(dc, key, index, (s2, y, w - 2. * s2), c(color))?;
                    }
                }
            }
            Kind::OptionLit => {
                if let Some((key, state)) = self.open {
                    // Opaque, so the resting label never shows through.
                    gfx::fill(dc, all, c("surface-overlay"))?;
                    gfx::fill_round(dc, all, t.n("r-sm"), c("surface-hover"))?;
                    self.draw_option(dc, key, state.active, (0., 0., w), c("text"))?;
                }
            }
            Kind::Ring {
                shape: Ring::Tight(radius),
                ..
            } => gfx::ring(dc, all, radius, 0., 2., accent_ring)?,
            Kind::Ring {
                shape: Ring::Field(radius),
                ..
            } => {
                gfx::border(dc, all, radius, 1., c("surface-field"))?;
                gfx::border(dc, all, radius, 1., c("theme-accent"))?;
                gfx::ring(dc, all, radius, 0., 2., accent_ring)?;
            }
            Kind::Ring {
                shape: Ring::Full(radius),
                ..
            } => {
                gfx::ring(dc, all, radius, 0., 2., c("surface-canvas"))?;
                gfx::ring(dc, all, radius, 2., 4., accent_ring)?;
            }
            Kind::Scrollbar => gfx::fill_round(dc, all, w / 2., c("border-strong"))?,
            Kind::FindField => {
                let radius = t.n("r-md");
                gfx::fill_round(dc, all, radius, c("surface-field"))?;
                gfx::border(dc, all, radius, 1., c("control-border"))?;
            }
            Kind::FindFieldHover => {
                let radius = t.n("r-md");
                gfx::border(dc, all, radius, 1., c("surface-field"))?;
                gfx::border(dc, all, radius, 1., c("border-strong"))?;
            }
            Kind::FindText => {
                let focused = self.text_focused() && self.window_focused;
                self.find.field.draw(dc, t, (0., w, h), focused)?;
            }
            Kind::Caret => gfx::fill(dc, all, c("text"))?,
            Kind::FindCount => {
                if let Some(count) = &self.find.count {
                    gfx::text(dc, count, 0., (h - count.height) / 2., c("text-subtle"))?;
                }
            }
            Kind::FindButton { part, lit } => {
                let radius = t.n("r-md");
                let (bg, edge, ink) = if lit {
                    ("control-hover", "border-strong", "text")
                } else {
                    ("control", "control-border", "text-subtle")
                };
                if !lit {
                    gfx::shadow(dc, all, radius, t.shadow("shadow-xs"))?;
                }
                gfx::fill_round(dc, all, radius, c(bg))?;
                gfx::border(dc, all, radius, 1., c(edge))?;
                // 14 px icons, stroke 1.7 user units of their view box.
                let (d, units) = match part {
                    FindPart::Previous => (FIND_PREVIOUS_PATH, 16.),
                    FindPart::Next => (FIND_NEXT_PATH, 16.),
                    _ => (FIND_CLOSE_PATH, 24.),
                };
                let scale = FIND_ICON / units;
                let (x, y) = ((w - FIND_ICON) / 2., (h - FIND_ICON) / 2.);
                gfx::path(dc, &g.factory, d, false, (x, y, scale), 1.7 * scale, c(ink))?;
            }
            Kind::Match(index) => {
                let radius = t.n("r-lg");
                if self.find.current == Some(index) {
                    // `0 0 0 2px` accent at 0.62 and a `0 0 20px` glow at 0.18.
                    gfx::glow(dc, all, radius, 20., t.color_alpha("theme-accent", 0.18))?;
                    gfx::ring(dc, all, radius, 0., 2., t.color_alpha("theme-accent", 0.62))?;
                }
                gfx::fill_round(dc, all, radius, c("surface-selected"))?;
            }
        }
        Ok(())
    }

    /// One option row at `(x, y)` across `width`: label (medium when
    /// selected) and the selected check in the readable accent.
    fn draw_option(
        &self,
        dc: &ID2D1DeviceContext,
        key: SelectKey,
        index: usize,
        (x, y, width): (f32, f32, f32),
        color: windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F,
    ) -> Result<()> {
        let t = &self.t;
        let Some(texts) = self.texts.options.get(key.index()) else {
            return Ok(());
        };
        let selected = index == self.select_index(key);
        let label = if selected {
            texts.medium.get(index)
        } else {
            texts.regular.get(index)
        };
        let h = self.option_h();
        let pad = t.n("s-4");
        if let Some(label) = label {
            gfx::text(dc, label, x + pad, y + (h - label.height) / 2., color)?;
        }
        if selected && let Some(check) = &self.texts.option_check {
            let mark = if t.appearance == "dark" {
                "theme-accent"
            } else {
                "theme-accent-readable"
            };
            let cx = x + width - pad - check.width;
            gfx::text(dc, check, cx, y + (h - check.height) / 2., t.color(mark))?;
        }
        Ok(())
    }

    /// Paint the whole tree at `scale` into `dc` (offscreen screenshots).
    pub fn paint(&self, dc: &ID2D1DeviceContext, g: &Gfx, scale: f32) -> Result<()> {
        self.paint_node(dc, g, scale, 0, (0., 0.))
    }

    fn paint_node(
        &self,
        dc: &ID2D1DeviceContext,
        g: &Gfx,
        scale: f32,
        node: usize,
        origin: (f32, f32),
    ) -> Result<()> {
        use windows::Win32::Graphics::Direct2D::*;
        use windows_numerics::Matrix3x2;
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
            if n.clip {
                dc.PushAxisAlignedClip(
                    &rect(
                        x * scale,
                        y * scale,
                        (x + n.rect.w) * scale,
                        (y + n.rect.h) * scale,
                    ),
                    D2D1_ANTIALIAS_MODE_ALIASED,
                );
            }
            if n.kind.has_content() {
                dc.SetTransform(&Matrix3x2 {
                    M11: scale,
                    M12: 0.,
                    M21: 0.,
                    M22: scale,
                    M31: (x * scale).round(),
                    M32: (y * scale).round(),
                });
                self.draw(dc, g, node)?;
            }
            for &child in &self.children[node] {
                self.paint_node(dc, g, scale, child, (x, y))?;
            }
            dc.SetTransform(&Matrix3x2::identity());
            if n.clip {
                dc.PopAxisAlignedClip();
            }
            if layer {
                dc.PopLayer();
            }
        }
        Ok(())
    }
}

/// A card's header copy; Shortcuts appends the platform intro like shipping.
pub fn card_description(section: usize) -> String {
    let description = SECTIONS[section].description;
    if SECTIONS[section].title == "Shortcuts" {
        format!(
            "{description} {}",
            preferences::shortcut_help(PLATFORM).intro
        )
    } else {
        description.into()
    }
}

fn shortcut_ref(settings: &captures_settings::AppSettings, index: usize) -> &str {
    match SHORTCUT_ROWS[index].1 {
        ["new_capture_shortcut"] => &settings.new_capture_shortcut,
        ["region_shortcut"] => &settings.region_shortcut,
        ["window_shortcut"] => &settings.window_shortcut,
        ["display_shortcut"] => &settings.display_shortcut,
        ["recording", "video_shortcut"] => &settings.recording.video_shortcut,
        ["recording", "window_shortcut"] => &settings.recording.window_shortcut,
        ["recording", "display_shortcut"] => &settings.recording.display_shortcut,
        path => unreachable!("unknown shortcut {path:?}"),
    }
}

fn shortcut_mut(settings: &mut captures_settings::AppSettings, index: usize) -> &mut String {
    match SHORTCUT_ROWS[index].1 {
        ["new_capture_shortcut"] => &mut settings.new_capture_shortcut,
        ["region_shortcut"] => &mut settings.region_shortcut,
        ["window_shortcut"] => &mut settings.window_shortcut,
        ["display_shortcut"] => &mut settings.display_shortcut,
        ["recording", "video_shortcut"] => &mut settings.recording.video_shortcut,
        ["recording", "window_shortcut"] => &mut settings.recording.window_shortcut,
        ["recording", "display_shortcut"] => &mut settings.recording.display_shortcut,
        path => unreachable!("unknown shortcut {path:?}"),
    }
}

/// The `captures-settings` default for a recorder.
fn shortcut_default(index: usize) -> String {
    match SHORTCUT_ROWS[index].1 {
        ["new_capture_shortcut"] => captures_settings::default_new_capture_shortcut(),
        ["region_shortcut"] => captures_settings::default_region_shortcut(),
        ["window_shortcut"] => captures_settings::default_window_shortcut(),
        ["display_shortcut"] => captures_settings::default_display_shortcut(),
        ["recording", "video_shortcut"] => captures_settings::default_video_shortcut(),
        ["recording", "window_shortcut"] => captures_settings::default_record_window_shortcut(),
        ["recording", "display_shortcut"] => captures_settings::default_record_display_shortcut(),
        path => unreachable!("unknown shortcut {path:?}"),
    }
}

fn recording_value(settings: &captures_settings::AppSettings, toggle: usize) -> bool {
    let recording = &settings.recording;
    match RECORDING_TOGGLES[toggle] {
        "capture_system_audio" => recording.capture_system_audio,
        "mono_audio" => recording.mono_audio,
        "show_cursor" => recording.show_cursor,
        "highlight_clicks" => recording.highlight_clicks,
        "open_editor_after_recording" => recording.open_editor_after_recording,
        key => unreachable!("unknown switch {key}"),
    }
}

fn set_recording(settings: &mut captures_settings::AppSettings, toggle: usize, on: bool) {
    let recording = &mut settings.recording;
    match RECORDING_TOGGLES[toggle] {
        "capture_system_audio" => recording.capture_system_audio = on,
        "mono_audio" => recording.mono_audio = on,
        "show_cursor" => recording.show_cursor = on,
        "highlight_clicks" => recording.highlight_clicks = on,
        "open_editor_after_recording" => recording.open_editor_after_recording = on,
        key => unreachable!("unknown switch {key}"),
    }
}

fn flag_value(settings: &captures_settings::AppSettings, flag: usize) -> bool {
    match FLAGS[flag] {
        "auto_copy_to_clipboard" => settings.auto_copy_to_clipboard,
        "auto_start_on_selection" => settings.auto_start_on_selection,
        "freeze_screen" => settings.freeze_screen,
        "show_cursor_in_screenshots" => settings.show_cursor_in_screenshots,
        key => unreachable!("unknown switch {key}"),
    }
}

fn set_flag(settings: &mut captures_settings::AppSettings, flag: usize, on: bool) {
    match FLAGS[flag] {
        "auto_copy_to_clipboard" => settings.auto_copy_to_clipboard = on,
        "auto_start_on_selection" => settings.auto_start_on_selection = on,
        "freeze_screen" => settings.freeze_screen = on,
        "show_cursor_in_screenshots" => settings.show_cursor_in_screenshots = on,
        key => unreachable!("unknown switch {key}"),
    }
}
