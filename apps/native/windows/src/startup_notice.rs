//! Shipping's launch notice (`StartupNotice`, `.tray-notice.startup-notice`):
//! "Captures is ready to use" with the New Capture keys, anchored to the
//! notification-area icon by the shared `tray_notice` placement, shown after
//! first-run setup (15 s) and on a quiet launch (5 s). Like shipping, it never
//! takes focus, Close or a tray left-click hides it early, and while the icon
//! has no usable rect yet it retries before falling back above the taskbar.
use std::cell::RefCell;
use std::time::Duration;

use captures_app::tray_notice::{self, Caret, LogicalRect, Placement};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
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

use crate::gfx::{self, Gfx, rect};
use crate::panel_uia;
use crate::tokens::Tokens;

const CLASS: PCWSTR = w!("CapturesStartupNotice");
const RETRY_TIMER: usize = 1;
const HIDE_TIMER: usize = 2;
const WM_MOUSELEAVE: u32 = 0x02A3;

thread_local! {
    static NOTICE: RefCell<Option<Notice>> = const { RefCell::new(None) };
}

struct Notice {
    hwnd: HWND,
    g: Gfx,
    t: Tokens,
    keys: Vec<String>,
    visible_for: Duration,
    attempts: u32,
    placement: Placement,
    scale: f32,
    composition: Option<(
        IDCompositionTarget,
        IDCompositionVisual2,
        IDCompositionSurface,
    )>,
    hover: bool,
    tracking: bool,
    close: D2D_RECT_F,
}

impl Drop for Notice {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

/// The physical tray icon rect, or `None` while it has no usable one.
pub type TrayRect = fn() -> Option<RECT>;

/// Show the notice for `visible_for`; a later one replaces it.
pub fn show(t: Tokens, keys: Vec<String>, visible_for: Duration, tray: TrayRect) {
    hide();
    if let Err(error) = open(t, keys, visible_for, tray) {
        eprintln!("failed to show Captures launch notice: {error}");
        return;
    }
    // `role="status"`: announce the new notice. Raise it even before any
    // reader has asked this process for a provider: at login autostart
    // `UiaClientsAreListening` is still false, so gating on it went silent.
    panel_uia::raise(panel_uia::Window::Notice, Some(Vec::new()));
}

/// The notice's parts for UI Automation: the status card and Close.
pub fn uia_snapshot() -> Option<panel_uia::Snapshot> {
    with_notice(|n| {
        let card = n.placement.card_rect();
        let card = rect(
            card.x as f32,
            card.y as f32,
            (card.x + card.width) as f32,
            (card.y + card.height) as f32,
        );
        let hint = if n.keys.is_empty() {
            "Open History from the tray menu".to_string()
        } else {
            format!("{} {}", tray_notice::STARTUP_NOTICE_HINT, n.keys.join(" "))
        };
        let status = panel_uia::Item::new(
            0,
            panel_uia::Role::Status,
            format!("{}. {hint}", tray_notice::STARTUP_NOTICE_TITLE),
            card,
        );
        let mut close = panel_uia::Item::new(1, panel_uia::Role::Button, "Close", n.close);
        close.parent = Some(0);
        panel_uia::Snapshot {
            hwnd: n.hwnd,
            scale: n.scale,
            size: (f32::INFINITY, f32::INFINITY),
            name: tray_notice::STARTUP_NOTICE_WINDOW_TITLE,
            focusable: false,
            items: vec![status, close],
        }
    })
}

/// Close is the notice's only action.
pub fn uia_act(key: u32, act: panel_uia::Act) -> Option<()> {
    with_notice(|_| ())?;
    if key == 1 && matches!(act, panel_uia::Act::Invoke) {
        hide();
    }
    Some(())
}

/// Hide a visible notice. True when one was up (a tray left-click then only
/// dismisses it, as shipping's `dismiss_visible_tray_notice`).
pub fn hide() -> bool {
    NOTICE
        .with(|n| n.try_borrow_mut().ok().and_then(|mut n| n.take()))
        .is_some()
}

thread_local! {
    static TRAY: std::cell::Cell<Option<TrayRect>> = const { std::cell::Cell::new(None) };
}

fn open(t: Tokens, keys: Vec<String>, visible_for: Duration, tray: TrayRect) -> Result<()> {
    let g = Gfx::new(&t, true)?;
    TRAY.with(|c| c.set(Some(tray)));
    unsafe {
        let instance = windows::Win32::System::LibraryLoader::GetModuleHandleW(None)?;
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: CLASS,
            ..Default::default()
        });
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_NOREDIRECTIONBITMAP,
            CLASS,
            &HSTRING::from(tray_notice::STARTUP_NOTICE_WINDOW_TITLE),
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
        let (placement, scale) = resolve(tray());
        NOTICE.with(|n| {
            *n.borrow_mut() = Some(Notice {
                hwnd,
                g,
                t,
                keys,
                visible_for,
                attempts: 0,
                placement,
                scale,
                composition: None,
                hover: false,
                tracking: false,
                close: rect(0., 0., 0., 0.),
            })
        });
        if placement.caret == Caret::None && tray_notice::should_retry_tray_rect() {
            // The icon may not be laid out yet; poll before falling back.
            let delay = tray_notice::STARTUP_NOTICE_TRAY_RETRY_DELAY.as_millis() as u32;
            SetTimer(Some(hwnd), RETRY_TIMER, delay, None);
        } else {
            with_notice(|n| n.present());
        }
    }
    Ok(())
}

fn with_notice<R>(f: impl FnOnce(&mut Notice) -> R) -> Option<R> {
    NOTICE.with(|n| n.try_borrow_mut().ok()?.as_mut().map(f))
}

/// Placement in logical points on the tray's monitor (or the primary one),
/// with that monitor's scale.
fn resolve(tray: Option<RECT>) -> (Placement, f32) {
    unsafe {
        let center = tray.map_or(POINT::default(), |r| POINT {
            x: (r.left + r.right) / 2,
            y: (r.top + r.bottom) / 2,
        });
        let monitor = MonitorFromPoint(center, MONITOR_DEFAULTTOPRIMARY);
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let _ = GetMonitorInfoW(monitor, &mut info);
        let (mut dpi, mut unused) = (96, 96);
        let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi, &mut unused);
        let scale = dpi.max(96) as f64 / 96.;
        let logical = |r: RECT| {
            LogicalRect::new(
                r.left as f64 / scale,
                r.top as f64 / scale,
                (r.right - r.left) as f64 / scale,
                (r.bottom - r.top) as f64 / scale,
            )
        };
        let placement = tray_notice::startup_notice_placement(
            logical(info.rcMonitor),
            logical(info.rcWork),
            tray.map(logical),
            false,
            tray_notice::FallbackEdge::for_current_platform(),
        );
        (placement, scale as f32)
    }
}

impl Notice {
    fn present(&mut self) {
        let p = self.placement;
        let s = self.scale as f64;
        let px = ((p.width * s).round() as i32, (p.height * s).round() as i32);
        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                (p.x * s).round() as i32,
                (p.y * s).round() as i32,
                px.0,
                px.1,
                SWP_NOACTIVATE,
            );
        }
        if let Err(error) = self.render() {
            eprintln!("failed to draw Captures launch notice: {error}");
        }
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
            SetTimer(
                Some(self.hwnd),
                HIDE_TIMER,
                self.visible_for.as_millis() as u32,
                None,
            );
        }
    }

    fn render(&mut self) -> Result<()> {
        let dcomp = self.g.dcomp.clone().ok_or_else(Error::empty)?;
        let p = self.placement;
        let px = (
            (p.width * self.scale as f64).round().max(1.) as u32,
            (p.height * self.scale as f64).round().max(1.) as u32,
        );
        let surface = self.g.surface(px.0, px.1)?;
        unsafe {
            if self.composition.is_none() {
                let target = dcomp.CreateTargetForHwnd(self.hwnd, true)?;
                let visual = dcomp.CreateVisual()?;
                target.SetRoot(&visual)?;
                self.composition = Some((target, visual, surface.clone()));
            }
            if let Some((_, visual, current)) = self.composition.as_mut() {
                visual.SetContent(&surface)?;
                *current = surface.clone();
            }
        }
        let mut close = self.close;
        gfx::draw_surface(&surface, px, (self.scale, self.scale), 0., |dc| {
            close = paint(dc, &self.g, &self.t, &p, &self.keys, self.hover)?;
            Ok(())
        })?;
        self.close = close;
        unsafe { dcomp.Commit() }
    }
}

/// The notice in window-local points for `placement`; returns the Close
/// button's rect.
pub fn paint(
    dc: &ID2D1DeviceContext,
    g: &Gfx,
    t: &Tokens,
    placement: &Placement,
    keys: &[String],
    hover_close: bool,
) -> Result<D2D_RECT_F> {
    let n = |name: &str| t.n(name);
    let c = |name: &str| t.color(name);
    let anchored = placement.caret != Caret::None;
    let card = placement.card_rect();
    let mut card = rect(
        card.x as f32,
        card.y as f32,
        (card.x + card.width) as f32,
        (card.y + card.height) as f32,
    );
    if !anchored {
        // `.startup-notice:not([data-caret]) .startup-notice-card`: fixed
        // width and minimum height, never stretched to the window.
        let w = tray_notice::STARTUP_NOTICE_WIDTH as f32;
        card.right = card.left + w.min(card.right - card.left);
        let h = tray_notice::STARTUP_NOTICE_HEIGHT as f32;
        let mid = (card.top + card.bottom) / 2.;
        card.top = mid - h / 2.;
        card.bottom = mid + h / 2.;
    }
    let radius = if anchored {
        (card.bottom - card.top) / 2.
    } else {
        n("r-lg")
    };
    let shadow = if anchored {
        "tooltip-shadow"
    } else {
        "tooltip-shadow-compact"
    };
    gfx::shadow(dc, card, radius, t.shadow(shadow))?;
    let fill = c("glass-strong-solid");
    gfx::fill_round(dc, card, radius, fill)?;
    if let Some([tip, a, b]) = placement.caret_triangle(tray_notice::STARTUP_NOTICE_CARET_SPAN) {
        let d = format!("M{} {}L{} {}L{} {}Z", tip.0, tip.1, a.0, a.1, b.0, b.1);
        gfx::path(dc, &g.factory, &d, true, (0., 0., 1.), 0., fill)?;
    }
    if !anchored {
        gfx::border(dc, card, radius, 1., c("glass-border"))?;
    }

    // `strong` and the `p` line with its `kbd` chips.
    let title = g.text_sized(
        t,
        tray_notice::STARTUP_NOTICE_TITLE,
        n("text-md"),
        "weight-semibold",
        Some("tracking-tight"),
    )?;
    let size = n("text-sm");
    let hint = g.text_sized(
        t,
        if keys.is_empty() {
            "Open History from the tray menu"
        } else {
            tray_notice::STARTUP_NOTICE_HINT
        },
        size,
        "weight-regular",
        Some("tracking-tight"),
    )?;
    let space = g.text_sized(t, "a a", size, "weight-regular", None)?.width
        - g.text_sized(t, "aa", size, "weight-regular", None)?.width;
    let chips = keys
        .iter()
        .map(|key| g.text_sized(t, key, size, "weight-semibold", Some("tracking-tight")))
        .collect::<Result<Vec<_>>>()?;
    let (chip_pad, chip_gap) = (n("s-1"), n("s-1"));
    let line_width = hint.width
        + if chips.is_empty() { 0. } else { space }
        + chips
            .iter()
            .map(|chip| chip_gap + chip.width + 2. * chip_pad + 2.)
            .sum::<f32>();
    let gap = if anchored { 0. } else { n("s-1") };
    let block = title.height + gap + hint.height;
    let (left, top, bottom, right) = if anchored {
        (
            card.left + n("s-6"),
            card.top + n("s-2"),
            card.bottom - n("s-3"),
            card.right - n("s-6"),
        )
    } else {
        (
            card.left + 1. + n("s-5"),
            card.top + 1. + n("s-4"),
            card.bottom - 1. - n("s-4"),
            card.right - 1. - n("h-sm") - n("s-5"),
        )
    };
    let mut y = top + ((bottom - top) - block) / 2.;
    let center = |width: f32| {
        if anchored {
            left + ((right - left) - width) / 2.
        } else {
            left
        }
    };
    gfx::text(dc, &title, center(title.width), y, c("glass-text"))?;
    y += title.height + gap;
    // Wider fonts or longer shortcuts slide the line left rather than run
    // under Close.
    let close_left = card.right - n("s-3") - n("h-sm");
    let mut x = center(line_width)
        .min(close_left - n("s-2") - line_width)
        .max(left);
    let hint_color = if anchored {
        "glass-text-subtle"
    } else {
        "glass-text-muted"
    };
    gfx::text(dc, &hint, x, y, c(hint_color))?;
    x += hint.width + space;
    for chip in &chips {
        x += chip_gap;
        let r = rect(
            x,
            y - 1.,
            x + chip.width + 2. * chip_pad + 2.,
            y + hint.height + 1.,
        );
        gfx::fill_round(dc, r, n("r-sm"), c("glass-hover"))?;
        gfx::border(dc, r, n("r-sm"), 1., c("glass-border"))?;
        gfx::text(dc, chip, r.left + 1. + chip_pad, y, c("glass-text"))?;
        x = r.right;
    }

    // `.startup-notice-dismiss`
    let side = n("h-sm");
    let mid = (card.top + card.bottom) / 2.;
    let close = rect(
        card.right - n("s-3") - side,
        mid - side / 2.,
        card.right - n("s-3"),
        mid + side / 2.,
    );
    if hover_close {
        gfx::fill_round(dc, close, side / 2., c("glass-hover"))?;
    }
    gfx::icon(
        dc,
        &g.factory,
        "close",
        &[],
        (close.left + (side - 14.) / 2., mid - 7., 14.),
        2. * 24. / 14.,
        c(if hover_close {
            "glass-text"
        } else {
            "glass-text-subtle"
        }),
    )?;
    Ok(close)
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_TIMER if wp.0 == RETRY_TIMER => {
                let tray = TRAY.with(|c| c.get());
                with_notice(|n| {
                    n.attempts += 1;
                    let (placement, scale) = resolve(tray.and_then(|tray| tray()));
                    (n.placement, n.scale) = (placement, scale);
                    if placement.caret != Caret::None
                        || n.attempts >= tray_notice::STARTUP_NOTICE_TRAY_RETRY_ATTEMPTS
                    {
                        let _ = KillTimer(Some(hwnd), RETRY_TIMER);
                        n.present();
                    }
                });
                LRESULT(0)
            }
            WM_TIMER if wp.0 == HIDE_TIMER => {
                hide();
                LRESULT(0)
            }
            WM_GETOBJECT if lp.0 as i32 == UiaRootObjectId => UiaReturnRawElementProvider(
                hwnd,
                wp,
                lp,
                &panel_uia::node(panel_uia::Window::Notice, None),
            ),
            WM_DESTROY => {
                let _ = UiaReturnRawElementProvider(hwnd, WPARAM(0), LPARAM(0), None);
                LRESULT(0)
            }
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            WM_MOUSEMOVE | WM_LBUTTONUP => {
                let (x, y) = (lp.0 as u16 as i16 as f32, (lp.0 >> 16) as u16 as i16 as f32);
                let close = with_notice(|n| {
                    let (x, y) = (x / n.scale, y / n.scale);
                    let over = x >= n.close.left
                        && x < n.close.right
                        && y >= n.close.top
                        && y < n.close.bottom;
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
                    over && msg == WM_LBUTTONUP
                });
                if close == Some(true) {
                    hide();
                }
                LRESULT(0)
            }
            WM_MOUSELEAVE => {
                with_notice(|n| {
                    n.tracking = false;
                    if std::mem::take(&mut n.hover) {
                        let _ = n.render();
                    }
                });
                LRESULT(0)
            }
            WM_SETCURSOR => {
                let hover = with_notice(|n| n.hover).unwrap_or(false);
                SetCursor(LoadCursorW(None, if hover { IDC_HAND } else { IDC_ARROW }).ok());
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
