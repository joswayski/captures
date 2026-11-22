//! Shipping's "Recording ready" notice (`RecordingSavedNotice`,
//! `.recording-saved-notice`): shown by the resident app when a recording
//! editor closes and the take is still in History. Save file keeps a copy in
//! the Captures folder through the shared `SaveRecording` (then "Recording
//! saved" with Show in Folder, which dismisses it once Explorer opens); an
//! already-saved take opens on "Recording saved". It floats in the primary
//! display's top-right corner on the fixed glass palette, never takes focus,
//! stays out of captures (shipping's content protection), closes when a new
//! capture starts, and plays `recording-saved-lifecycle` over its 15 s life
//! (the shared lifecycle pose, held still under reduced motion), closing
//! 200 ms after it ends. A later notice replaces it.
use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use captures_app::motion::{Animation, Motion, Pose};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectComposition::*;
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromPoint, ValidateRect,
};
use windows::Win32::UI::Accessibility::{UiaReturnRawElementProvider, UiaRootObjectId};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;
use windows_numerics::Matrix3x2;

use crate::Options;
use crate::gfx::{self, Gfx, Style, rect};
use crate::panel_uia;
use crate::tokens::Tokens;

type Rect = D2D_RECT_F;

const CLASS: PCWSTR = w!("CapturesRecordingReady");
const TITLE: &str = "Recording saved";
/// Shipping `RECORDING_SAVED_NOTICE_CARD_WIDTH` × `_HEIGHT`.
const CARD: (f32, f32) = (440., 116.);
/// Shipping `RECORDING_SAVED_NOTICE_FRAME_PAD`: transparent room for the
/// glass shadow.
pub const PAD: f32 = 28.;
pub const SIZE: (f32, f32) = (CARD.0 + PAD * 2., CARD.1 + PAD * 2.);
/// Shipping `RECORDING_SAVED_NOTICE_VISIBLE_FOR`.
const LIFETIME: Duration = Duration::from_millis(15_200);
/// The window closes this long after the 15 s animation ends.
const CLOSE_AFTER_ANIMATION_MS: f64 = 200.;
const FRAME_TIMER: usize = 1;
const HIDE_TIMER: usize = 2;
const ANNOUNCE_TIMER: usize = 3;
/// After the closing editor has handed focus back, so the focus change does
/// not talk over the announcement (as the launch notice).
const ANNOUNCE_DELAY_MS: u32 = 400;
const WM_DONE: u32 = WM_APP + 1;
const WM_MOUSELEAVE: u32 = 0x02A3;

thread_local! {
    static NOTICE: RefCell<Option<Notice>> = const { RefCell::new(None) };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Action,
    Dismiss,
}

/// Shipping's `permanentlySaved`, `busy` and `error` for one take.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    pub saved: Option<String>,
    pub busy: bool,
    pub error: Option<String>,
}

impl State {
    pub fn title(&self) -> &'static str {
        if self.saved.is_some() {
            "Recording saved"
        } else {
            "Recording ready"
        }
    }

    /// `.recording-saved-copy p`.
    pub fn detail(&self) -> &str {
        self.error.as_deref().unwrap_or(if self.saved.is_some() {
            "Saved to your Captures folder."
        } else {
            "Kept in Capture History for 30 days. Save a copy anytime."
        })
    }

    /// `.recording-saved-reveal`: its label and leading icon.
    pub fn action(&self) -> (&'static str, &'static str) {
        match (self.saved.is_some(), self.busy) {
            (true, true) => ("Opening…", "folder"),
            (true, false) => ("Show in Folder", "folder"),
            (false, true) => ("Saving…", "save"),
            (false, false) => ("Save file", "save"),
        }
    }
}

/// Where the parts landed, in window DIPs.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Parts {
    pub card: Rect,
    pub action: Rect,
    pub dismiss: Rect,
}

enum Done {
    Saved(std::result::Result<String, String>),
    Revealed(std::result::Result<(), String>),
}

struct Notice {
    hwnd: HWND,
    g: Gfx,
    t: Tokens,
    scale: f32,
    px: (u32, u32),
    composition: Option<(
        IDCompositionTarget,
        IDCompositionVisual2,
        IDCompositionSurface,
    )>,
    id: String,
    root: PathBuf,
    settings_file: PathBuf,
    state: State,
    shown_at: Instant,
    lifecycle: Option<Animation>,
    hover: Option<Part>,
    tracking: bool,
    parts: Parts,
    sender: Sender<Done>,
    receiver: Receiver<Done>,
}

impl Drop for Notice {
    fn drop(&mut self) {
        unsafe {
            let _ = SetWindowDisplayAffinity(self.hwnd, WDA_NONE);
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

/// A recording editor for `id` closed: show the notice when the take is
/// still in History with its media on disk, like shipping's
/// `show_recording_saved_notice`.
pub fn show(options: &Options, id: &str) {
    let root = options
        .history_root
        .clone()
        .unwrap_or_else(captures_app::default_history_root);
    let Ok(entries) = captures_history::load(&root, chrono::Utc::now()) else {
        return;
    };
    let Some(entry) = entries.into_iter().find(|e| {
        e.id == id
            && e.kind.is_recording()
            && e.recording_media_path(&root).is_some_and(|p| p.is_file())
    }) else {
        return;
    };
    hide();
    let settings = captures_settings::load(&options.settings_file).unwrap_or_default();
    let t = crate::appearance::tokens(options.appearance.as_deref(), &settings);
    let state = State {
        saved: entry.saved_path.clone(),
        ..Default::default()
    };
    if let Err(error) = open(t, root, options.settings_file.clone(), id.into(), state) {
        eprintln!("failed to show the Recording ready notice: {error}");
    }
}

/// Close a visible notice (Dismiss, the timer, or a new capture starting).
pub fn hide() -> bool {
    NOTICE
        .with(|n| n.try_borrow_mut().ok().and_then(|mut n| n.take()))
        .is_some()
}

fn with_notice<R>(f: impl FnOnce(&mut Notice) -> R) -> Option<R> {
    NOTICE.with(|n| n.try_borrow_mut().ok()?.as_mut().map(f))
}

fn open(t: Tokens, root: PathBuf, settings_file: PathBuf, id: String, state: State) -> Result<()> {
    let g = Gfx::shared(&t)?;
    unsafe {
        let instance = windows::Win32::System::LibraryLoader::GetModuleHandleW(None)?;
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: CLASS,
            ..Default::default()
        });
        // Shipping's `top_right_notice_position`: 18 px from the primary
        // monitor's right edge and 30 px below its top.
        let monitor = MonitorFromPoint(POINT::default(), MONITOR_DEFAULTTOPRIMARY);
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let _ = GetMonitorInfoW(monitor, &mut info);
        let (mut dpi, mut unused) = (96, 96);
        let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi, &mut unused);
        let scale = dpi.max(96) as f32 / 96.;
        let px = (
            (SIZE.0 * scale).round() as u32,
            (SIZE.1 * scale).round() as u32,
        );
        let x = info.rcMonitor.right - px.0 as i32 - (18. * scale).round() as i32;
        let y = info.rcMonitor.top + (30. * scale).round() as i32;
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_NOREDIRECTIONBITMAP,
            CLASS,
            &HSTRING::from(TITLE),
            WS_POPUP,
            x,
            y,
            px.0 as i32,
            px.1 as i32,
            None,
            None,
            Some(instance.into()),
            None,
        )?;
        let (sender, receiver) = channel();
        let lifecycle = Motion::RecordingSavedLifecycle.resolve(&t);
        NOTICE.with(|n| {
            *n.borrow_mut() = Some(Notice {
                hwnd,
                g,
                t,
                scale,
                px,
                composition: None,
                id,
                root,
                settings_file,
                state,
                shown_at: Instant::now(),
                lifecycle,
                hover: None,
                tracking: false,
                parts: Parts::default(),
                sender,
                receiver,
            })
        });
        let shown = with_notice(|n| {
            n.render()?;
            let _ = SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            SetTimer(Some(hwnd), HIDE_TIMER, LIFETIME.as_millis() as u32, None);
            SetTimer(Some(hwnd), ANNOUNCE_TIMER, ANNOUNCE_DELAY_MS, None);
            n.schedule();
            Ok::<_, Error>(())
        });
        if let Some(Err(error)) = shown {
            hide();
            return Err(error);
        }
    }
    Ok(())
}

impl Notice {
    /// Milliseconds since it appeared and until the animation must end.
    fn clock(&self) -> (f64, f64) {
        let since = self.shown_at.elapsed().as_secs_f64() * 1000.;
        let until = LIFETIME.as_secs_f64() * 1000. - since - CLOSE_AFTER_ANIMATION_MS;
        (since, until.max(0.))
    }

    fn pose(&self) -> Pose {
        let (since, until) = self.clock();
        self.lifecycle.map_or(Pose::REST, |a| {
            a.lifecycle_pose(since, Some(until), crate::appearance::reduced_motion())
        })
    }

    /// Paint frames while it arrives or leaves; sleep through the steady
    /// middle until the exit begins.
    fn schedule(&self) {
        let Some(a) = self.lifecycle else { return };
        let (since, until) = self.clock();
        let reduced = crate::appearance::reduced_motion();
        let delay = if a.lifecycle_running(since, Some(until), reduced) {
            16
        } else if reduced || until <= 0. {
            unsafe {
                let _ = KillTimer(Some(self.hwnd), FRAME_TIMER);
            }
            return;
        } else {
            (a.lifecycle_exit_in(until).ceil() as u32).max(16)
        };
        unsafe {
            SetTimer(Some(self.hwnd), FRAME_TIMER, delay, None);
        }
    }

    fn render(&mut self) -> Result<()> {
        let dcomp = self.g.dcomp.clone().ok_or_else(Error::empty)?;
        unsafe {
            if self.composition.is_none() {
                let target = dcomp.CreateTargetForHwnd(self.hwnd, true)?;
                let visual = dcomp.CreateVisual()?;
                let surface = self.g.surface(self.px.0, self.px.1)?;
                visual.SetContent(&surface)?;
                target.SetRoot(&visual)?;
                self.composition = Some((target, visual, surface));
            }
        }
        let surface = self.composition.as_ref().map(|c| c.2.clone());
        let surface = surface.ok_or_else(Error::empty)?;
        let pose = self.pose();
        let mut parts = self.parts;
        gfx::draw_surface(&surface, self.px, (self.scale, self.scale), 0., |dc| {
            parts = paint(dc, &self.g, &self.t, &self.state, self.hover, pose)?;
            Ok(())
        })?;
        self.parts = parts;
        unsafe { dcomp.Commit() }
    }

    fn part_at(&self, x: f32, y: f32) -> Option<Part> {
        let inside = |r: Rect| x >= r.left && x < r.right && y >= r.top && y < r.bottom;
        if inside(self.parts.dismiss) {
            Some(Part::Dismiss)
        } else if inside(self.parts.action) {
            Some(Part::Action)
        } else {
            None
        }
    }

    /// Save file or Show in Folder, off the UI thread like shipping's
    /// `save_recording_artifact` / `reveal_recording_artifact`.
    fn act(&mut self) {
        if self.state.busy {
            return;
        }
        self.state.busy = true;
        self.state.error = None;
        let (sender, hwnd) = (self.sender.clone(), self.hwnd.0 as isize);
        let saved = self.state.saved.clone();
        let request = captures_app::Request::SaveRecording {
            root: self.root.clone(),
            id: self.id.clone(),
            directory: PathBuf::from(
                captures_settings::load(&self.settings_file)
                    .unwrap_or_default()
                    .output_directory,
            ),
        };
        std::thread::spawn(move || {
            let done = match saved {
                Some(path) => Done::Revealed(crate::history::reveal(&path)),
                None => Done::Saved(match captures_app::execute(request) {
                    Ok(captures_app::Response::Saved { path, .. }) => {
                        Ok(path.to_string_lossy().into_owned())
                    }
                    Ok(_) => Err("unexpected response".into()),
                    Err(error) => Err(error.to_string()),
                }),
            };
            if sender.send(done).is_ok() {
                unsafe {
                    let _ = PostMessageW(Some(HWND(hwnd as _)), WM_DONE, WPARAM(0), LPARAM(0));
                }
            }
        });
        let _ = self.render();
    }

    /// Apply a finished save or reveal. True when the notice should close
    /// (Show in Folder succeeded).
    fn finish(&mut self) -> bool {
        let mut close = false;
        while let Ok(done) = self.receiver.try_recv() {
            self.state.busy = false;
            match done {
                Done::Saved(Ok(path)) => self.state.saved = Some(path),
                Done::Saved(Err(error)) => {
                    self.state.error = Some(format!("Could not save the recording: {error}"))
                }
                Done::Revealed(Ok(())) => close = true,
                Done::Revealed(Err(error)) => {
                    self.state.error = Some(format!(
                        "Could not show the recording in its folder: {error}"
                    ))
                }
            }
        }
        if !close {
            let _ = self.render();
        }
        close
    }
}

/// The notice at `pose` in window-local DIPs (the card inside its shadow
/// frame); returns where its buttons are at rest.
pub fn paint(
    dc: &ID2D1DeviceContext,
    g: &Gfx,
    t: &Tokens,
    state: &State,
    hover: Option<Part>,
    pose: Pose,
) -> Result<Parts> {
    let n = |name: &str| t.n(name);
    let c = |name: &str| t.color(name);
    let card = rect(PAD, PAD, SIZE.0 - PAD, SIZE.1 - PAD);
    let (cx, cy) = ((card.left + card.right) / 2., (card.top + card.bottom) / 2.);
    if pose.opacity <= 0. {
        return Ok(parts(g, t, state, card));
    }
    let mut base = Matrix3x2::identity();
    let s = pose.scale as f32;
    unsafe {
        dc.GetTransform(&mut base);
        // `transform: translateY() scale()` about the card's center.
        let local = Matrix3x2 {
            M11: s,
            M12: 0.,
            M21: 0.,
            M22: s,
            M31: cx * (1. - s) + pose.translate_x as f32,
            M32: cy * (1. - s) + pose.translate_y as f32,
        };
        dc.SetTransform(&(local * base));
    }
    let faded = pose.opacity < 1.;
    if faded {
        let layer = D2D1_LAYER_PARAMETERS1 {
            contentBounds: rect(-1e6, -1e6, 1e6, 1e6),
            maskTransform: Matrix3x2::identity(),
            opacity: pose.opacity as f32,
            ..Default::default()
        };
        unsafe { dc.PushLayer(&layer, None::<&ID2D1Layer>) };
    }
    let result = (|| -> Result<Parts> {
        let radius = n("r-xl");
        gfx::shadow(dc, card, radius, t.shadow("glass-shadow"))?;
        gfx::fill_round(dc, card, radius, c("glass-strong"))?;
        gfx::border(dc, card, radius, 1., c("glass-border"))?;
        let p = parts(g, t, state, card);

        // `.recording-saved-icon`: a 38 px positive tile, 20 px check.
        let tile_left = card.left + 1. + n("s-5");
        let tile = rect(tile_left, cy - 19., tile_left + 38., cy + 19.);
        gfx::fill_round(dc, tile, n("r-lg"), c("positive"))?;
        gfx::icon(
            dc,
            &g.factory,
            "check",
            &[],
            (tile.left + 9., tile.top + 9., 20.),
            2.2,
            c("positive-ink"),
        )?;

        // `.recording-saved-reveal`, at half opacity while busy.
        let (label, icon) = state.action();
        let alpha = if state.busy { 0.5 } else { 1. };
        let b = p.action;
        let fill = if hover == Some(Part::Action) && !state.busy {
            "glass-active"
        } else {
            "glass-hover"
        };
        gfx::fill_round(dc, b, n("r-md"), t.color_alpha(fill, alpha))?;
        gfx::border(
            dc,
            b,
            n("r-md"),
            1.,
            t.color_alpha("glass-border-strong", alpha),
        )?;
        let text = g.text_sized(t, label, n("text-xs"), "weight-semibold", None)?;
        let ink = t.color_alpha("glass-text", alpha);
        let icon_left = b.left + 1. + n("s-4");
        gfx::icon(
            dc,
            &g.factory,
            icon,
            &[],
            (icon_left, cy - 7., 14.),
            1.8,
            ink,
        )?;
        gfx::text(
            dc,
            &text,
            icon_left + 14. + n("s-3"),
            cy - text.height / 2.,
            ink,
        )?;

        // `.recording-saved-copy`: the title over the wrapped line.
        let left = tile.right + n("s-5");
        let width = (b.left - n("s-5") - left).max(1.);
        let title = g.text_sized(t, state.title(), n("text-md"), "weight-semibold", None)?;
        if title.width > width {
            g.ellipsis(&title, width)?;
        }
        let detail = g.text(t, Style::Note, state.detail(), width)?;
        let room = card.bottom - card.top - 2. - 2. * n("s-5");
        let block = (title.height + 2. + detail.height).min(room);
        let top = cy - block / 2.;
        gfx::text(dc, &title, left, top, c("glass-text"))?;
        unsafe {
            dc.PushAxisAlignedClip(
                &rect(left, top, left + width, top + block),
                D2D1_ANTIALIAS_MODE_ALIASED,
            );
        }
        let drawn = gfx::text(
            dc,
            &detail,
            left,
            top + title.height + 2.,
            c("glass-text-muted"),
        );
        unsafe { dc.PopAxisAlignedClip() };
        drawn?;

        // `.recording-saved-dismiss`: a quiet 24 px ×.
        let d = p.dismiss;
        let over = hover == Some(Part::Dismiss);
        if over {
            gfx::fill_round(dc, d, n("r-sm"), c("glass-hover"))?;
        }
        gfx::icon(
            dc,
            &g.factory,
            "close",
            &[],
            (d.left + 5., d.top + 5., 14.),
            2.,
            c(if over {
                "glass-text"
            } else {
                "glass-text-subtle"
            }),
        )?;
        Ok(p)
    })();
    unsafe {
        if faded {
            dc.PopLayer();
        }
        dc.SetTransform(&base);
    }
    result
}

/// The buttons' rest rects: the action right of the copy, inside the card's
/// 38 px right padding, and the dismiss `top: 10px; right: 10px`.
fn parts(g: &Gfx, t: &Tokens, state: &State, card: Rect) -> Parts {
    let n = |name: &str| t.n(name);
    let label = g
        .text_sized(t, state.action().0, n("text-xs"), "weight-semibold", None)
        .map_or(80., |text| text.width);
    let width = (2. * n("s-4") + 14. + n("s-3") + label + 2.).ceil();
    let (right, cy, h) = (
        card.right - 1. - 38.,
        (card.top + card.bottom) / 2.,
        n("h-md"),
    );
    let dismiss_right = card.right - 1. - 10.;
    Parts {
        card,
        action: rect(right - width, cy - h / 2., right, cy + h / 2.),
        dismiss: rect(
            dismiss_right - 24.,
            card.top + 1. + 10.,
            dismiss_right,
            card.top + 35.,
        ),
    }
}

/// `role="status"` with the copy, the action and Dismiss.
pub fn uia_snapshot() -> Option<panel_uia::Snapshot> {
    with_notice(|n| {
        let status = panel_uia::Item::new(
            0,
            panel_uia::Role::Status,
            format!("{}. {}", n.state.title(), n.state.detail()),
            n.parts.card,
        );
        let mut action = panel_uia::Item::new(
            1,
            panel_uia::Role::Button,
            n.state.action().0,
            n.parts.action,
        );
        action.parent = Some(0);
        action.enabled = !n.state.busy;
        let mut dismiss =
            panel_uia::Item::new(2, panel_uia::Role::Button, "Dismiss", n.parts.dismiss);
        dismiss.parent = Some(0);
        panel_uia::Snapshot {
            hwnd: n.hwnd,
            scale: n.scale,
            size: SIZE,
            name: TITLE,
            focusable: false,
            items: vec![status, action, dismiss],
        }
    })
}

pub fn uia_act(key: u32, act: panel_uia::Act) -> Option<()> {
    with_notice(|_| ())?;
    if matches!(act, panel_uia::Act::Invoke) {
        match key {
            1 => {
                with_notice(|n| n.act());
            }
            2 => {
                hide();
            }
            _ => {}
        }
    }
    Some(())
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_TIMER if wp.0 == FRAME_TIMER => {
                with_notice(|n| {
                    let _ = n.render();
                    n.schedule();
                });
                LRESULT(0)
            }
            WM_TIMER if wp.0 == HIDE_TIMER => {
                hide();
                LRESULT(0)
            }
            WM_TIMER if wp.0 == ANNOUNCE_TIMER => {
                let _ = KillTimer(Some(hwnd), ANNOUNCE_TIMER);
                let copy = with_notice(|n| format!("{}. {}", n.state.title(), n.state.detail()));
                if let Some(copy) = copy {
                    panel_uia::opened(panel_uia::Window::RecordingReady, &copy);
                }
                LRESULT(0)
            }
            WM_DONE => {
                let before = panel_uia::watch(panel_uia::Window::RecordingReady);
                if with_notice(|n| n.finish()) == Some(true) {
                    hide();
                } else {
                    panel_uia::raise(panel_uia::Window::RecordingReady, before);
                }
                LRESULT(0)
            }
            WM_GETOBJECT if lp.0 as i32 == UiaRootObjectId => UiaReturnRawElementProvider(
                hwnd,
                wp,
                lp,
                &panel_uia::node(panel_uia::Window::RecordingReady, None),
            ),
            WM_DESTROY => {
                let _ = UiaReturnRawElementProvider(hwnd, WPARAM(0), LPARAM(0), None);
                LRESULT(0)
            }
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            WM_MOUSEMOVE | WM_LBUTTONUP => {
                let (x, y) = (lp.0 as u16 as i16 as f32, (lp.0 >> 16) as u16 as i16 as f32);
                let clicked = with_notice(|n| {
                    let over = n.part_at(x / n.scale, y / n.scale);
                    if !n.tracking {
                        let _ = TrackMouseEvent(&mut TRACKMOUSEEVENT {
                            cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                            dwFlags: TME_LEAVE,
                            hwndTrack: hwnd,
                            ..Default::default()
                        });
                        n.tracking = true;
                    }
                    if over != n.hover {
                        n.hover = over;
                        let _ = n.render();
                    }
                    over.filter(|_| msg == WM_LBUTTONUP)
                })
                .flatten();
                match clicked {
                    Some(Part::Dismiss) => {
                        hide();
                    }
                    Some(Part::Action) => {
                        let before = panel_uia::watch(panel_uia::Window::RecordingReady);
                        with_notice(|n| n.act());
                        panel_uia::raise(panel_uia::Window::RecordingReady, before);
                    }
                    None => {}
                }
                LRESULT(0)
            }
            WM_MOUSELEAVE => {
                with_notice(|n| {
                    n.tracking = false;
                    if n.hover.take().is_some() {
                        let _ = n.render();
                    }
                });
                LRESULT(0)
            }
            WM_SETCURSOR => {
                let busy = with_notice(|n| n.state.busy).unwrap_or(false);
                let hover = with_notice(|n| n.hover).flatten();
                let hand = matches!(hover, Some(Part::Dismiss))
                    || matches!(hover, Some(Part::Action)) && !busy;
                SetCursor(LoadCursorW(None, if hand { IDC_HAND } else { IDC_ARROW }).ok());
                LRESULT(1)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_PAINT => {
                let _ = ValidateRect(Some(hwnd), None);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_follows_shipping_for_ready_saving_saved_and_errors() {
        let mut s = State::default();
        assert_eq!(s.title(), "Recording ready");
        assert_eq!(
            s.detail(),
            "Kept in Capture History for 30 days. Save a copy anytime."
        );
        assert_eq!(s.action(), ("Save file", "save"));
        s.busy = true;
        assert_eq!(s.action(), ("Saving…", "save"));
        s.busy = false;
        s.error = Some("Could not save the recording: disk full".into());
        assert_eq!(s.title(), "Recording ready");
        assert_eq!(s.detail(), "Could not save the recording: disk full");
        s.error = None;
        s.saved = Some(r"C:\Captures\clip.mp4".into());
        assert_eq!(s.title(), "Recording saved");
        assert_eq!(s.detail(), "Saved to your Captures folder.");
        assert_eq!(s.action(), ("Show in Folder", "folder"));
        s.busy = true;
        assert_eq!(s.action(), ("Opening…", "folder"));
    }

    #[test]
    fn lifecycle_arrives_holds_and_is_gone_before_the_window_closes() {
        let t = Tokens::variant("dark", "mustard");
        let a = Motion::RecordingSavedLifecycle.resolve(&t).unwrap();
        let total = LIFETIME.as_secs_f64() * 1000.;
        let until = |since: f64| (total - since - CLOSE_AFTER_ANIMATION_MS).max(0.);
        let pose = |since: f64, reduced| a.lifecycle_pose(since, Some(until(since)), reduced);
        assert_eq!(pose(0., false).opacity, 0.);
        assert!(pose(0., false).translate_y < 0.);
        assert_eq!(pose(5_000., false), Pose::REST);
        assert!(!a.lifecycle_running(5_000., Some(until(5_000.)), false));
        let fading = pose(14_000., false);
        assert!(fading.opacity > 0. && fading.opacity < 1.);
        assert_eq!(pose(15_000., false).opacity, 0.);
        // Reduced motion: still and fully visible until the window closes.
        assert_eq!(pose(0., true), Pose::REST);
        assert_eq!(pose(14_900., true), Pose::REST);
        assert!(!a.lifecycle_running(0., Some(until(0.)), true));
    }
}
