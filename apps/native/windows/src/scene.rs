//! The Preferences window as a retained tree of nodes, following shipping
//! `.preferences` (`styles/windows.css`) and the copy in
//! `captures_app::preferences`.
//!
//! Each node with content maps to one DirectComposition surface (see
//! `host.rs`) and is redrawn only when its own `dirty` flag is set. Hover,
//! active and toggle transitions animate node opacity and offsets on the
//! compositor; the only CPU redraw on hover is a 32×19 switch track. The same
//! tree paints offscreen for `--screenshot`.
use std::path::PathBuf;
use std::time::Instant;

use captures_app::motion::CubicBezier;
use captures_app::preferences::{self, SECTIONS};
use windows::Win32::Graphics::Direct2D::ID2D1DeviceContext;
use windows::core::Result;

use crate::gfx::{self, Gfx, Style, Text, rect};
use crate::tokens::Tokens;

pub const NAV_WIDTH: f32 = 196.;
const SWITCH: (f32, f32) = (32., 19.);
const KNOB: f32 = 13.;
const KNOB_TRAVEL: f32 = 13.;
const SECTIONS_MAX: f32 = 720.;
const SCROLLBAR: f32 = 10.;
/// Cards in shipping order: (section index, rows).
pub const CARDS: [(usize, &[usize]); 2] = [(0, &[0]), (2, &[1, 2, 3, 4])];
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
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Ring {
    /// `--focus-ring-tight`: 2 px of accent at 55%.
    Tight(f32),
    /// `--focus-ring`: 2 px of canvas, then 2 px of accent at 55%.
    Full(f32),
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
    RowCopy(usize),
    TrackOff(usize),
    TrackOn,
    KnobOff,
    KnobOn,
    Ring(Ring),
    Thumb,
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
pub enum Target {
    Nav(usize),
    History,
    Row(usize),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RowKey {
    Login,
    Flag(usize),
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
}

/// Accessibility notifications, raised by the host outside its borrow.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Event {
    Focus(Target),
    Toggled(usize),
    Detail(usize),
}

struct RowNodes {
    copy: usize,
    switch: usize,
    track_off: usize,
    track_on: usize,
    knob: usize,
    knob_on: usize,
    ring: usize,
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
    row_titles: Vec<Text>,
    row_details: Vec<Option<Text>>,
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
    settings: captures_settings::AppSettings,
    settings_file: PathBuf,
    texts: Texts,
    nav_items: Vec<(usize, usize, usize)>,
    nav_bg: usize,
    header: usize,
    history: (usize, usize, usize),
    viewport: usize,
    content: usize,
    cards: Vec<usize>,
    row_nodes: Vec<RowNodes>,
    thumb: usize,
    // Layout results, DIPs.
    header_h: f32,
    title_y: f32,
    card_rects: Vec<Rect>,
    separators: Vec<Vec<f32>>,
    /// Whole `label` rows, in content coordinates.
    row_rects: Vec<Rect>,
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
            settings,
            settings_file,
            texts: Texts::default(),
            nav_items: Vec::new(),
            nav_bg: 0,
            header: 0,
            history: (0, 0, 0),
            viewport: 0,
            content: 0,
            cards: Vec::new(),
            row_nodes: Vec::new(),
            thumb: 0,
            header_h: 0.,
            title_y: 0.,
            card_rects: Vec::new(),
            separators: Vec::new(),
            row_rects: Vec::new(),
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

    fn build(&mut self) {
        let root = self.add(None, Kind::Group, 0.);
        self.nav_bg = self.add(Some(root), Kind::NavBg, 0.);
        for index in 0..SECTIONS.len() {
            let base = self.add(Some(self.nav_bg), Kind::NavItem { index, lit: false }, 0.);
            let lit = self.add(Some(base), Kind::NavItem { index, lit: true }, 0.);
            let ring = self.add(Some(base), Kind::Ring(Ring::Tight(8.)), 2.);
            self.nav_items.push((base, lit, ring));
        }
        self.header = self.add(Some(root), Kind::HeaderBg, 0.);
        let base = self.add(Some(self.header), Kind::History { lit: false }, 0.);
        let lit = self.add(Some(base), Kind::History { lit: true }, 0.);
        let ring = self.add(Some(base), Kind::Ring(Ring::Full(8.)), 4.);
        self.history = (base, lit, ring);
        self.viewport = self.add(Some(root), Kind::Viewport, 0.);
        self.content = self.add(Some(self.viewport), Kind::Group, 0.);
        for (card, (_, rows)) in CARDS.iter().enumerate() {
            let node = self.add(Some(self.content), Kind::Card(card), 8.);
            self.cards.push(node);
            for &row in *rows {
                let copy = self.add(Some(node), Kind::RowCopy(row), 0.);
                let switch = self.add(Some(node), Kind::Group, 0.);
                self.nodes[switch].group_opacity = true;
                let track_off = self.add(Some(switch), Kind::TrackOff(row), 0.);
                let track_on = self.add(Some(switch), Kind::TrackOn, 0.);
                let knob = self.add(Some(switch), Kind::Group, 0.);
                self.add(Some(knob), Kind::KnobOff, 3.);
                let knob_on = self.add(Some(knob), Kind::KnobOn, 0.);
                let ring = self.add(Some(switch), Kind::Ring(Ring::Full(SWITCH.1 / 2.)), 4.);
                self.row_nodes.push(RowNodes {
                    copy,
                    switch,
                    track_off,
                    track_on,
                    knob,
                    knob_on,
                    ring,
                });
            }
        }
        self.thumb = self.add(Some(self.viewport), Kind::Thumb, 0.);
        self.sync_state();
    }

    /// Static opacities, offsets and visibility that follow from state.
    fn sync_state(&mut self) {
        for index in 0..SECTIONS.len() {
            let (_, lit, ring) = self.nav_items[index];
            self.nodes[lit].opacity = self.nav_lit(index);
            self.nodes[ring].visible = self.ring_shown(Target::Nav(index));
        }
        self.nodes[self.history.1].opacity = f32::from(self.hover == Some(Target::History));
        self.nodes[self.history.2].visible = self.ring_shown(Target::History);
        for (row, nodes) in self.row_nodes.iter().enumerate() {
            let state = &self.rows[row];
            let on = f32::from(state.on);
            self.nodes[nodes.track_on].opacity = on;
            self.nodes[nodes.knob_on].opacity = on;
            self.nodes[nodes.knob].rect.x = 3. + KNOB_TRAVEL * on;
            self.nodes[nodes.switch].opacity = if state.enabled { 1. } else { 0.4 };
            self.nodes[nodes.copy].opacity = if state.enabled { 1. } else { 0.55 };
            self.nodes[nodes.ring].visible = self.ring_shown(Target::Row(row));
        }
        for node in &mut self.nodes {
            node.props = true;
        }
    }

    fn nav_lit(&self, index: usize) -> f32 {
        f32::from(index == self.active || self.hover == Some(Target::Nav(index)))
    }

    fn ring_shown(&self, target: Target) -> bool {
        self.focus_visible && self.focus == Some(target)
    }

    // ---- layout ----

    /// Lay out every node for a `w`×`h` DIP client area.
    pub fn layout(&mut self, g: &Gfx, w: f32, h: f32) -> Result<()> {
        self.w = w;
        self.h = h;
        // Layout snaps running motions to their targets.
        self.motions.clear();
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
        for (index, &(base, lit, ring)) in self.nav_items.iter().enumerate() {
            let r = Rect::new(s5, list_y + (item_h + 1.) * index as f32, list_w, item_h);
            self.nodes[base].rect = r;
            self.nodes[lit].rect = Rect::new(0., 0., r.w, r.h);
            self.nodes[ring].rect = Rect::new(0., 0., r.w, r.h);
        }

        // `.preferences-header`: 16/24 padding, min-height 64, 1 px rule.
        let body_w = (w - NAV_WIDTH).max(0.);
        let title = g.text(t, Style::Title, preferences::TITLE, 1e4)?;
        let subtitle = g.text(t, Style::Subtitle, preferences::SUBTITLE, 1e4)?;
        let history = g.text(t, Style::Button, preferences::HISTORY_ACTION, 1e4)?;
        let block = title.height + 2. + subtitle.height;
        let header_h = (s6 + block.max(item_h) + s6 + 1.).max(64.);
        self.title_y = (header_h - 1. - block) / 2.;
        self.header_h = header_h;
        self.nodes[self.header].rect = Rect::new(NAV_WIDTH, 0., body_w, header_h);
        let button_w = (history.width + 2. * s5 + 2.).ceil();
        let button = Rect::new(
            body_w - s8 - button_w,
            ((header_h - 1. - item_h) / 2.).round(),
            button_w,
            item_h,
        );
        self.nodes[self.history.0].rect = button;
        self.nodes[self.history.1].rect = Rect::new(0., 0., button.w, button.h);
        self.nodes[self.history.2].rect = Rect::new(0., 0., button.w, button.h);
        self.texts.title = Some(title);
        self.texts.subtitle = Some(subtitle);
        self.texts.history = Some(history);

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
        // Force the offsets and thumb to be recomputed for the new layout.
        self.scroll = -1.;
        self.set_scroll(scroll, false);
        for node in &mut self.nodes {
            node.dirty = true;
            node.props = true;
        }
        self.sync_state();
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
        let measure = g.ch(t, Style::CardDescription)?;
        self.card_rects.clear();
        self.separators.clear();
        self.texts.card_titles.clear();
        self.texts.card_descriptions.clear();
        self.texts.row_titles.clear();
        self.texts.row_details.clear();
        self.row_rects = vec![Rect::default(); self.rows.len()];
        let mut row_layouts = Vec::new();
        let mut y = s8;
        for (section, rows) in CARDS.iter() {
            let section = &SECTIONS[*section];
            let title = g.text(t, Style::CardTitle, section.title, inner_w)?;
            let description = g.text(
                t,
                Style::CardDescription,
                section.description,
                inner_w.min(52. * measure),
            )?;
            let inset = s6 + 1.;
            let mut cy = inset + title.height + s2 + description.height;
            let mut separators = Vec::new();
            for (position, &row) in rows.iter().enumerate() {
                cy += s5;
                let top = cy;
                // `.settings-card > * + *` rules; the first row after the header
                // has only `--s-1` of padding.
                let pad = if position == 0 {
                    s1
                } else {
                    separators.push(top);
                    1. + s5
                };
                let copy_w = (inner_w - SWITCH.0 - s6).max(40.);
                let state = &self.rows[row];
                let row_title = g.text(t, Style::RowTitle, state.title, copy_w)?;
                let detail = if state.detail.is_empty() {
                    None
                } else {
                    Some(g.text(t, Style::RowDescription, &state.detail, copy_w)?)
                };
                // `.check-row small` adds 3 px to the span's 3 px grid gap.
                let copy_h = row_title.height + detail.as_ref().map_or(0., |d| 6. + d.height);
                let content_h = copy_h.max(SWITCH.1);
                row_layouts.push((row, top, pad, copy_w, copy_h, content_h));
                self.row_rects[row] = Rect::new(s8 + inset, y + top, inner_w, pad + content_h);
                self.texts.row_titles.push(row_title);
                self.texts.row_details.push(detail);
                cy = top + pad + content_h;
            }
            let card_h = cy + inset;
            self.card_rects.push(Rect::new(s8, y, card_w, card_h));
            self.separators.push(separators);
            self.texts.card_titles.push(title);
            self.texts.card_descriptions.push(description);
            y += card_h + s6;
        }
        self.content_h = y - s6 + s12;
        // Texts were pushed in row order because CARDS lists rows in order.
        for (row, top, pad, copy_w, copy_h, content_h) in row_layouts {
            let nodes = &self.row_nodes[row];
            let inset = s6 + 1.;
            self.nodes[nodes.copy].rect =
                Rect::new(inset, top + pad + (content_h - copy_h) / 2., copy_w, copy_h);
            self.nodes[nodes.switch].rect = Rect::new(
                inset + inner_w - SWITCH.0,
                (top + pad + (content_h - SWITCH.1) / 2.).round(),
                SWITCH.0,
                SWITCH.1,
            );
            let (track_off, track_on, knob, knob_on, ring) = (
                nodes.track_off,
                nodes.track_on,
                nodes.knob,
                nodes.knob_on,
                nodes.ring,
            );
            let track = Rect::new(0., 0., SWITCH.0, SWITCH.1);
            self.nodes[track_off].rect = track;
            self.nodes[track_on].rect = track;
            self.nodes[ring].rect = track;
            self.nodes[knob].rect = Rect::new(3., 3., KNOB, KNOB);
            let knob_off = self.children[knob][0];
            self.nodes[knob_off].rect = Rect::new(0., 0., KNOB, KNOB);
            self.nodes[knob_on].rect = Rect::new(0., 0., KNOB, KNOB);
        }
        Ok(())
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

    fn thumb_rect(&self, scroll: f32) -> Rect {
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
        let thumb = self.thumb_rect(target);
        self.nodes[self.thumb].rect = Rect::new(thumb.x, thumb.y, thumb.w, thumb.h);
        self.nodes[self.thumb].visible = self.max_scroll() > 0.;
        let from_thumb = self.thumb_rect(from).y;
        self.motions.retain(|m| {
            !(m.prop == Prop::OffsetY && (m.node == self.content || m.node == self.thumb))
        });
        self.nodes[self.content].rect.y = -target;
        self.nodes[self.content].props = true;
        self.nodes[self.thumb].props = true;
        if smooth && !self.reduced_motion && from.is_finite() {
            let start = Instant::now();
            let curve = self.curve("ease-standard");
            let seconds = self.t.n("dur-5") / 1000.;
            for (node, from, to) in [
                (self.content, -from, -target),
                (self.thumb, from_thumb, thumb.y),
            ] {
                self.motions.push(Motion {
                    node,
                    prop: Prop::OffsetY,
                    from,
                    to,
                    start,
                    seconds,
                    curve,
                    applied: false,
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

    /// Animate `prop` of `node` to `to` over the `duration` token.
    fn animate(&mut self, node: usize, prop: Prop, to: f32, duration: &str) {
        let now = Instant::now();
        let current = |n: &Node| match prop {
            Prop::Opacity => n.opacity,
            Prop::OffsetX => n.rect.x,
            Prop::OffsetY => n.rect.y,
        };
        let from = self
            .motions
            .iter()
            .find(|m| m.node == node && m.prop == prop)
            .map_or_else(|| current(&self.nodes[node]), |m| m.value(now));
        self.motions.retain(|m| !(m.node == node && m.prop == prop));
        let target = &mut self.nodes[node];
        match prop {
            Prop::Opacity => target.opacity = to,
            Prop::OffsetX => target.rect.x = to,
            Prop::OffsetY => target.rect.y = to,
        }
        target.props = true;
        if self.reduced_motion || (from - to).abs() < 1e-4 {
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
        });
    }

    // ---- state ----

    fn set_active(&mut self, index: usize) {
        if index == self.active {
            return;
        }
        let old = std::mem::replace(&mut self.active, index);
        for i in [old, index] {
            let (_, lit, _) = self.nav_items[i];
            self.nodes[lit].dirty = true;
            let to = self.nav_lit(i);
            self.animate(lit, Prop::Opacity, to, "dur-2");
        }
    }

    pub fn set_hover(&mut self, hover: Option<Target>) {
        if hover == self.hover {
            return;
        }
        let old = std::mem::replace(&mut self.hover, hover);
        for target in [old, hover].into_iter().flatten() {
            match target {
                Target::Nav(i) => {
                    let (_, lit, _) = self.nav_items[i];
                    let to = self.nav_lit(i);
                    self.animate(lit, Prop::Opacity, to, "dur-2");
                }
                Target::History => {
                    let to = f32::from(self.hover == Some(Target::History));
                    self.animate(self.history.1, Prop::Opacity, to, "dur-2");
                }
                // The off track's border lightens; the only hover repaint.
                Target::Row(row) => {
                    if self.rows[row].enabled && !self.rows[row].on {
                        self.nodes[self.row_nodes[row].track_off].dirty = true;
                    }
                }
            }
        }
    }

    pub fn set_focus(&mut self, focus: Option<Target>, visible: bool) {
        if focus == self.focus && visible == self.focus_visible {
            return;
        }
        let old = self.focus;
        self.focus = focus;
        self.focus_visible = visible;
        for target in [old, focus].into_iter().flatten() {
            let ring = self.ring(target);
            self.nodes[ring].visible = self.ring_shown(target);
            self.nodes[ring].props = true;
        }
        if let Some(target) = focus
            && focus != old
        {
            self.events.push(Event::Focus(target));
        }
        if visible && let Some(Target::Row(row)) = focus {
            self.reveal(row);
        }
    }

    fn ring(&self, target: Target) -> usize {
        match target {
            Target::Nav(i) => self.nav_items[i].2,
            Target::History => self.history.2,
            Target::Row(row) => self.row_nodes[row].ring,
        }
    }

    pub fn focusable(&self) -> Vec<Target> {
        let mut list: Vec<Target> = (0..SECTIONS.len()).map(Target::Nav).collect();
        list.push(Target::History);
        for (row, state) in self.rows.iter().enumerate() {
            if state.enabled {
                list.push(Target::Row(row));
            }
        }
        list
    }

    /// Tab order: sidebar, header action, then switches.
    pub fn focus_step(&mut self, forward: bool) {
        let list = self.focusable();
        let index = self.focus.and_then(|f| list.iter().position(|t| *t == f));
        let next = match (index, forward) {
            (None, true) => 0,
            (None, false) => list.len() - 1,
            (Some(i), true) => (i + 1) % list.len(),
            (Some(i), false) => (i + list.len() - 1) % list.len(),
        };
        self.set_focus(Some(list[next]), true);
    }

    /// Keep a focused row inside the viewport.
    fn reveal(&mut self, row: usize) {
        let r = self.row_rects[row];
        let vh = self.viewport_rect().h;
        let margin = self.t.n("s-6");
        if r.y - margin < self.scroll {
            self.set_scroll(r.y - margin, false);
        } else if r.y + r.h + margin > self.scroll + vh {
            self.set_scroll(r.y + r.h + margin - vh, false);
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
        }
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
                match captures_settings::save(&self.settings_file, &self.settings) {
                    Ok(saved) => self.settings = saved,
                    Err(error) => eprintln!("{}", preferences::save_error(&error.to_string())),
                }
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

    pub fn settings_file(&self) -> &std::path::Path {
        &self.settings_file
    }

    // ---- geometry for input and accessibility (client DIPs) ----

    pub fn absolute(&self, node: usize) -> Rect {
        let mut r = self.nodes[node].rect;
        let mut parent = self.nodes[node].parent;
        while let Some(p) = parent {
            let pr = self.nodes[p].rect;
            r = r.offset(pr.x, pr.y);
            parent = self.nodes[p].parent;
        }
        r
    }

    fn content_rect(&self, r: Rect) -> Rect {
        let viewport = self.viewport_rect();
        r.offset(viewport.x, viewport.y - self.scroll_now())
    }

    pub fn target_rect(&self, target: Target) -> Rect {
        match target {
            Target::Nav(i) => self.absolute(self.nav_items[i].0),
            Target::History => self.absolute(self.history.0),
            Target::Row(row) => self.absolute(self.row_nodes[row].switch),
        }
    }

    pub fn row_rect(&self, row: usize) -> Rect {
        self.content_rect(self.row_rects[row])
    }

    pub fn card_rect(&self, card: usize) -> Rect {
        self.content_rect(self.card_rects[card])
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
        for (i, &(base, _, _)) in self.nav_items.iter().enumerate() {
            if self.absolute(base).contains(x, y) {
                return Some(Target::Nav(i));
            }
        }
        if self.absolute(self.history.0).contains(x, y) {
            return Some(Target::History);
        }
        if !self.viewport_rect().contains(x, y) {
            return None;
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
                let y = (h - label.height) / 2.;
                if lit {
                    // Opaque so the resting label never shows through.
                    gfx::fill(dc, all, c("surface-sunken"))?;
                    let bg = if index == self.active {
                        "surface-active"
                    } else {
                        "surface-hover"
                    };
                    gfx::fill_round(dc, all, t.n("r-md"), c(bg))?;
                    gfx::text(dc, label, t.n("s-4"), y, c("text"))?;
                } else {
                    gfx::text(dc, label, t.n("s-4"), y, c("text-subtle"))?;
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
                    let y = (h - label.height) / 2.;
                    gfx::text(dc, label, t.n("s-5") + 1., y, c("text"))?;
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
            Kind::RowCopy(row) => {
                let title = &self.texts.row_titles[row];
                gfx::text(dc, title, 0., 0., c("text"))?;
                if let Some(detail) = &self.texts.row_details[row] {
                    gfx::text(dc, detail, 0., title.height + 6., c("text-subtle"))?;
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
            Kind::Ring(Ring::Tight(radius)) => gfx::ring(dc, all, radius, 0., 2., accent_ring)?,
            Kind::Ring(Ring::Full(radius)) => {
                gfx::ring(dc, all, radius, 0., 2., c("surface-canvas"))?;
                gfx::ring(dc, all, radius, 2., 4., accent_ring)?;
            }
            Kind::Thumb => gfx::fill_round(dc, all, w / 2., c("border-strong"))?,
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
