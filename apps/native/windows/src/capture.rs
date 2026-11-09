//! New Capture's Screenshot mode: shipping's floating command bar
//! (`.recording-selector-panel` in `RecordingSelector`) over the display
//! under the pointer, frozen like shipping when Freeze screen is on.
//!
//! The shared `CaptureFlow` owns the generation and global Escape from the
//! moment the menu opens; `WindowSession` prepares the display, its window
//! list (and frozen pixels) off the UI thread and captures after
//! confirmation. Region drags a rectangle, Window picks the window under the
//! pointer (the desktop means the whole display) and Full screen saves the
//! display. A configured delay shows shipping's ScreenshotCountdown scrim
//! (`.recording-countdown`) on the selected display; Esc cancels it. Record,
//! the Aspect and Display selects and UI Automation are not ported yet.

use std::cell::RefCell;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::{Duration, Instant};

use captures_app::capture_flow::CaptureFlow;
use captures_app::capture_menu::{self, MenuMode, PrimaryState};
use captures_app::selection::Point;
use captures_app::window::{self as window_target, WindowSession};
use captures_capture::LogicalRect;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectComposition::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Gdi::{ScreenToClient, ValidateRect};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

use crate::countdown::{Countdown, Tick};
use crate::gfx::{self, Gfx, Style, rect};
use crate::tokens::{Shadow, Tokens};

/// Shipping `CAPTURE_HUD_HIDE_SETTLE_MS`: the closed menu leaves the
/// compositor before fresh pixels are read.
const SETTLE: Duration = Duration::from_millis(40);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Region,
    Window,
    Display,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Hit {
    Close,
    Screenshot,
    Record,
    Target(Target),
    Primary,
}

type Rect = D2D_RECT_F;

fn contains(r: &Rect, x: f32, y: f32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

fn alpha(mut color: D2D1_COLOR_F, a: f32) -> D2D1_COLOR_F {
    color.a *= a;
    color
}

/// `rgba(4, 5, 8, a)`: shipping's capture shade.
fn shade(a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: 4. / 255.,
        g: 5. / 255.,
        b: 8. / 255.,
        a,
    }
}

struct Segment {
    rect: Rect,
    label: gfx::Text,
    icon: Option<&'static str>,
    hit: Hit,
    enabled: bool,
    active: bool,
}

struct Layout {
    panel: Rect,
    close: Rect,
    switches: [Rect; 2],
    divider: Rect,
    segments: Vec<Segment>,
    primary: Option<(Rect, gfx::Text, bool)>,
    note: Vec<(gfx::Text, f32, bool)>,
    note_y: f32,
}

/// The menu's state and drawing, shared by the live window and the
/// offscreen `--screenshot` renderer. Coordinates are DIPs from the
/// display's top-left corner.
pub struct Menu {
    pub target: Target,
    pub size: (f32, f32),
    pub selection: Option<Rect>,
    drag: Option<(f32, f32)>,
    hover: Option<Hit>,
    pressed: Option<Hit>,
    display: (String, u32, u32),
    auto_start: bool,
    layout: Option<Layout>,
    /// Window targets in DIPs from the display's corner, with their labels.
    pub windows: Vec<(Rect, String)>,
    /// The window under the pointer; `Some(None)` is the display.
    pub hovered: Option<Option<usize>>,
    /// The clicked window target, kept when the pointer leaves it.
    pub picked: Option<Option<usize>>,
}

impl Menu {
    pub fn new(
        target: Target,
        size: (f32, f32),
        display: (String, u32, u32),
        auto_start: bool,
    ) -> Self {
        Self {
            target,
            size,
            selection: None,
            drag: None,
            hover: None,
            pressed: None,
            display,
            auto_start,
            layout: None,
            windows: Vec::new(),
            hovered: None,
            picked: None,
        }
    }

    /// The window target under the pointer; true when it changed.
    fn hover_window(&mut self, hit: Option<Option<usize>>) -> bool {
        std::mem::replace(&mut self.hovered, hit) != hit
    }

    /// A drawn region of at least one DIP each way, or Full screen.
    fn can_start(&self) -> bool {
        match self.target {
            Target::Display => true,
            Target::Region => self
                .selection
                .is_some_and(|r| r.right - r.left >= 1. && r.bottom - r.top >= 1.),
            Target::Window => self.picked.is_some(),
        }
    }

    fn layout(&self, g: &Gfx, t: &Tokens) -> Result<Layout> {
        const PAD: f32 = 8.;
        const GAP: f32 = 8.;
        let label = |text: &str| g.text(t, Style::Segment, text, 10_000.);
        // `.capture-action-switch button`: 12 + icon 15 + 6 + label + 12.
        let width = |text: &gfx::Text| 12. + 15. + 6. + text.width + 12.;
        let action = [
            (
                label("Screenshot")?,
                Some("capture"),
                Hit::Screenshot,
                true,
                true,
            ),
            (label("Record")?, None, Hit::Record, false, false),
        ];
        let mut targets = Vec::new();
        for (target, text, icon) in [
            (Target::Region, "Region", "target-region"),
            (Target::Window, "Window", "target-window"),
            (Target::Display, "Full screen", "target-display"),
        ] {
            targets.push((
                label(text)?,
                Some(icon),
                Hit::Target(target),
                true,
                target == self.target,
            ));
        }
        let box_width = |items: &[(gfx::Text, Option<&str>, Hit, bool, bool)]| {
            8. + items.iter().map(|i| width(&i.0)).sum::<f32>()
        };
        let (aw, tw) = (box_width(&action), box_width(&targets));
        let primary = capture_menu::primary_action(
            MenuMode::Screenshot,
            self.auto_start,
            PrimaryState::default(),
        );
        let primary = if primary.hidden {
            None
        } else {
            let text = g.text(t, Style::Action, primary.label, 10_000.)?;
            let w = (16. + 16. + 6. + text.width + 16.).max(112.);
            Some((w, text))
        };
        let row = PAD
            + 32.
            + GAP
            + aw
            + GAP
            + 1.
            + GAP
            + tw
            + primary.as_ref().map_or(0., |(w, _)| GAP + 4. + w)
            + PAD;
        // `These controls **won’t** show in screenshots · Press Enter…`: the
        // menu closes before every capture, so it never shows in one.
        let visibility = capture_menu::visibility_note(MenuMode::Screenshot, true, true);
        let confirm = capture_menu::confirm_note(self.auto_start);
        let note_text = |text: &str, strong| {
            g.text(
                t,
                if strong {
                    Style::NoteStrong
                } else {
                    Style::Status
                },
                text,
                10_000.,
            )
        };
        // DirectWrite widths drop trailing spaces; add the space back.
        let space = note_text("a a", false)?.width - 2. * note_text("a", false)?.width;
        let pieces = [
            (visibility.lead.trim_end(), false, space),
            (visibility.emphasis, true, 0.),
            (visibility.trail.as_str(), false, 4.),
            (capture_menu::NOTE_SEPARATOR, false, 4.),
            (confirm.text, false, 0.),
        ];
        let mut note = Vec::new();
        let mut x = 0.;
        for (text, strong, after) in pieces {
            let laid = note_text(text, strong)?;
            let w = laid.width;
            note.push((laid, x, strong));
            x += w + after;
        }
        let note_w = x;
        let pw = row.max(note_w + 24.) + 2.;
        let ph = 1. + 56. + 28. + 1.;
        let (w, h) = self.size;
        let left = ((w - pw) / 2.).round();
        let top = (h - 26. - ph).round();
        let panel = rect(left, top, left + pw, top + ph);
        let cy = top + 1. + 28.;
        let mut x = left + 1. + PAD;
        let close = rect(x, cy - 16., x + 32., cy + 16.);
        x += 32. + GAP;
        let mut segments = Vec::new();
        let mut place =
            |x0: f32, items: Vec<(gfx::Text, Option<&'static str>, Hit, bool, bool)>| {
                let mut sx = x0 + 4.;
                for (label, icon, hit, enabled, active) in items {
                    let sw = width(&label);
                    segments.push(Segment {
                        rect: rect(sx, cy - 14., sx + sw, cy + 14.),
                        label,
                        icon,
                        hit,
                        enabled,
                        active,
                    });
                    sx += sw;
                }
            };
        let action_box = rect(x, cy - 18., x + aw, cy + 18.);
        place(x, action.into_iter().collect());
        x += aw + GAP;
        let divider = rect(x, cy - 12., x + 1., cy + 12.);
        x += 1. + GAP;
        let target_box = rect(x, cy - 18., x + tw, cy + 18.);
        place(x, targets);
        x += tw;
        let primary = primary.map(|(w, text)| {
            let px = x + GAP + 4.;
            (rect(px, cy - 20., px + w, cy + 20.), text, self.can_start())
        });
        Ok(Layout {
            panel,
            close,
            switches: [action_box, target_box],
            divider,
            segments,
            primary,
            note: note
                .into_iter()
                .map(|(text, nx, strong)| (text, left + (pw - note_w) / 2. + nx, strong))
                .collect(),
            note_y: top + 1. + 56. + 10.,
        })
    }

    fn hit(&self, x: f32, y: f32) -> Option<Hit> {
        let layout = self.layout.as_ref()?;
        if contains(&layout.close, x, y) {
            return Some(Hit::Close);
        }
        if let Some((r, _, enabled)) = &layout.primary
            && *enabled
            && contains(r, x, y)
        {
            return Some(Hit::Primary);
        }
        layout
            .segments
            .iter()
            .find(|s| s.enabled && contains(&s.rect, x, y))
            .map(|s| s.hit)
    }

    fn over_panel(&self, x: f32, y: f32) -> bool {
        self.layout
            .as_ref()
            .is_some_and(|l| contains(&l.panel, x, y))
    }

    pub fn paint(
        &mut self,
        dc: &ID2D1DeviceContext,
        g: &Gfx,
        t: &Tokens,
        frozen: Option<&ID2D1Bitmap1>,
    ) -> Result<()> {
        let layout = self.layout(g, t)?;
        let (w, h) = self.size;
        let full = rect(0., 0., w, h);
        if let Some(bitmap) = frozen {
            unsafe {
                dc.DrawBitmap(
                    bitmap,
                    Some(&full),
                    1.,
                    D2D1_INTERPOLATION_MODE_LINEAR,
                    None,
                    None,
                )
            };
        }
        let glass_text = t.color("glass-text");
        let accent = t.color("theme-accent");
        match self.target {
            // `.capture-shade-full`; once drawn, the cutout shade is hidden
            // until keyboard focus and only the frame remains.
            Target::Region => match self.selection {
                None => gfx::fill(dc, full, shade(0.38))?,
                Some(r) => {
                    gfx::ring(dc, r, 2., 0., 1., t.color("selection-hairline-outer"))?;
                    gfx::border(dc, r, 2., 1.5, accent)?;
                }
            },
            // `.recording-target-window`: clear until a target is hovered; the
            // clicked one stays when the pointer leaves.
            // `.recording-target-window`: clear until a target is hovered. The
            // shade cuts out the hovered (else picked) window; like shipping's
            // `.hovered` and `.selected` classes, both the hovered and the
            // picked window keep their tint, ring and name chip.
            Target::Window => {
                let picked = self.picked.flatten();
                let clip = |index: usize| {
                    self.windows.get(index).map(|(r, label)| {
                        let r = rect(
                            r.left.max(0.),
                            r.top.max(0.),
                            r.right.min(w),
                            r.bottom.min(h),
                        );
                        (r, label.as_str())
                    })
                };
                if let Some((r, _)) = self.hovered.or(self.picked).flatten().and_then(clip) {
                    for band in [
                        rect(0., 0., w, r.top),
                        rect(0., r.bottom, w, h),
                        rect(0., r.top, r.left, r.bottom),
                        rect(r.right, r.top, w, r.bottom),
                    ] {
                        if band.right > band.left && band.bottom > band.top {
                            gfx::fill(dc, band, shade(0.3))?;
                        }
                    }
                }
                let hovered_window = self.hovered.flatten();
                let mut marked: Vec<usize> = picked.into_iter().collect();
                marked.extend(hovered_window.filter(|&i| Some(i) != picked));
                for (r, label) in marked.into_iter().filter_map(clip) {
                    gfx::fill(dc, r, alpha(accent, 0.14))?;
                    // An outer 2 px ring, never over the window.
                    gfx::ring(dc, r, 0., 0., 2., accent)?;
                    unsafe { dc.PushAxisAlignedClip(&r, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE) };
                    let chip = self.chip(
                        dc,
                        g,
                        t,
                        (r.left + 8., r.top + 8.),
                        label,
                        r.right - r.left - 8.,
                    );
                    unsafe { dc.PopAxisAlignedClip() };
                    chip?;
                }
                // The desktop outline: hovered without a picked window, or picked.
                let desktop = match (self.hovered, self.picked) {
                    (Some(None), Some(Some(_))) => false,
                    (Some(None), _) => true,
                    (None, Some(None)) => true,
                    _ => false,
                };
                if desktop {
                    gfx::border(dc, full, 0., 2., accent)?;
                    self.chip(
                        dc,
                        g,
                        t,
                        (16., 16.),
                        capture_menu::DISPLAY_TARGET,
                        360_f32.min(w - 8.),
                    )?;
                }
            }
            Target::Display => {
                gfx::fill(dc, full, shade(0.5))?;
                gfx::border(dc, full, 0., 2., accent)?;
                self.identity(dc, g, t)?;
            }
        }
        // `.recording-selector-panel`
        gfx::shadow(dc, layout.panel, 18., t.shadow("glass-shadow"))?;
        gfx::fill_round(dc, layout.panel, 18., t.color("glass-strong"))?;
        gfx::border(dc, layout.panel, 18., 1., t.color("glass-border"))?;
        let hovered = |hit| self.hover == Some(hit);
        // `.capture-selector-close`
        if hovered(Hit::Close) {
            gfx::fill_round(dc, layout.close, 8., t.color("glass-hover"))?;
        }
        let c = &layout.close;
        let close_color = if hovered(Hit::Close) {
            glass_text
        } else {
            t.color("glass-text-subtle")
        };
        gfx::icon(
            dc,
            &g.factory,
            "close",
            &[],
            (c.left + 8.5, c.top + 8.5, 15.),
            2.,
            close_color,
        )?;
        // `.on-media .capture-action-switch` / `.recording-target-switch`
        for r in layout.switches {
            gfx::fill_round(
                dc,
                r,
                10.,
                D2D1_COLOR_F {
                    r: 0.,
                    g: 0.,
                    b: 0.,
                    a: 0.32,
                },
            )?;
        }
        gfx::fill(dc, layout.divider, t.color("glass-border"))?;
        for s in &layout.segments {
            if s.active {
                gfx::shadow(dc, s.rect, 6., t.shadow("shadow-sm"))?;
                gfx::fill_round(dc, s.rect, 6., t.color("glass-raised"))?;
                gfx::border(dc, s.rect, 6., 1., t.color("glass-border"))?;
            }
            let base = if s.active || (s.enabled && hovered(s.hit)) {
                glass_text
            } else {
                t.color("glass-text-muted")
            };
            let color = if s.enabled { base } else { alpha(base, 0.35) };
            let cy = (s.rect.top + s.rect.bottom) / 2.;
            let x = s.rect.left + 12.;
            match s.icon {
                Some(icon) => gfx::icon(dc, &g.factory, icon, &[], (x, cy - 7.5, 15.), 1.7, color)?,
                // `.capture-record-dot`
                None => gfx::fill_round(
                    dc,
                    rect(x + 3., cy - 4.5, x + 12., cy + 4.5),
                    4.5,
                    alpha(t.color("theme-signal"), if s.enabled { 1. } else { 0.35 }),
                )?,
            }
            gfx::text(dc, &s.label, x + 21., cy - s.label.height / 2., color)?;
        }
        // `.recording-start`
        if let Some((r, label, enabled)) = &layout.primary {
            let opacity = if *enabled { 1. } else { 0.4 };
            let lift = if *enabled && hovered(Hit::Primary) {
                -1.
            } else {
                0.
            };
            let r = rect(r.left, r.top + lift, r.right, r.bottom + lift);
            let shadow = [Shadow {
                x: 0.,
                y: 6.,
                blur: 18.,
                spread: 0.,
                color: [0., 0., 0., 0.3 * opacity],
            }];
            gfx::shadow(dc, r, 10., &shadow)?;
            let fill = if *enabled && hovered(Hit::Primary) {
                "theme-accent-hover"
            } else {
                "theme-accent"
            };
            gfx::fill_round(dc, r, 10., t.color_alpha(fill, opacity))?;
            let ink = t.color_alpha("theme-accent-ink", opacity);
            let content = 16. + 6. + label.width;
            let x = (r.left + r.right - content) / 2.;
            let cy = (r.top + r.bottom) / 2.;
            gfx::icon(dc, &g.factory, "capture", &[], (x, cy - 8., 16.), 1.8, ink)?;
            gfx::text(dc, label, x + 22., cy - label.height / 2., ink)?;
        }
        // `.capture-selector-note`
        for (text, x, strong) in &layout.note {
            let color = if *strong {
                glass_text
            } else {
                t.color("glass-text-subtle")
            };
            gfx::text(dc, text, *x, layout.note_y, color)?;
        }
        self.layout = Some(layout);
        Ok(())
    }

    /// `.recording-display-identity`: the icon, display name and size,
    /// centered and lifted by 60% of its height.
    /// A glass name chip: `--s-3`/`--s-2` padding, `--text-xs` medium,
    /// ellipsized within `max_width`.
    fn chip(
        &self,
        dc: &ID2D1DeviceContext,
        g: &Gfx,
        t: &Tokens,
        (x, y): (f32, f32),
        label: &str,
        max_width: f32,
    ) -> Result<()> {
        let (px, py) = (12., 8.);
        let text = g.text(t, Style::Chip, label, 10_000.)?;
        let text_w = text.width.min((max_width - 2. * px).max(0.));
        g.ellipsis(&text, text_w)?;
        let chip = rect(x, y, x + text_w + 2. * px, y + text.height + 2. * py);
        gfx::fill_round(dc, chip, 6., t.color("glass-strong"))?;
        gfx::text(dc, &text, x + px, y + py, t.color("glass-text"))
    }

    fn identity(&self, dc: &ID2D1DeviceContext, g: &Gfx, t: &Tokens) -> Result<()> {
        let (name, pw, ph) = &self.display;
        let identity = capture_menu::display_identity(name, *pw, *ph, None);
        let name = g.text(t, Style::RecordingTitle, &identity.name, 10_000.)?;
        let detail = g.text(t, Style::Body, &identity.detail, 10_000.)?;
        let total = 50. + 8. + name.height + 8. + detail.height;
        let (w, h) = self.size;
        let mut y = h / 2. - total * 0.6;
        let icon = rect(w / 2. - 34., y, w / 2. + 34., y + 50.);
        gfx::shadow(dc, icon, 14., t.shadow("glass-shadow"))?;
        gfx::fill_round(dc, icon, 14., t.color("glass"))?;
        gfx::border(dc, icon, 14., 1., t.color("glass-border-strong"))?;
        gfx::icon(
            dc,
            &g.factory,
            "target-display",
            &[],
            (w / 2. - 17., y + 8., 34.),
            1.4,
            t.color("glass-text"),
        )?;
        y += 58.;
        // `.recording-display-identity` text shadows: 0 2px 12px / 10px at 0.5.
        let (nx, dx) = ((w - name.width) / 2., (w - detail.width) / 2.);
        gfx::text_shadow(dc, &name, nx, y + 2., 12., 0.5)?;
        gfx::text(dc, &name, nx, y, t.color("glass-text"))?;
        y += name.height + 8.;
        gfx::text_shadow(dc, &detail, dx, y + 2., 10., 0.5)?;
        gfx::text(dc, &detail, dx, y, t.color("glass-text-muted"))
    }

    /// Pointer motion; true when the menu needs a repaint.
    fn moved(&mut self, x: f32, y: f32) -> bool {
        if let Some((sx, sy)) = self.drag {
            let (w, h) = self.size;
            let (x, y) = (x.clamp(0., w), y.clamp(0., h));
            self.selection = Some(rect(sx.min(x), sy.min(y), sx.max(x), sy.max(y)));
            return true;
        }
        let hover = self.hit(x, y);
        std::mem::replace(&mut self.hover, hover) != hover
    }

    fn pressed(&mut self, x: f32, y: f32) -> bool {
        self.pressed = self.hit(x, y);
        if self.pressed.is_none() && self.target == Target::Region && !self.over_panel(x, y) {
            self.drag = Some((x, y));
            self.selection = None;
            return true;
        }
        false
    }

    /// A click outside the bar in Window mode picks the hovered target;
    /// auto-start then confirms it.
    fn pick(&mut self) -> Option<Hit> {
        let hovered = self.hovered?;
        self.picked = Some(hovered);
        self.auto_start.then_some(Hit::Primary)
    }

    /// Release; returns the action to run.
    fn released(&mut self, x: f32, y: f32) -> Option<Hit> {
        if self.drag.take().is_some() {
            if !self.can_start() {
                self.selection = None;
            }
            return (self.auto_start && self.can_start()).then_some(Hit::Primary);
        }
        let hit = self.hit(x, y);
        (self.pressed.take() == hit).then_some(hit).flatten()
    }
}

/// What confirmation captures.
enum Choice {
    Session(window_target::Target),
    Display,
}

type CaptureResult = std::result::Result<Option<captures_app::Artifact>, String>;

enum Phase {
    Preparing(Receiver<std::result::Result<Option<WindowSession>, String>>),
    Selecting(Box<Overlay>, Box<WindowSession>),
    Counting(Box<Counting>),
    Capturing(Receiver<CaptureResult>),
    Done(Box<CaptureResult>),
}

struct Capture {
    flow: CaptureFlow,
    target: Target,
    settings: captures_settings::AppSettings,
    tokens: Tokens,
    root: std::path::PathBuf,
    wake: std::sync::Arc<dyn Fn() + Send + Sync>,
    phase: Phase,
}

struct Overlay {
    hwnd: HWND,
    menu: Menu,
    g: Gfx,
    scale: f32,
    px: (u32, u32),
    frozen: Option<ID2D1Bitmap1>,
    _target: IDCompositionTarget,
    _visual: IDCompositionVisual2,
    surface: IDCompositionSurface,
}

/// The confirmed choice waiting out the screenshot delay behind
/// `.recording-countdown` on the selected display.
struct Counting {
    countdown: Countdown,
    /// Taken when the deadline passes.
    next: Option<(Choice, Box<WindowSession>)>,
}

impl Drop for Overlay {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

thread_local! {
    static CAPTURE: RefCell<Option<Capture>> = const { RefCell::new(None) };
    static REGISTERED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub fn active() -> bool {
    CAPTURE.with(|c| c.try_borrow().map_or(true, |c| c.is_some()))
}

/// Open New Capture on `target` over the display under the pointer.
/// `from_menu` is true when a tray menu click opened it, so its fade must
/// leave the screen first. `wake` runs on worker threads when the host loop
/// has work.
pub fn open(
    options: &crate::Options,
    target: Target,
    from_menu: bool,
    wake: impl Fn() + Send + Sync + Clone + 'static,
) -> std::result::Result<(), String> {
    if active() {
        // Shipping ignores a second request (`CaptureInProgress`); bring the
        // open controls forward so Enter and Esc keep reaching them.
        focus_overlay();
        return Ok(());
    }
    let settings = captures_settings::load(&options.settings_file).unwrap_or_default();
    let flow = CaptureFlow::begin(0)?;
    let generation = flow.generation();
    let (freeze, include_cursor) = (settings.freeze_screen, settings.show_cursor_in_screenshots);
    let (sender, prepared) = channel();
    let worker_wake = wake.clone();
    std::thread::spawn(move || {
        wait_for_shell_menus(from_menu);
        let result = pointer_display().and_then(|display| {
            match WindowSession::prepare(&display, generation, freeze, include_cursor, 0.) {
                Ok(session) => Ok(Some(session)),
                Err(captures_app::Error::Cancelled) => Ok(None),
                Err(error) => Err(error.to_string()),
            }
        });
        let _ = sender.send(result);
        worker_wake();
    });
    let tokens = crate::appearance::tokens(options.appearance.as_deref(), &settings);
    let root = options
        .history_root
        .clone()
        .unwrap_or_else(captures_app::default_history_root);
    CAPTURE.with(|c| {
        *c.borrow_mut() = Some(Capture {
            flow,
            target,
            settings,
            tokens,
            root,
            wake: std::sync::Arc::new(wake),
            phase: Phase::Preparing(prepared),
        })
    });
    Ok(())
}

fn focus_overlay() {
    CAPTURE.with(|c| {
        if let Ok(c) = c.try_borrow()
            && let Some(Capture {
                phase: Phase::Selecting(overlay, _),
                ..
            }) = c.as_ref()
        {
            unsafe {
                let _ = SetForegroundWindow(overlay.hwnd);
            }
        }
    });
}

/// The tray menu that opened New Capture fades out after the click, and
/// Windows 11's notification-area overflow flyout closes behind it. Wait for
/// both (bounded like shipping's shell-flyout dismissal) plus the fade, so
/// neither freezes into the snapshot. A shortcut press skips the fade wait
/// unless one of those menus was open. `RegionSession::prepare` then
/// dismisses Start/Search as shipping does.
fn wait_for_shell_menus(from_menu: bool) {
    const CLASSES: [PCWSTR; 3] = [
        w!("#32768"),
        w!("TopLevelWindowForOverflowXamlIsland"),
        w!("NotifyIconOverflowWindow"),
    ];
    let visible = || {
        CLASSES.iter().any(|class| {
            let mut after = None;
            loop {
                let Ok(hwnd) = (unsafe { FindWindowExW(None, after, *class, None) }) else {
                    return false;
                };
                if hwnd.is_invalid() {
                    return false;
                }
                if unsafe { IsWindowVisible(hwnd) }.as_bool() && !cloaked(hwnd) {
                    return true;
                }
                after = Some(hwnd);
            }
        })
    };
    let started = Instant::now();
    let open = visible();
    while visible() && started.elapsed() < Duration::from_millis(750) {
        std::thread::sleep(Duration::from_millis(16));
    }
    // Menu selection fade (`SPI_GETSELECTIONFADE`) and flyout close animations
    // outlive the window's visibility.
    if from_menu || open {
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn cloaked(hwnd: HWND) -> bool {
    use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
    let mut cloaked = 0u32;
    unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            (&raw mut cloaked).cast(),
            size_of::<u32>() as u32,
        )
    }
    .is_ok()
        && cloaked != 0
}

/// Advance the capture; `Some` with its result once it ends. Call from the
/// host loop outside any window borrow.
pub fn service() -> Option<CaptureResult> {
    CAPTURE.with(|cell| {
        let mut slot = cell.try_borrow_mut().ok()?;
        let capture = slot.as_mut()?;
        let finished = match &mut capture.phase {
            Phase::Preparing(prepared) => match prepared.try_recv() {
                Ok(Ok(Some(session))) => {
                    if let Err(error) = capture.show(session) {
                        capture.flow.cancel();
                        Some(Err(error.message().to_string()))
                    } else {
                        None
                    }
                }
                Ok(Ok(None)) => Some(Ok(None)),
                Ok(Err(error)) => Some(Err(error)),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some(Err("The capture worker stopped".into())),
            },
            // Global Escape or a session lock cancelled the generation.
            Phase::Selecting(..) => (!capture.flow.is_current()).then_some(Ok(None)),
            // The countdown window's timer advances it.
            Phase::Counting(..) => None,
            Phase::Capturing(results) => match results.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some(Err("The capture worker stopped".into())),
            },
            Phase::Done(result) => Some(std::mem::replace(&mut **result, Ok(None))),
        };
        if finished.is_some() {
            *slot = None;
        }
        finished
    })
}

impl Capture {
    fn show(&mut self, session: WindowSession) -> Result<()> {
        let display = session.display().clone();
        let g = match crate::host::with_host(|h| h.g.clone()) {
            Some(g) => g,
            None => Gfx::new(&self.tokens, true)?,
        };
        let dcomp = g.dcomp.clone().ok_or_else(Error::empty)?;
        unsafe {
            let hwnd = popup(&display, w!("CapturesNewCapture"), w!("New Capture"))?;
            let scale = GetDpiForWindow(hwnd).max(96) as f32 / 96.;
            let px = (display.width, display.height);
            let frozen = match session.frozen_preview() {
                Some(image) => Some(bitmap(&g, &image)?),
                None => None,
            };
            let target = dcomp.CreateTargetForHwnd(hwnd, true)?;
            let visual = dcomp.CreateVisual()?;
            let surface = g.surface(px.0, px.1)?;
            visual.SetContent(&surface)?;
            target.SetRoot(&visual)?;
            let mut menu = Menu::new(
                self.target,
                (px.0 as f32 / scale, px.1 as f32 / scale),
                (display.name.clone(), display.width, display.height),
                self.settings.auto_start_on_selection,
            );
            menu.windows = window_rects(&session);
            let mut overlay = Overlay {
                hwnd,
                menu,
                g,
                scale,
                px,
                frozen,
                _target: target,
                _visual: visual,
                surface,
            };
            overlay.render(&self.tokens)?;
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
            let _ = SetFocus(Some(hwnd));
            self.phase = Phase::Selecting(Box::new(overlay), Box::new(session));
        }
        Ok(())
    }

    fn cancel(&mut self) {
        self.flow.cancel();
        self.phase = Phase::Done(Box::new(Ok(None)));
        (self.wake)();
    }

    fn confirm(&mut self) {
        let Phase::Selecting(overlay, _) = &self.phase else {
            return;
        };
        if !overlay.menu.can_start() {
            return;
        }
        let Phase::Selecting(_, session) = &self.phase else {
            return;
        };
        let choice = match (overlay.menu.target, overlay.menu.selection) {
            (Target::Region, Some(r)) => Choice::Session(window_target::Target::Region {
                rect: LogicalRect {
                    x: f64::from(r.left),
                    y: f64::from(r.top),
                    width: f64::from(r.right - r.left),
                    height: f64::from(r.bottom - r.top),
                },
            }),
            (Target::Window, _) => Choice::Session(match overlay.menu.picked.flatten() {
                Some(index) => match session.windows().get(index) {
                    Some(window) => window_target::Target::Window {
                        id: window.id.clone(),
                    },
                    None => return,
                },
                None => window_target::Target::Display,
            }),
            (Target::Display, _) => Choice::Display,
            _ => return,
        };
        let seconds = self.settings.screenshot_countdown_seconds;
        if self.flow.start_countdown(seconds).is_err() {
            return self.cancel();
        }
        // Dropping the overlay closes the menu before the countdown.
        let Phase::Selecting(_, session) =
            std::mem::replace(&mut self.phase, Phase::Done(Box::new(Ok(None))))
        else {
            return;
        };
        if seconds == 0 {
            return self.spawn_capture(choice, session, false);
        }
        match self.count(seconds, choice, session) {
            Ok(counting) => self.phase = Phase::Counting(Box::new(counting)),
            Err(error) => {
                self.flow.cancel();
                self.phase = Phase::Done(Box::new(Err(error.message().to_string())));
                (self.wake)();
            }
        }
    }

    /// Open the countdown scrim over the selected display.
    fn count(&self, seconds: u8, choice: Choice, session: Box<WindowSession>) -> Result<Counting> {
        let display = session.display().clone();
        let g = match crate::host::with_host(|h| h.g.clone()) {
            Some(g) => g,
            None => Gfx::new(&self.tokens, true)?,
        };
        let hwnd = unsafe {
            popup(
                &display,
                w!("CapturesCountdown"),
                w!("Screenshot Countdown"),
            )?
        };
        let countdown = Countdown::open(hwnd, &display, g, &self.tokens, seconds, false)?;
        Ok(Counting {
            countdown,
            next: Some((choice, session)),
        })
    }

    /// A countdown frame: cancel (with the exit motion), tick the number, or
    /// close the scrim and capture once the deadline passes.
    fn tick(&mut self) {
        let Phase::Counting(counting) = &mut self.phase else {
            return;
        };
        match counting
            .countdown
            .tick(&self.tokens, self.flow.is_current())
        {
            Tick::Running => {}
            Tick::Closed => {
                self.phase = Phase::Done(Box::new(Ok(None)));
                (self.wake)();
            }
            Tick::Elapsed => {
                // Dropping the countdown closes the scrim before the capture.
                let Some((choice, session)) = counting.next.take() else {
                    return;
                };
                self.phase = Phase::Done(Box::new(Ok(None)));
                self.spawn_capture(choice, session, true);
            }
        }
    }

    /// Capture `choice` off the UI thread once closed windows have settled.
    fn spawn_capture(&mut self, choice: Choice, session: Box<WindowSession>, after: bool) {
        let (generation, root) = (self.flow.generation(), self.root.clone());
        let include_cursor = self.settings.show_cursor_in_screenshots;
        let (sender, results) = channel();
        let wake = self.wake.clone();
        std::thread::spawn(move || {
            std::thread::sleep(SETTLE);
            let result = match choice {
                Choice::Session(target) => match session.capture(&root, &target, after) {
                    Ok(artifact) => Ok(Some(artifact)),
                    Err(captures_app::Error::Cancelled) => Ok(None),
                    Err(error) => Err(error.to_string()),
                },
                // `persist_capture_still` is Linux-only: Full screen reads
                // fresh pixels after the menu closes, on the selected display.
                Choice::Display => {
                    match captures_app::execute(captures_app::Request::CaptureDisplay {
                        root,
                        display_id: session.display().id.clone(),
                        generation,
                        include_cursor,
                    }) {
                        Ok(captures_app::Response::Captured { artifact }) => Ok(Some(artifact)),
                        Ok(_) => Err("The capture returned no screenshot.".into()),
                        Err(captures_app::Error::Cancelled) => Ok(None),
                        Err(error) => Err(error.to_string()),
                    }
                }
            };
            let _ = sender.send(result);
            wake();
        });
        self.phase = Phase::Capturing(results);
    }

    fn handle(&mut self, hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
        if let Phase::Counting(counting) = &self.phase
            && counting.countdown.hwnd == hwnd
        {
            match msg {
                WM_TIMER => self.tick(),
                WM_PAINT => unsafe {
                    let _ = ValidateRect(Some(hwnd), None);
                },
                WM_ERASEBKGND => return Some(LRESULT(1)),
                WM_KEYDOWN if VIRTUAL_KEY(wp.0 as u16) == VK_ESCAPE => {
                    self.flow.cancel();
                    self.tick();
                }
                _ => return None,
            }
            return Some(LRESULT(0));
        }
        let Phase::Selecting(overlay, session) = &mut self.phase else {
            return None;
        };
        if overlay.hwnd != hwnd {
            return None;
        }
        let point = |lp: LPARAM, scale: f32| {
            (
                (lp.0 & 0xffff) as i16 as f32 / scale,
                ((lp.0 >> 16) & 0xffff) as i16 as f32 / scale,
            )
        };
        let mut action = None;
        match msg {
            WM_PAINT => unsafe {
                let _ = ValidateRect(Some(hwnd), None);
            },
            WM_ERASEBKGND => return Some(LRESULT(1)),
            WM_MOUSEMOVE => {
                let (x, y) = point(lp, overlay.scale);
                let mut changed = overlay.menu.moved(x, y);
                if overlay.menu.target == Target::Window {
                    let hit = (!overlay.menu.over_panel(x, y)).then(|| {
                        session.hit_test(Point {
                            x: f64::from(x),
                            y: f64::from(y),
                        })
                    });
                    changed |= overlay.menu.hover_window(hit);
                }
                if changed {
                    let _ = overlay.render(&self.tokens);
                }
            }
            WM_LBUTTONDOWN => {
                let (x, y) = point(lp, overlay.scale);
                if overlay.menu.pressed(x, y) {
                    unsafe { SetCapture(hwnd) };
                    let _ = overlay.render(&self.tokens);
                }
            }
            WM_LBUTTONUP => {
                let (x, y) = point(lp, overlay.scale);
                let _ = unsafe { ReleaseCapture() };
                let outside = overlay.menu.pressed.is_none() && !overlay.menu.over_panel(x, y);
                action = if overlay.menu.target == Target::Window && outside {
                    overlay.menu.pick()
                } else {
                    overlay.menu.released(x, y)
                };
                let _ = overlay.render(&self.tokens);
            }
            WM_KEYDOWN => match VIRTUAL_KEY(wp.0 as u16) {
                VK_RETURN => action = Some(Hit::Primary),
                VK_ESCAPE => action = Some(Hit::Close),
                _ => {}
            },
            WM_SETCURSOR => {
                if (lp.0 & 0xffff) as u32 != HTCLIENT {
                    return None;
                }
                let mut p = POINT::default();
                let (x, y) = unsafe {
                    let _ = GetCursorPos(&mut p);
                    let _ = ScreenToClient(hwnd, &mut p);
                    (p.x as f32 / overlay.scale, p.y as f32 / overlay.scale)
                };
                let cursor = if overlay.menu.hit(x, y).is_some() {
                    IDC_HAND
                } else if overlay.menu.over_panel(x, y) {
                    IDC_ARROW
                } else if overlay.menu.target == Target::Region {
                    IDC_CROSS
                } else if overlay.menu.target == Target::Window {
                    // `--capture-action-cursor`
                    IDC_HAND
                } else {
                    IDC_ARROW
                };
                unsafe { SetCursor(LoadCursorW(None, cursor).ok()) };
                return Some(LRESULT(1));
            }
            _ => return None,
        }
        match action {
            Some(Hit::Close) => self.cancel(),
            Some(Hit::Primary) => self.confirm(),
            Some(Hit::Target(target)) => {
                overlay.menu.target = target;
                let auto = target == Target::Display && overlay.menu.auto_start;
                let _ = overlay.render(&self.tokens);
                if auto {
                    self.confirm();
                }
            }
            Some(Hit::Screenshot | Hit::Record) | None => {}
        }
        Some(LRESULT(0))
    }
}

impl Overlay {
    fn render(&mut self, t: &Tokens) -> Result<()> {
        let Self {
            menu,
            g,
            frozen,
            surface,
            px,
            scale,
            ..
        } = self;
        gfx::draw_surface(surface, *px, (*scale, *scale), 0., |dc| {
            menu.paint(dc, g, t, frozen.as_ref())
        })?;
        unsafe { g.dcomp.as_ref().ok_or_else(Error::empty)?.Commit() }
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let handled = CAPTURE.with(|c| {
        let mut c = c.try_borrow_mut().ok()?;
        c.as_mut()?.handle(hwnd, msg, wp, lp)
    });
    handled.unwrap_or_else(|| unsafe { DefWindowProcW(hwnd, msg, wp, lp) })
}

/// A topmost tool popup covering `display`, registering both overlay classes
/// on first use.
unsafe fn popup(
    display: &captures_capture::DisplayDescriptor,
    class: PCWSTR,
    title: PCWSTR,
) -> Result<HWND> {
    unsafe {
        let instance = GetModuleHandleW(None)?;
        if !REGISTERED.get() {
            for name in [w!("CapturesNewCapture"), w!("CapturesCountdown")] {
                RegisterClassW(&WNDCLASSW {
                    lpfnWndProc: Some(wndproc),
                    hInstance: instance.into(),
                    lpszClassName: name,
                    ..Default::default()
                });
            }
            REGISTERED.set(true);
        }
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOREDIRECTIONBITMAP,
            class,
            title,
            WS_POPUP,
            display.x,
            display.y,
            display.width as i32,
            display.height as i32,
            None,
            None,
            Some(instance.into()),
            None,
        )
    }
}

/// Straight RGBA as an opaque premultiplied BGRA Direct2D bitmap.
pub fn bitmap(g: &Gfx, image: &image::RgbaImage) -> Result<ID2D1Bitmap1> {
    let (w, h) = image.dimensions();
    let bgra: Vec<u8> = image
        .pixels()
        .flat_map(|p| [p[2], p[1], p[0], 255])
        .collect();
    let props = D2D1_BITMAP_PROPERTIES1 {
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
        },
        dpiX: 96.,
        dpiY: 96.,
        bitmapOptions: D2D1_BITMAP_OPTIONS_NONE,
        colorContext: std::mem::ManuallyDrop::new(None),
    };
    unsafe {
        g.dc.CreateBitmap(
            D2D_SIZE_U {
                width: w,
                height: h,
            },
            Some(bgra.as_ptr().cast()),
            w * 4,
            &props,
        )
    }
}

/// Window targets in display-local DIPs, as `WindowSession::hit_test` reads
/// them, labelled by title, then app name.
fn window_rects(session: &WindowSession) -> Vec<(Rect, String)> {
    let display = session.display();
    let scale = if captures_capture::DisplayDescriptor::reports_physical_geometry() {
        display.scale_factor.max(1.) as f32
    } else {
        1.
    };
    session
        .windows()
        .iter()
        .map(|window| {
            let left = (window.x - display.x) as f32 / scale;
            let top = (window.y - display.y) as f32 / scale;
            let label = [
                Some(window.title.trim()),
                window.app_name.as_deref().map(str::trim),
            ]
            .into_iter()
            .flatten()
            .find(|name| !name.is_empty())
            .unwrap_or("Window")
            .to_owned();
            (
                rect(
                    left,
                    top,
                    left + window.width as f32 / scale,
                    top + window.height as f32 / scale,
                ),
                label,
            )
        })
        .collect()
}

/// Shipping starts on the display under the pointer, else the primary.
fn pointer_display() -> std::result::Result<String, String> {
    let backend = captures_capture::XcapBackend;
    captures_capture::pointer_position()
        .and_then(|point| backend.display_id_at_point(point))
        .or_else(|| {
            let displays = backend.displays().ok()?;
            displays
                .iter()
                .find(|display| display.is_primary)
                .or(displays.first())
                .map(|display| display.id.clone())
        })
        .ok_or_else(|| "No display is available for capture.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn menu(target: Target, auto_start: bool) -> Menu {
        Menu::new(
            target,
            (1280., 800.),
            ("Display".into(), 2560, 1600),
            auto_start,
        )
    }

    #[test]
    fn region_drag_draws_a_normalized_selection_and_click_clears_it() {
        let mut m = menu(Target::Region, false);
        assert!(!m.can_start());
        assert!(m.pressed(400., 300.));
        m.moved(150., 120.);
        assert_eq!(m.released(150., 120.), None);
        let r = m.selection.expect("dragged selection");
        assert_eq!((r.left, r.top, r.right, r.bottom), (150., 120., 400., 300.));
        assert!(m.can_start());
        // A click without movement leaves no zero-size region behind.
        m.pressed(600., 400.);
        m.released(600., 400.);
        assert!(m.selection.is_none());
        assert!(!m.can_start());
    }

    #[test]
    fn auto_start_confirms_a_finished_region_drag_only() {
        let mut m = menu(Target::Region, true);
        m.pressed(10., 10.);
        assert_eq!(m.released(10., 10.), None);
        m.pressed(10., 10.);
        m.moved(90., 70.);
        assert_eq!(m.released(90., 70.), Some(Hit::Primary));
        // Drags clamp to the display.
        let mut m = menu(Target::Region, false);
        m.pressed(1200., 700.);
        m.moved(2000., -50.);
        let r = m.selection.expect("clamped selection");
        assert_eq!((r.left, r.top, r.right, r.bottom), (1200., 0., 1280., 700.));
    }

    #[test]
    fn full_screen_starts_without_a_selection() {
        assert!(menu(Target::Display, false).can_start());
    }

    #[test]
    fn window_click_picks_the_hovered_target_and_keeps_it() {
        let mut m = menu(Target::Window, false);
        assert!(!m.can_start());
        // A click with nothing hovered yet picks nothing.
        assert_eq!(m.pick(), None);
        assert!(!m.can_start());
        m.hover_window(Some(Some(1)));
        assert_eq!(m.pick(), None);
        assert_eq!(m.picked, Some(Some(1)));
        // Leaving the window keeps the pick; the desktop picks the display.
        assert!(m.hover_window(None));
        assert!(m.can_start());
        m.hover_window(Some(None));
        m.pick();
        assert_eq!(m.picked, Some(None));
        assert!(m.can_start());
        // Auto-start confirms the pick at once.
        let mut m = menu(Target::Window, true);
        m.hover_window(Some(Some(0)));
        assert_eq!(m.pick(), Some(Hit::Primary));
    }
}
