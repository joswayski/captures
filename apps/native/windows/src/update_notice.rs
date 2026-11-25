//! Shipping's floating update notice (`UpdateNotice` in `App.tsx`, the
//! `.update-notice*` rules in `styles/windows.css`): a solid
//! `--surface-raised` card in a transparent, never-activating topmost window,
//! with a caret toward the notification-area icon when its rect is known and
//! the shared caretless tray-corner fallback otherwise. Copy and state come
//! from `captures_app::update_notice::present`; placement from
//! `update_notice::placement`.
//!
//! The resident app (`live.rs`) maps its `CheckWorker::preview` status to
//! shipping's `UpdateStatus` with [`status`] and calls [`sync`] after every
//! poll. [`Schedule`] is shipping's `schedule_update_notice` and
//! `notice_disposition`: a newly available version (once per version per
//! session) or a failed install started here opens the notice, deferred
//! while a capture or recording runs. The notice's actions reach the worker
//! through the same `updates::Step`s as Preferences' Updates card.
use std::cell::RefCell;
use std::path::PathBuf;
use std::time::Instant;

use captures_app::motion::{CubicBezier, Motion};
use captures_app::tray_notice::{self, Caret, LogicalRect, Placement};
use captures_app::update_notice::{
    self as notice, Action, Icon, IconTone, Presentation, UpdateStatus, ViewState,
};
use captures_app::updater::ReleaseInfo;
use captures_app::updater::checks::CheckStatus;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectComposition::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromPoint, ValidateRect,
};
use windows::Win32::UI::Accessibility::{UiaReturnRawElementProvider, UiaRootObjectId};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;
use windows_numerics::Vector2;

use crate::gfx::{self, Gfx, Text, rect};
use crate::notice_motion::{self, Curve};
use crate::panel_uia::{self, Role};
use crate::startup_notice::TrayRect;
use crate::tokens::Tokens;
use crate::updates::Step;

/// Shipping's window title (`.title("Captures Update")`).
pub const TITLE: &str = "Captures Update";
/// Shipping's View release for a copy that cannot update itself.
const RELEASES_URL: &str = "https://github.com/joswayski/captures/releases";
const CLASS: PCWSTR = w!("CapturesUpdateNotice");
const ANIMATION_TIMER: usize = 1;
/// Polls the tray icon rect before the first show, like the launch notice,
/// and after a caretless fallback show until the icon has a usable rect.
const RETRY_TIMER: usize = 2;
/// Polls before the first show while the tray menu or overflow flyout is
/// still open (New Capture's bounded shell-menu wait).
const MENU_WAIT_POLLS: u32 = (crate::capture::SHELL_MENUS_WAIT.as_millis()
    / tray_notice::STARTUP_NOTICE_TRAY_RETRY_DELAY.as_millis()) as u32;
/// After a fallback show, poll this often, this many times (a minute), for a
/// usable tray rect, such as the overflow chevron once the flyout closes.
const REPLACE_DELAY_MS: u32 = 250;
const REPLACE_POLLS: u32 = 240;
/// Announces the shown notice once focus has settled.
const ANNOUNCE_TIMER: usize = 3;
const ANNOUNCE_DELAY_MS: u32 = 400;
const WM_MOUSELEAVE: u32 = 0x02A3;
/// Shipping's restart countdown, which the restart bar empties over.
const RESTART_SECONDS: f32 = 3.;

// ------------------------------------------------------------- contract ----

/// The worker's status in shipping's `UpdateStatus` terms. `installing`:
/// Update now was pressed, so a verified package restarts; `installable`: the
/// worker can stage a package (otherwise shipping's View release);
/// `will_close`: a capture, editor or recording is open.
pub fn status(
    current: &str,
    check: &CheckStatus,
    installing: bool,
    installable: bool,
    will_close: bool,
) -> UpdateStatus {
    use crate::updates::display_version;
    let current_version = current.to_owned();
    let current_display_version = display_version(current);
    let available = |release: &ReleaseInfo| UpdateStatus::Available {
        current_version: current_version.clone(),
        current_display_version: current_display_version.clone(),
        version: release.version.clone(),
        display_version: display_version(&release.version),
        notes: release.notes.clone(),
        changelog: release.changelog.clone(),
        installable,
        manual_download_url: (!installable).then(|| RELEASES_URL.to_owned()),
        download_size: (release.size > 0).then_some(release.size),
        will_close_open_captures: will_close,
    };
    let downloading = |version: &str, downloaded: u64, total: u64| UpdateStatus::Downloading {
        current_version: current_version.clone(),
        current_display_version: current_display_version.clone(),
        version: version.to_owned(),
        display_version: display_version(version),
        downloaded,
        total: (total > 0).then_some(total),
    };
    match check {
        CheckStatus::Idle => UpdateStatus::Idle {
            current_version,
            current_display_version,
        },
        CheckStatus::Checking => UpdateStatus::Checking {
            current_version,
            current_display_version,
        },
        CheckStatus::UpToDate => UpdateStatus::UpToDate {
            current_version,
            current_display_version,
        },
        CheckStatus::Available { release } => available(release),
        CheckStatus::Downloading {
            release,
            downloaded,
            total,
        } => downloading(&release.version, *downloaded, *total),
        // Shipping has no separate verification step: the download is done.
        CheckStatus::Verifying { release } | CheckStatus::Cancelling { release } => {
            downloading(&release.version, release.size, release.size)
        }
        CheckStatus::Staged { release, .. } if installing => UpdateStatus::Restarting {
            display_version: display_version(&release.version),
            version: release.version.clone(),
            current_version,
            current_display_version,
            seconds_remaining: RESTART_SECONDS as u8,
        },
        // Verified, but the install was held (a capture was running).
        CheckStatus::Staged { release, .. } => available(release),
        CheckStatus::Error { message } => UpdateStatus::Error {
            current_version,
            current_display_version,
            message: message.clone(),
            retry_install: false,
        },
        CheckStatus::DownloadError { message, .. } => UpdateStatus::Error {
            current_version,
            current_display_version,
            message: captures_app::updater::install_failure_message(message),
            retry_install: true,
        },
    }
}

/// Shipping's `NoticeDisposition`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Disposition {
    Ignore,
    Show,
    Defer,
}

/// Shipping `notice_disposition`: errors always show; an available or
/// downloading update waits for the capture or recording to finish.
pub fn disposition(status: &UpdateStatus, capture_active: bool) -> Disposition {
    match status {
        UpdateStatus::Error { .. } => Disposition::Show,
        UpdateStatus::Available { .. } | UpdateStatus::Downloading { .. } if capture_active => {
            Disposition::Defer
        }
        UpdateStatus::Available { .. } | UpdateStatus::Downloading { .. } => Disposition::Show,
        _ => Disposition::Ignore,
    }
}

/// When the notice opens on its own: shipping's `notified_version` (one
/// notice per available version per session, forgotten once a check finds
/// Captures up to date) and the deferral while a capture runs.
#[derive(Default, Debug)]
pub struct Schedule {
    notified: Option<String>,
    pending: bool,
    /// Update now or Try again was pressed in the notice.
    installing: bool,
    failed: bool,
}

impl Schedule {
    /// Update now or Try again was pressed in the notice: a failure shows.
    pub fn install_requested(&mut self) {
        self.installing = true;
    }

    /// Observe the status after a poll; true when the notice should open now.
    pub fn observe(&mut self, status: &UpdateStatus, capture_active: bool, visible: bool) -> bool {
        let failed = matches!(status, UpdateStatus::Error { .. });
        match status {
            UpdateStatus::Available { version, .. }
                if self.notified.as_deref() != Some(version.as_str()) =>
            {
                self.notified = Some(version.clone());
                self.pending = true;
            }
            UpdateStatus::UpToDate { .. } => self.notified = None,
            _ => {}
        }
        if failed && !self.failed && self.installing {
            self.pending = true;
        }
        if !matches!(
            status,
            UpdateStatus::Available { .. } | UpdateStatus::Downloading { .. }
        ) {
            // The attempt ended: it failed, restarts, or a check replaced it.
            self.installing = false;
        }
        self.failed = failed;
        if !self.pending {
            return false;
        }
        if visible {
            self.pending = false;
            return false;
        }
        match disposition(status, capture_active) {
            Disposition::Show => {
                self.pending = false;
                true
            }
            Disposition::Defer => false,
            Disposition::Ignore => {
                self.pending = false;
                false
            }
        }
    }
}

// ------------------------------------------------------------- live app ----

/// What the resident app hands the notice after each poll.
pub struct Live {
    pub status: UpdateStatus,
    /// The Updates card's step: what Update now / Try again runs.
    pub step: Option<Step>,
    pub capture_active: bool,
    pub settings_file: PathBuf,
    pub appearance: Option<String>,
    pub tray: TrayRect,
}

#[derive(Default)]
struct State {
    schedule: Schedule,
    /// A notice button's step, taken by the resident app's next poll.
    step: Option<Step>,
    /// An install refused by a running capture, shown as the notice error.
    blocked: Option<String>,
}

thread_local! {
    static NOTICE: RefCell<Option<Notice>> = const { RefCell::new(None) };
    static STATE: RefCell<State> = RefCell::default();
    /// `--scene update` without `--screenshot`: closing the notice quits.
    static FIXTURE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Open, refresh or leave the notice for the worker's latest status.
pub fn sync(live: Live) {
    let visible = with_notice(|_| ()).is_some();
    let show = STATE.with(|s| {
        s.borrow_mut()
            .schedule
            .observe(&live.status, live.capture_active, visible)
    });
    let blocked = STATE.with(|s| s.borrow_mut().blocked.take());
    if visible {
        let before = panel_uia::watch(panel_uia::Window::Update);
        with_notice(|n| {
            let changed = n.status != live.status;
            if changed {
                n.view.action_error = None;
                n.view.follow(Some(&n.status), &live.status);
            }
            if blocked.is_some() {
                n.view.action_error = blocked;
            }
            n.status = live.status;
            n.step = live.step;
            n.refresh(changed);
        });
        panel_uia::raise(panel_uia::Window::Update, before);
    } else if show
        // Hide update pop-ups: only the tray and Preferences offer the update.
        && !captures_settings::load(&live.settings_file).is_ok_and(|s| s.hide_update_notices)
        && let Err(error) = open(live)
    {
        eprintln!("failed to show the Captures update notice: {error}");
    }
}

/// Open the notice now if it is not up (shipping's tray Check for Updates
/// with an update available), then refresh it.
pub fn show(live: Live) {
    if with_notice(|_| ()).is_some() {
        return sync(live);
    }
    if let Err(error) = open(live) {
        eprintln!("failed to show the Captures update notice: {error}");
    }
}

/// A notice button's step for the worker, once.
pub fn take_step() -> Option<Step> {
    STATE.with(|s| s.borrow_mut().step.take())
}

/// Shipping returns an install refused by a running capture to the notice,
/// which shows it as the error. False when no notice is open.
pub fn blocked(message: &str) -> bool {
    let visible = with_notice(|_| ()).is_some();
    if visible {
        STATE.with(|s| s.borrow_mut().blocked = Some(message.to_owned()));
    }
    visible
}

/// Settings changed elsewhere (appearance, theme, What's new in Preferences).
pub fn settings_changed(settings: &captures_settings::AppSettings) {
    with_notice(|n| {
        n.t = crate::appearance::tokens(n.appearance.as_deref(), settings);
        n.view.show_changelog = settings.show_update_changelog;
        n.refresh(true);
    });
}

/// Hide the notice (Not now/Close). True when one was up.
pub fn hide() -> bool {
    let hidden = NOTICE
        .with(|n| n.try_borrow_mut().ok().and_then(|mut n| n.take()))
        .is_some();
    if hidden && FIXTURE.get() {
        unsafe { PostQuitMessage(0) };
    }
    hidden
}

/// `--scene update --update NAME` without `--screenshot`: the fixture as the
/// live notice, anchored to the taskbar's notification area (`--tool
/// fallback`: the caretless corner) until Later or Close quits. Its buttons
/// reach no worker.
pub fn open_fixture(options: &crate::Options) -> Result<()> {
    let name = options.update.as_deref().unwrap_or("available");
    let status =
        notice::fixture(name).ok_or_else(|| Error::new(E_INVALIDARG, "unknown --update"))?;
    FIXTURE.set(true);
    open(Live {
        status,
        step: None,
        capture_active: false,
        settings_file: options.settings_file.clone(),
        appearance: options.appearance.clone(),
        tray: if options.tool.as_deref() == Some("fallback") {
            || None
        } else {
            notification_area
        },
    })
}

/// The taskbar's notification area, standing in for the tray icon that a
/// fixture without the resident app does not have.
fn notification_area() -> Option<RECT> {
    unsafe {
        let taskbar = FindWindowW(w!("Shell_TrayWnd"), None).ok()?;
        let area = FindWindowExW(Some(taskbar), None, w!("TrayNotifyWnd"), None).ok()?;
        let mut r = RECT::default();
        GetWindowRect(area, &mut r).ok()?;
        (r.right > r.left && r.bottom > r.top).then_some(r)
    }
}

fn with_notice<R>(f: impl FnOnce(&mut Notice) -> R) -> Option<R> {
    NOTICE.with(|n| n.try_borrow_mut().ok()?.as_mut().map(f))
}

struct Notice {
    hwnd: HWND,
    g: Gfx,
    t: Tokens,
    appearance: Option<String>,
    tray: TrayRect,
    /// The last usable tray icon rect. A re-placement keeps it when a later
    /// lookup fails, so the caret and position stay put.
    anchor: Option<RECT>,
    /// Tray rect polls in this phase: with the tray menu or overflow flyout
    /// open, and without.
    menu_waits: u32,
    attempts: u32,
    /// Shown (`present`); a caretless show keeps polling to re-place.
    shown: bool,
    status: UpdateStatus,
    step: Option<Step>,
    view: ViewState,
    presentation: Presentation,
    placement: Placement,
    scale: f32,
    composition: Option<(IDCompositionTarget, IDCompositionVisual2)>,
    parts: Vec<Part>,
    hover: Option<u32>,
    pressed: Option<u32>,
    tracking: bool,
    scroll: f32,
    scroll_max: f32,
    notes: Option<D2D_RECT_F>,
    /// The version link's part, when the description has one.
    link: Option<u32>,
    /// The version link's hover: how far it has come (0 to 1, before
    /// easing), toward which end, and when it last stepped.
    glow: (f32, bool, Instant),
    started: Instant,
    restart_at: Option<Instant>,
    animating: bool,
    /// An update's card height fitted to its painted content; the shared
    /// estimate is the most it takes.
    fit: Option<f64>,
    /// The offered card's fitted height, held while its list stays on to
    /// download and restart (`ViewState::follow`).
    held: f64,
    /// The last paint fitted a new height: place the card again.
    refit: bool,
}

impl Drop for Notice {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

fn open(live: Live) -> Result<()> {
    let settings = captures_settings::load(&live.settings_file).unwrap_or_default();
    let t = crate::appearance::tokens(live.appearance.as_deref(), &settings);
    let g = Gfx::shared(&t)?;
    let view = ViewState {
        show_changelog: settings.show_update_changelog,
        ..ViewState::default()
    };
    let presentation = notice::present(Some(&live.status), &view);
    unsafe {
        let instance = windows::Win32::System::LibraryLoader::GetModuleHandleW(None)?;
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: CLASS,
            ..Default::default()
        });
        // Informational, as shipping's `.focusable(false)`: the app the user
        // is typing in stays active, and a first click still works.
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_NOREDIRECTIONBITMAP,
            CLASS,
            &HSTRING::from(TITLE),
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
        let mut anchor = None;
        let (placement, scale) =
            anchored(tray_rect(live.tray), &mut anchor, presentation.card_height);
        NOTICE.with(|n| {
            *n.borrow_mut() = Some(Notice {
                hwnd,
                g,
                t,
                appearance: live.appearance,
                tray: live.tray,
                anchor,
                menu_waits: 0,
                attempts: 0,
                shown: false,
                status: live.status,
                step: live.step,
                view,
                presentation,
                placement,
                scale,
                composition: None,
                parts: Vec::new(),
                hover: None,
                pressed: None,
                tracking: false,
                scroll: 0.,
                scroll_max: 0.,
                notes: None,
                link: None,
                glow: (0., false, Instant::now()),
                started: Instant::now(),
                restart_at: None,
                animating: false,
                fit: None,
                held: 0.,
                refit: false,
            })
        });
        if placement.caret == Caret::None && tray_notice::should_retry_tray_rect() {
            // As the launch notice: the icon may have no rect yet (a quiet
            // launch), or the tray menu that chose Check for Updates and the
            // overflow flyout holding the icon are still closing; poll before
            // falling back above the taskbar.
            let delay = tray_notice::STARTUP_NOTICE_TRAY_RETRY_DELAY.as_millis() as u32;
            SetTimer(Some(hwnd), RETRY_TIMER, delay, None);
        } else {
            with_notice(|n| n.present());
        }
    }
    Ok(())
}

/// `role="dialog"` in a window that never takes focus: tell a reader it
/// opened and what it says, as the webview's new dialog does.
fn announce() {
    let copy = with_notice(|n| format!("{}. {}", n.presentation.title, n.presentation.description));
    if let Some(copy) = copy {
        panel_uia::opened(panel_uia::Window::Update, &copy);
    }
}

/// The tray icon rect, or none while the tray menu or overflow flyout is
/// open: inside the flyout the icon's rect is above the taskbar (no caret),
/// and once the flyout closes it moves to the hidden-icons chevron.
fn tray_rect(tray: TrayRect) -> Option<RECT> {
    if crate::capture::shell_menus_open() {
        None
    } else {
        tray()
    }
}

/// What one tray rect poll does with the notice.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Poll {
    Wait,
    /// The first show, at the caret or (bounds spent) the caretless fallback.
    Show,
    /// A fallback-placed notice moves to the usable rect, without focus.
    Replace,
    /// Stop polling; a fallback-placed notice stays where it is.
    Stop,
}

/// The next step after a poll. `menu_waits` and `attempts` count this
/// phase's polls, including this one, with the tray menu or overflow flyout
/// open and without; `caret` says the poll found a usable rect (never while
/// they are open).
fn next_poll(shown: bool, menu_waits: u32, attempts: u32, menus_open: bool, caret: bool) -> Poll {
    if shown {
        return if caret {
            Poll::Replace
        } else if menu_waits + attempts >= REPLACE_POLLS {
            Poll::Stop
        } else {
            Poll::Wait
        };
    }
    if caret {
        Poll::Show
    } else if menus_open {
        if menu_waits >= MENU_WAIT_POLLS {
            Poll::Show
        } else {
            Poll::Wait
        }
    } else if attempts >= tray_notice::STARTUP_NOTICE_TRAY_RETRY_ATTEMPTS {
        Poll::Show
    } else {
        Poll::Wait
    }
}

/// Placement for `card_height` at the tray icon rect `found`; when that is
/// missing or unusable, at `anchor`, the last rect that gave a caret.
fn anchored(found: Option<RECT>, anchor: &mut Option<RECT>, card_height: f64) -> (Placement, f32) {
    let fresh = resolve(found, card_height);
    if fresh.0.caret != Caret::None {
        *anchor = found;
        return fresh;
    }
    match *anchor {
        Some(_) => resolve(*anchor, card_height),
        None => fresh,
    }
}

/// Placement in logical points on the tray's monitor (or the primary one),
/// with that monitor's scale.
fn resolve(tray: Option<RECT>, card_height: f64) -> (Placement, f32) {
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
        let placement = notice::placement(
            logical(info.rcMonitor),
            logical(info.rcWork),
            tray.map(logical),
            false,
            notice::CARD_WIDTH,
            card_height,
        );
        (placement, scale as f32)
    }
}

impl Notice {
    /// Present the current status; `moved` re-resolves the placement too.
    fn refresh(&mut self, changed: bool) {
        let presentation = notice::present(Some(&self.status), &self.view);
        if !changed && presentation == self.presentation {
            return;
        }
        let height = presentation.card_height != self.presentation.card_height;
        if presentation.notes.is_none() {
            self.scroll = 0.;
        }
        self.presentation = presentation;
        if height {
            self.replace();
        } else {
            self.render_logged();
        }
    }

    /// Present the notice for a new height at the tray icon, or at the last
    /// usable icon rect when the lookup fails. It can fail transiently (the
    /// physical recheck lost the caret and moved the card 8 pt left after a
    /// height change); the icon has not moved, so neither does the notice.
    fn replace(&mut self) {
        let height = self.height();
        (self.placement, self.scale) = anchored(tray_rect(self.tray), &mut self.anchor, height);
        self.place();
    }

    /// Place and show the notice without activating it, then announce it.
    fn present(&mut self) {
        let first = !std::mem::replace(&mut self.shown, true);
        self.place();
        if first {
            self.play(Motion::UpdateNoticeIn);
        }
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
            // After the tray menu (or another window) has handed focus back,
            // so the focus change does not talk over the announcement.
            SetTimer(Some(self.hwnd), ANNOUNCE_TIMER, ANNOUNCE_DELAY_MS, None);
        }
    }

    /// One tray rect poll: before the first show, as the launch notice but
    /// first waiting (bounded) for the tray menu and overflow flyout to
    /// close; after a caretless fallback show, until a usable rect appears.
    fn retry(&mut self) {
        let open = crate::capture::shell_menus_open();
        if open {
            self.menu_waits += 1;
        } else {
            self.attempts += 1;
        }
        let found = if open { None } else { (self.tray)() };
        let height = self.height();
        let (placement, scale) = anchored(found, &mut self.anchor, height);
        let caret = placement.caret != Caret::None;
        let hwnd = self.hwnd;
        let stop = || unsafe {
            let _ = KillTimer(Some(hwnd), RETRY_TIMER);
        };
        match next_poll(self.shown, self.menu_waits, self.attempts, open, caret) {
            Poll::Wait => {}
            Poll::Show => {
                (self.placement, self.scale) = (placement, scale);
                self.present();
                if caret {
                    stop();
                } else {
                    // Placed by the fallback (the flyout may stay open):
                    // keep watching for the icon's rect.
                    (self.menu_waits, self.attempts) = (0, 0);
                    unsafe {
                        SetTimer(Some(self.hwnd), RETRY_TIMER, REPLACE_DELAY_MS, None);
                    }
                }
            }
            Poll::Replace => {
                stop();
                (self.placement, self.scale) = (placement, scale);
                self.place();
            }
            Poll::Stop => stop(),
        }
    }

    fn height(&self) -> f64 {
        self.fit.unwrap_or(self.presentation.card_height)
    }

    fn place(&mut self) {
        let p = self.placement;
        let s = self.scale as f64;
        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                (p.x * s).round() as i32,
                (p.y * s).round() as i32,
                (p.width * s).round() as i32,
                (p.height * s).round() as i32,
                SWP_NOACTIVATE,
            );
        }
        self.render_logged();
    }

    fn render_logged(&mut self) {
        // The restart state starts shipping's `update-restart-exit` (a 3 s
        // hold, then a fade up and out); a failed restart drops the state and
        // the animation with it, as `:has(.update-restarting)`.
        let restarting = self.presentation.restart.is_some();
        let changed = restarting != self.restart_at.is_some();
        if let Err(error) = self.render() {
            eprintln!("failed to draw the Captures update notice: {error}");
        }
        if std::mem::take(&mut self.refit) {
            self.replace();
        }
        if changed && restarting {
            self.play(Motion::UpdateNoticeRestartExit);
        } else if changed
            && let (Some(dcomp), Some((_, visual))) = (&self.g.dcomp, &self.composition)
            && let Err(error) = notice_motion::rest(dcomp, visual)
        {
            eprintln!("failed to animate the Captures update notice: {error}");
        }
    }

    /// Play a shipping notice motion on the window's visual.
    fn play(&self, motion: Motion) {
        let (Some(dcomp), Some((_, visual)), Some(animation)) =
            (&self.g.dcomp, &self.composition, motion.resolve(&self.t))
        else {
            return;
        };
        let p = self.placement;
        let s = self.scale as f64;
        let size = ((p.width * s) as f32, (p.height * s) as f32);
        let reduced = crate::appearance::reduced_motion();
        let played = notice_motion::play(
            dcomp,
            visual,
            size,
            self.scale,
            Curve::Once(&animation),
            reduced,
        )
        .and_then(|()| unsafe { dcomp.Commit() });
        if let Err(error) = played {
            eprintln!("failed to animate the Captures update notice: {error}");
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
                self.composition = Some((target, visual));
            }
            if let Some((_, visual)) = &self.composition {
                visual.SetContent(&surface)?;
            }
        }
        let reduced = crate::appearance::reduced_motion();
        let restart = if self.presentation.restart.is_some() {
            let at = *self.restart_at.get_or_insert_with(Instant::now);
            if reduced {
                1.
            } else {
                1. - at.elapsed().as_secs_f32() / RESTART_SECONDS
            }
        } else {
            self.restart_at = None;
            1.
        };
        let elapsed = if reduced {
            0.
        } else {
            self.started.elapsed().as_secs_f32()
        };
        let link = self.glow_step(reduced);
        let ui = Ui {
            hover: self.hover,
            scroll: self.scroll,
            time: elapsed,
            restart: restart.clamp(0., 1.),
            link,
        };
        let mut painted = None;
        gfx::draw_surface(&surface, px, (self.scale, self.scale), 0., |dc| {
            painted = Some(paint(dc, &self.g, &self.t, &self.presentation, &p, &ui)?);
            Ok(())
        })?;
        if let Some(painted) = painted {
            self.parts = painted.parts;
            self.scroll_max = painted.scroll_max;
            self.notes = painted.notes;
            self.link = painted.link;
            self.scroll = self.scroll.min(self.scroll_max);
            self.fit_to(painted.fitted);
        }
        let (glow, on, _) = self.glow;
        // The spinner, an indeterminate bar, the restart bar and the version
        // link's hover fade move.
        let animating = !reduced
            && (glow != if on { 1. } else { 0. }
                || self.presentation.icon == Icon::Spinner
                || self
                    .presentation
                    .download
                    .as_ref()
                    .is_some_and(|d| d.percent.is_none())
                || (self.presentation.restart.is_some() && restart > 0.));
        if animating != self.animating {
            self.animating = animating;
            unsafe {
                if animating {
                    SetTimer(Some(self.hwnd), ANIMATION_TIMER, 33, None);
                } else {
                    let _ = KillTimer(Some(self.hwnd), ANIMATION_TIMER);
                }
            }
        }
        unsafe { dcomp.Commit() }
    }

    /// Shrink an update's card to its painted content (the shared estimate
    /// is the most it takes), holding the offered card's height while its
    /// list stays on to download and restart.
    fn fit_to(&mut self, fitted: Option<f32>) {
        let Some(fitted) = fitted else {
            (self.fit, self.held) = (None, 0.);
            return;
        };
        let estimate = self.presentation.card_height;
        // Whole DIPs, so the caret tip stays put across heights.
        let mut height = f64::from(fitted).ceil().min(estimate);
        match self.presentation.visual_state.as_str() {
            "available" => self.held = height,
            "downloading" | "restarting" if self.presentation.notes.is_some() => {
                height = height.max(self.held).min(estimate);
            }
            _ => self.held = 0.,
        }
        // A work-area clamp can keep the card from that height: fit once.
        if self.fit != Some(height) && (height - self.placement.card_rect().height).abs() > 0.5 {
            self.fit = Some(height);
            self.refit = true;
        }
    }

    /// Step the version link's hover and return it eased: 0 at rest, 1
    /// hovered. It moves over `--dur-3` with `--ease-in-out`, at once under
    /// reduced motion; a new direction starts from where it is.
    fn glow_step(&mut self, reduced: bool) -> f32 {
        let on = self.link.is_some() && self.hover == self.link;
        let (mut progress, was, last) = self.glow;
        let now = Instant::now();
        let target = if on { 1. } else { 0. };
        if reduced {
            progress = target;
        } else if on == was {
            let step = now.duration_since(last).as_secs_f32() * 1000. / self.t.n("dur-3");
            progress = if on {
                (progress + step).min(1.)
            } else {
                (progress - step).max(0.)
            };
        }
        self.glow = (progress, on, now);
        let [x1, y1, x2, y2] = self.t.easing("ease-in-out");
        CubicBezier::new(x1, y1, x2, y2)
            .unwrap_or(CubicBezier::LINEAR)
            .ease(f64::from(progress)) as f32
    }

    fn part_at(&self, x: f32, y: f32) -> Option<&Part> {
        self.parts.iter().rev().find(|part| {
            part.action.is_some()
                && x >= part.rect.left
                && x < part.rect.right
                && y >= part.rect.top
                && y < part.rect.bottom
        })
    }
}

/// Apply a notice action, as `UpdateNotice`'s handlers.
fn act(action: Action) {
    match action {
        Action::Dismiss => {
            let blocked = with_notice(|n| n.presentation.dismiss_blocked).unwrap_or(true);
            if !blocked {
                hide();
            }
        }
        Action::Install => {
            let target = with_notice(|n| {
                n.view.action_error = None;
                match (&n.status, n.step) {
                    (
                        UpdateStatus::Available {
                            installable: false, ..
                        },
                        _,
                    ) => Err(()),
                    (_, Some(step @ (Step::Download | Step::Install))) => Ok(Some(step)),
                    _ => Ok(None),
                }
            });
            match target {
                Some(Err(())) => open_url(RELEASES_URL),
                Some(Ok(Some(step))) => STATE.with(|s| {
                    let mut s = s.borrow_mut();
                    s.step = Some(step);
                    s.schedule.install_requested();
                }),
                _ => {}
            }
        }
        Action::Check => {
            with_notice(|n| n.view.action_error = None);
            STATE.with(|s| s.borrow_mut().step = Some(Step::Check));
        }
        Action::OpenPullRequest { url } => open_url(&url),
        Action::OpenDownloadPage => open_url(notice::DOWNLOAD_PAGE_URL),
        // Development-only acquisition; the Preview notice never offers it.
        Action::DownloadVerify | Action::CancelDownload => {}
    }
}

fn open_url(url: &str) {
    let opened = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            &HSTRING::from(url),
            None,
            None,
            SW_SHOWNORMAL,
        )
    };
    if opened.0 as usize <= 32 {
        eprintln!("Could not open {url}");
    }
}

// ---------------------------------------------------------- automation ----

/// The notice's parts for UI Automation, in shipping's DOM order.
pub fn uia_snapshot() -> Option<panel_uia::Snapshot> {
    with_notice(|n| panel_uia::Snapshot {
        hwnd: n.hwnd,
        scale: n.scale,
        size: (n.placement.width as f32, n.placement.height as f32),
        name: TITLE,
        focusable: false,
        items: items(&n.parts),
    })
}

fn items(parts: &[Part]) -> Vec<panel_uia::Item> {
    parts
        .iter()
        .map(|part| {
            let mut item = panel_uia::Item::new(part.key, part.role, &part.name, part.rect);
            item.enabled = part.enabled;
            item
        })
        .collect()
}

/// Invoke a button through the same path as a click.
pub fn uia_act(key: u32, act_: panel_uia::Act) -> Option<()> {
    let action = with_notice(|n| {
        n.parts
            .iter()
            .find(|part| part.key == key && part.enabled)
            .and_then(|part| part.action.clone())
    })?;
    if let (Some(action), panel_uia::Act::Invoke) = (action, act_) {
        act(action);
    }
    Some(())
}

// -------------------------------------------------------------- drawing ----

/// One accessible or clickable part of the drawn card, in window DIPs.
#[derive(Clone, Debug)]
pub struct Part {
    pub key: u32,
    pub role: Role,
    pub name: String,
    pub rect: D2D_RECT_F,
    pub action: Option<Action>,
    pub enabled: bool,
}

/// Transient drawing state.
pub struct Ui {
    pub hover: Option<u32>,
    /// The notes list's scroll offset.
    pub scroll: f32,
    /// Seconds since the notice opened, for the spinner and sliding bar.
    pub time: f32,
    /// The restart bar's remaining fraction.
    pub restart: f32,
    /// The version link's hover, eased: 0 at rest, 1 hovered.
    pub link: f32,
}

pub struct Painted {
    pub parts: Vec<Part>,
    pub scroll_max: f32,
    /// The notes scroller, for the wheel.
    pub notes: Option<D2D_RECT_F>,
    /// The version link's part.
    pub link: Option<u32>,
    /// An update's card height for its content: the header, the list at its
    /// natural height and the footer bar. `None` on other cards.
    pub fitted: Option<f32>,
}

/// Wrapped text at `size`/`weight`, with a CSS `line-height` multiplier.
pub(crate) fn para(
    g: &Gfx,
    t: &Tokens,
    copy: &str,
    (size, weight): (f32, &str),
    tracking: Option<&str>,
    leading: Option<f32>,
    width: f32,
) -> Result<Text> {
    let text = g.text_sized(t, copy, size, weight, tracking)?;
    unsafe {
        if let Some(leading) = leading {
            // CSS line boxes split the leading evenly above and below.
            let mut lines = [DWRITE_LINE_METRICS::default()];
            let mut count = 0;
            text.layout.GetLineMetrics(Some(&mut lines), &mut count)?;
            let line = leading * size;
            text.layout.SetLineSpacing(
                DWRITE_LINE_SPACING_METHOD_UNIFORM,
                line,
                (line - lines[0].height) / 2. + lines[0].baseline,
            )?;
        }
        text.layout.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP)?;
        text.layout.SetMaxWidth(width.max(1.))?;
        let mut metrics = DWRITE_TEXT_METRICS::default();
        text.layout.GetMetrics(&mut metrics)?;
        Ok(Text {
            layout: text.layout,
            width: metrics.width,
            height: metrics.height,
            units: text.units,
        })
    }
}

fn utf16_range(copy: &str, start: usize, len: usize) -> DWRITE_TEXT_RANGE {
    DWRITE_TEXT_RANGE {
        startPosition: copy[..start].encode_utf16().count() as u32,
        length: copy[start..start + len].encode_utf16().count() as u32,
    }
}

/// The layout-relative boxes of `range`.
fn range_rects(text: &Text, range: DWRITE_TEXT_RANGE) -> Vec<D2D_RECT_F> {
    let mut count = 0;
    unsafe {
        let _ = text.layout.HitTestTextRange(
            range.startPosition,
            range.length,
            0.,
            0.,
            None,
            &mut count,
        );
        let mut out = vec![DWRITE_HIT_TEST_METRICS::default(); count as usize];
        if text
            .layout
            .HitTestTextRange(
                range.startPosition,
                range.length,
                0.,
                0.,
                Some(&mut out),
                &mut count,
            )
            .is_err()
        {
            return Vec::new();
        }
        out.truncate(count as usize);
        out.iter()
            .map(|m| rect(m.left, m.top, m.left + m.width, m.top + m.height))
            .collect()
    }
}

/// Text with an underlined link range in its own color.
fn link_text(
    dc: &ID2D1DeviceContext,
    text: &Text,
    range: DWRITE_TEXT_RANGE,
    (x, y): (f32, f32),
    (color, link): (D2D1_COLOR_F, D2D1_COLOR_F),
) -> Result<()> {
    unsafe {
        text.layout.SetUnderline(true, range)?;
        let brush: IUnknown = gfx::brush(dc, link)?.cast()?;
        text.layout.SetDrawingEffect(&brush, range)?;
    }
    gfx::text(dc, text, x, y, color)
}

/// The first line's baseline, from the layout's top.
fn first_baseline(text: &Text) -> f32 {
    let mut count = 0;
    unsafe {
        let _ = text.layout.GetLineMetrics(None, &mut count);
        let mut lines = vec![DWRITE_LINE_METRICS::default(); count.max(1) as usize];
        if text
            .layout
            .GetLineMetrics(Some(&mut lines), &mut count)
            .is_err()
        {
            return text.height;
        }
        lines[0].baseline
    }
}

/// The bounding box of `boxes`.
fn union(boxes: impl Iterator<Item = D2D_RECT_F>) -> Option<D2D_RECT_F> {
    boxes.reduce(|a, b| {
        rect(
            a.left.min(b.left),
            a.top.min(b.top),
            a.right.max(b.right),
            a.bottom.max(b.bottom),
        )
    })
}

fn mix(a: D2D1_COLOR_F, b: D2D1_COLOR_F, amount: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: a.r + (b.r - a.r) * amount,
        g: a.g + (b.g - a.g) * amount,
        b: a.b + (b.b - a.b) * amount,
        a: a.a + (b.a - a.a) * amount,
    }
}

fn fade(mut color: D2D1_COLOR_F, enabled: bool) -> D2D1_COLOR_F {
    if !enabled {
        // `button:disabled { opacity: 0.45 }`
        color.a *= 0.45;
    }
    color
}

fn offset(r: D2D_RECT_F, x: f32, y: f32) -> D2D_RECT_F {
    rect(r.left + x, r.top + y, r.right + x, r.bottom + y)
}

/// `.update-spinner`: a 2 px ring with its right quarter open, turning.
fn spinner(
    dc: &ID2D1DeviceContext,
    g: &Gfx,
    (cx, cy, radius): (f32, f32, f32),
    turn: f32,
    color: D2D1_COLOR_F,
) -> Result<()> {
    let at = |angle: f32| Vector2 {
        X: cx + radius * angle.cos(),
        Y: cy + radius * angle.sin(),
    };
    // Three quarters of a ring from 45° past the turn (the right quarter
    // open at rest), flattened like the icon paths.
    let start = std::f32::consts::FRAC_PI_4 + turn;
    let points: Vec<Vector2> = (0..=36)
        .map(|step| at(start + step as f32 / 36. * std::f32::consts::PI * 1.5))
        .collect();
    unsafe {
        let geometry = g.factory.CreatePathGeometry()?;
        let sink = geometry.Open()?;
        sink.BeginFigure(points[0], D2D1_FIGURE_BEGIN_HOLLOW);
        sink.AddLines(&points[1..]);
        sink.EndFigure(D2D1_FIGURE_END_OPEN);
        sink.Close()?;
        dc.DrawGeometry(&geometry, &gfx::brush(dc, color)?, 2., None);
    }
    Ok(())
}

/// `.update-progress`: a 4 px pill on the canvas color with an info fill.
/// `None` is the indeterminate 34% segment sliding at `time`.
fn progress(
    dc: &ID2D1DeviceContext,
    t: &Tokens,
    bar: D2D_RECT_F,
    fraction: Option<f32>,
    time: f32,
) -> Result<()> {
    let radius = (bar.bottom - bar.top) / 2.;
    gfx::fill_round(dc, bar, radius, t.color("surface-canvas"))?;
    let width = bar.right - bar.left;
    let (left, right) = match fraction {
        Some(fraction) => (bar.left, bar.left + width * fraction.clamp(0., 1.)),
        None => {
            // `update-progress-slide`: margin-left -35% → 68% → -35%, eased.
            let phase = (time / 1.25).fract();
            let swing = 0.5 - 0.5 * (phase * std::f32::consts::TAU).cos();
            let left = bar.left + width * (-0.35 + 1.03 * swing);
            (left, left + width * 0.34)
        }
    };
    let (left, right) = (left.max(bar.left), right.min(bar.right));
    if right > left {
        unsafe {
            dc.PushAxisAlignedClip(&bar, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
        }
        let result = gfx::fill_round(
            dc,
            rect(left, bar.top, right, bar.bottom),
            radius,
            t.color("info"),
        );
        unsafe { dc.PopAxisAlignedClip() };
        result?;
    }
    Ok(())
}

/// A laid-out release note: a highlight's title, its text (with the pull
/// request appended) and the link's range.
struct NoteLine {
    title: Option<Text>,
    text: Text,
    link: Option<(DWRITE_TEXT_RANGE, u64, String)>,
    /// An older release's version after the title (or text): its text and
    /// widened-arrow ranges and its release URL.
    tag: Option<Tag>,
}

struct Tag {
    text: DWRITE_TEXT_RANGE,
    arrow: DWRITE_TEXT_RANGE,
    name: String,
    url: String,
}

/// `copy` with `tag` and a ↗ (widened to hold the raised link icon) after it.
fn tag_copy(copy: &str, tag: &notice::VersionLink) -> (String, Tag) {
    let tagged = format!("{copy} {}\u{2197}", tag.text);
    let at = copy.len() + 1;
    let tag = Tag {
        text: utf16_range(&tagged, at, tag.text.len()),
        arrow: utf16_range(&tagged, at + tag.text.len(), '\u{2197}'.len_utf8()),
        name: tag.text.clone(),
        url: tag.url.clone(),
    };
    (tagged, tag)
}

/// The tag's small regular face, and the 11-DIP gap and icon in its arrow.
fn style_tag(text: &mut Text, tag: &Tag, size: f32) -> Result<()> {
    let both = DWRITE_TEXT_RANGE {
        startPosition: tag.text.startPosition,
        length: tag.text.length + tag.arrow.length,
    };
    unsafe {
        text.layout.SetFontSize(size, both)?;
        text.layout.SetFontWeight(DWRITE_FONT_WEIGHT_NORMAL, both)?;
        let layout: IDWriteTextLayout1 = text.layout.cast()?;
        layout.SetCharacterSpacing(0., 0., 11., tag.arrow)?;
        let mut metrics = DWRITE_TEXT_METRICS::default();
        text.layout.GetMetrics(&mut metrics)?;
        (text.width, text.height) = (metrics.width, metrics.height);
    }
    Ok(())
}

/// Colour a tag (underlined on hover), hide its ↗ and draw the raised
/// link icon at its top right instead.
fn paint_tag(
    dc: &ID2D1DeviceContext,
    g: &Gfx,
    text: &Text,
    tag: &Tag,
    (x, y): (f32, f32),
    (color, hovered): (D2D1_COLOR_F, bool),
) -> Result<()> {
    unsafe {
        let brush: IUnknown = gfx::brush(dc, color)?.cast()?;
        text.layout.SetDrawingEffect(&brush, tag.text)?;
        let clear: IUnknown = gfx::brush(dc, D2D1_COLOR_F::default())?.cast()?;
        text.layout.SetDrawingEffect(&clear, tag.arrow)?;
        text.layout.SetUnderline(hovered, tag.text)?;
    }
    if let Some(b) = range_rects(text, tag.text).last() {
        let b = offset(*b, x, y);
        gfx::icon(
            dc,
            &g.factory,
            "release-link",
            &[],
            (b.right + 2., b.top + 1., 9.),
            2.,
            color,
        )?;
    }
    Ok(())
}

/// One `.update-notes-group`: its optional `h3` and items.
struct Group {
    heading: Option<Text>,
    items: Vec<NoteLine>,
    empty: Option<Text>,
}

/// Paint the notice in window-local DIPs for `placement`.
pub fn paint(
    dc: &ID2D1DeviceContext,
    g: &Gfx,
    t: &Tokens,
    p: &Presentation,
    placement: &Placement,
    ui: &Ui,
) -> Result<Painted> {
    let n = |name: &str| t.n(name);
    let c = |name: &str| t.color(name);
    let (s3, s4, s5, s6) = (n("s-3"), n("s-4"), n("s-5"), n("s-6"));
    let (sm, xxs) = (n("text-sm"), n("text-2xs"));
    // An update's own cards: no icon, one list under a hairline, and a
    // darker footer bar holding the restart note, progress and buttons.
    let update = p.icon == Icon::None;
    let s1 = n("s-1");
    // A highlight: its title, `s-1`, then its description. Highlights sit
    // `s-4` from their neighbours, plain bullets `s-3`.
    let item_height = |item: &NoteLine| {
        item.title.as_ref().map_or(0., |title| title.height + s1) + item.text.height
    };
    let item_gap = |items: &[NoteLine], i: usize| match i {
        0 => 0.,
        _ if items[i - 1].title.is_some() || items[i].title.is_some() => s4,
        _ => s3,
    };
    let (snug, tight) = (n("leading-snug"), n("leading-tight"));
    let mut parts: Vec<Part> = Vec::new();
    let mut part = |role: Role, name: &str, r: D2D_RECT_F, action: Option<Action>, enabled| {
        let key = parts.len() as u32;
        parts.push(Part {
            key,
            role,
            name: name.to_owned(),
            rect: r,
            action,
            enabled,
        });
        key
    };
    let hovered = |key: u32| ui.hover == Some(key);

    // `.tray-notice-card`: shadow, raised fill, 1 px border, caret.
    let card = placement.card_rect();
    let card = rect(
        card.x as f32,
        card.y as f32,
        (card.x + card.width) as f32,
        (card.y + card.height) as f32,
    );
    let radius = n("r-xl");
    gfx::shadow(dc, card, radius, t.shadow("shadow-md"))?;
    gfx::fill_round(dc, card, radius, c("surface-raised"))?;
    gfx::border(dc, card, radius, 1., c("border"))?;
    if let Some([tip, a, b]) = placement.caret_triangle(notice::CARET_SPAN) {
        let d = format!("M{} {}L{} {}L{} {}Z", tip.0, tip.1, a.0, a.1, b.0, b.1);
        gfx::path(
            dc,
            &g.factory,
            &d,
            true,
            (0., 0., 1.),
            0.,
            // Under an update's card the caret continues its footer bar.
            c(
                if p.icon == Icon::None && placement.caret == Caret::Bottom {
                    "surface-sunken"
                } else {
                    "surface-raised"
                },
            ),
        )?;
    }
    // The grid inside the border.
    let (left, right) = (card.left + 1., card.right - 1.);
    let (top, bottom) = (card.top + 1., card.bottom - 1.);
    let width = right - left;
    let inner = width - 2. * s6;

    // ---- measure ----
    // `.update-notice-header`: 36 px icon and copy.
    let copy_width = if update { inner } else { inner - 36. - s4 };
    let title = para(
        g,
        t,
        &p.title,
        (n("text-lg"), "weight-semibold"),
        Some("tracking-tight"),
        Some(tight),
        copy_width,
    )?;
    // The version link: its text, then a ↗ widened to hold the place of
    // Caper's external-link icon (`release-link`): a 4-DIP gap and the
    // centred 12-DIP icon, or on an update's card a 2-DIP gap and a 10-DIP
    // icon raised to the text's top right.
    let (link_gap, link_side) = if update { (2f32, 10f32) } else { (4., 12.) };
    let link = p.version_link.as_ref().and_then(|link| {
        let at = p.description.find(&link.text)?;
        Some((at, at + link.text.len(), link))
    });
    let description_copy = match link {
        Some((_, end, _)) => format!("{}\u{2197}{}", &p.description[..end], &p.description[end..]),
        None => p.description.clone(),
    };
    let mut description = para(
        g,
        t,
        &description_copy,
        (sm, "weight-regular"),
        None,
        None,
        copy_width,
    )?;
    // (the link's text, the text with its arrow, the arrow, the link)
    let link = link.map(|(at, end, link)| {
        let arrow = '\u{2197}'.len_utf8();
        (
            utf16_range(&description_copy, at, end - at),
            utf16_range(&description_copy, at, end + arrow - at),
            utf16_range(&description_copy, end, arrow),
            link,
        )
    });
    if let Some((_, _, arrow, _)) = &link {
        unsafe {
            let layout: IDWriteTextLayout1 = description.layout.cast()?;
            // Exactly the gap and icon, whatever the font's ↗ measures.
            layout.SetCharacterSpacing(0., 0., link_gap + link_side, *arrow)?;
            let mut metrics = DWRITE_TEXT_METRICS::default();
            description.layout.GetMetrics(&mut metrics)?;
            (description.width, description.height) = (metrics.width, metrics.height);
        }
    }
    let copy_height = title.height + 2. + description.height;
    let header_content = if update {
        copy_height
    } else {
        copy_height.max(36.)
    };
    let header = s6 + header_content + s5;

    // `.update-notice-body` blocks at the inner width.
    let boxed = inner - 2. * (s5 + 1.);
    let notes = match &p.notes {
        Some(notes) => {
            let heading = para(
                g,
                t,
                &notes.heading.to_uppercase(),
                (xxs, "weight-semibold"),
                Some("tracking-wide"),
                Some(tight),
                10_000.,
            )?;
            let intro = match &notes.intro {
                Some(intro) => Some(para(
                    g,
                    t,
                    intro,
                    (sm, "weight-regular"),
                    None,
                    Some(snug),
                    boxed,
                )?),
                None => None,
            };
            // "No highlights for Windows. See all changes on GitHub", the
            // link inline like the error's download fallback.
            let empty = match &notes.empty {
                Some(empty) => {
                    let copy = match &notes.empty_link {
                        Some(link) => format!("{empty} {}", link.text),
                        None => empty.clone(),
                    };
                    let text = para(g, t, &copy, (sm, "weight-regular"), None, Some(snug), boxed)?;
                    let range = notes
                        .empty_link
                        .as_ref()
                        .map(|link| utf16_range(&copy, empty.len() + 1, link.text.len()));
                    Some((text, range))
                }
                None => None,
            };
            // `.update-notes-scroll { padding-right: var(--s-1) }`.
            let list = boxed - if notes.stacked { n("s-1") } else { 0. };
            let mut groups = Vec::new();
            let list = if update { inner } else { list };
            for group in &notes.groups {
                let heading = if notes.stacked && !update {
                    Some(para(
                        g,
                        t,
                        &group.display_version,
                        (sm, "weight-semibold"),
                        Some("tracking-tight"),
                        Some(tight),
                        list,
                    )?)
                } else {
                    None
                };
                let mut items = Vec::new();
                for item in &group.items {
                    let tag_on_title = item.title.is_some();
                    let (copy, link) = match &item.pull_request {
                        Some(pull) => {
                            let label = format!("#{}", pull.number);
                            let copy = format!("{} {label}", item.text);
                            let range = utf16_range(&copy, item.text.len() + 1, label.len());
                            (copy, Some((range, pull.number, pull.url.clone())))
                        }
                        None => (item.text.clone(), None),
                    };
                    let (copy, mut tag) = match &group.tag {
                        Some(version) if !tag_on_title => {
                            let (copy, tag) = tag_copy(&copy, version);
                            (copy, Some(tag))
                        }
                        _ => (copy, None),
                    };
                    let mut text = para(
                        g,
                        t,
                        &copy,
                        (sm, "weight-regular"),
                        None,
                        Some(snug),
                        list - s5,
                    )?;
                    if let Some(tag) = &tag {
                        style_tag(&mut text, tag, n("text-xs"))?;
                    }
                    let title = match &item.title {
                        Some(title) => {
                            let (copy, title_tag) = match &group.tag {
                                Some(version) => {
                                    let (copy, tag) = tag_copy(title, version);
                                    (copy, Some(tag))
                                }
                                None => (title.clone(), None),
                            };
                            let mut text = para(
                                g,
                                t,
                                &copy,
                                (sm, "weight-semibold"),
                                None,
                                Some(snug),
                                list - s5,
                            )?;
                            if let Some(title_tag) = title_tag {
                                style_tag(&mut text, &title_tag, n("text-xs"))?;
                                tag = Some(title_tag);
                            }
                            Some(text)
                        }
                        None => None,
                    };
                    items.push(NoteLine {
                        title,
                        text,
                        link,
                        tag,
                    });
                }
                let empty = if group.items.is_empty() {
                    Some(para(
                        g,
                        t,
                        &notes.group_empty,
                        (sm, "weight-regular"),
                        None,
                        Some(snug),
                        list,
                    )?)
                } else {
                    None
                };
                groups.push(Group {
                    heading,
                    items,
                    empty,
                });
            }
            Some((notes, heading, intro, empty, groups))
        }
        None => None,
    };
    let download = match &p.download {
        Some(download) => {
            let label = para(
                g,
                t,
                &download.label,
                (sm, "weight-regular"),
                None,
                None,
                10_000.,
            )?;
            let detail = para(
                g,
                t,
                &download.detail,
                (sm, "weight-regular"),
                None,
                None,
                10_000.,
            )?;
            let percent = match &download.percent_label {
                Some(percent) => Some(para(
                    g,
                    t,
                    percent,
                    (sm, "weight-bold"),
                    None,
                    None,
                    10_000.,
                )?),
                None => None,
            };
            Some((download, label, detail, percent))
        }
        None => None,
    };
    let restart = match &p.restart {
        Some(restart) => Some(para(
            g,
            t,
            &restart.message,
            (sm, "weight-regular"),
            None,
            Some(snug),
            boxed,
        )?),
        None => None,
    };
    let error = match &p.error {
        Some(error) => {
            let message = para(
                g,
                t,
                &error.message,
                (sm, "weight-regular"),
                None,
                Some(snug),
                boxed,
            )?;
            let copy = format!(
                "{}{}{}",
                error.fallback_prefix, error.fallback_link, error.fallback_suffix
            );
            let fallback = para(g, t, &copy, (sm, "weight-regular"), None, Some(snug), inner)?;
            let range = (!error.fallback_link.is_empty()).then(|| {
                utf16_range(
                    &copy,
                    error.fallback_prefix.len(),
                    error.fallback_link.len(),
                )
            });
            Some((message, fallback, range, error.fallback_link.clone()))
        }
        None => None,
    };
    let status = match &p.status_message {
        Some(message) => Some((
            message,
            para(
                g,
                t,
                message,
                (sm, "weight-regular"),
                None,
                Some(snug),
                inner,
            )?,
        )),
        None => None,
    };
    let sunken = |content: f32| content + 2. * (s5 + 1.);
    let bar = 4.;
    let mut body_natural = 0.;
    if let Some((_, label, ..)) = download.as_ref().filter(|_| !update) {
        body_natural += sunken(label.height + s4 + bar);
    }
    if let Some(message) = restart.as_ref().filter(|_| !update) {
        body_natural += sunken(message.height + bar);
    }
    if let Some((message, fallback, ..)) = &error {
        body_natural += sunken(message.height) + s3 + fallback.height;
    }
    if let Some((_, text)) = &status {
        body_natural += text.height;
    }
    let has_body = notes.is_some() || body_natural > 0.;
    let body_natural = if has_body { body_natural + s4 } else { 0. };

    // The footer bar's note: the 12-DIP restart icon beside wrapped copy.
    let note = match &p.footer_note {
        Some(note) => Some((
            note,
            para(
                g,
                t,
                note,
                (sm, "weight-regular"),
                None,
                Some(snug),
                inner - 12. - s3,
            )?,
        )),
        None => None,
    };

    // `.update-notice-footer`: Later/Close, then the primary.
    let footer_natural = if update {
        // The footer bar: note, progress or buttons, between its padding.
        let mut h = s5 + s6;
        if let Some((_, text)) = &note {
            h += text.height + s4;
        }
        if p.footer.is_some() {
            h += n("h-md");
        }
        if let Some((_, label, ..)) = &download {
            h += label.height + s4 + bar;
        }
        if let Some(message) = &restart {
            h += message.height + s4 + bar;
        }
        h
    } else if p.footer.is_some() {
        56f32.max(s4 + n("h-md") + s6)
    } else {
        0.
    };

    // ---- rows ----
    // Shipping's four grid rows: compact cards share the spare height
    // between every track (empty ones too); with notes or progress, the body
    // takes it, so the header stays put while an update's card holds its
    // Available height (`ViewState::follow`).
    let content = bottom - top;
    let (header_h, body_h, footer_h) =
        if update || notes.is_some() || download.is_some() || restart.is_some() {
            let body = (content - header - footer_natural).max(0.);
            (header, body, footer_natural)
        } else {
            let used = header + body_natural + footer_natural;
            let extra = (content - used).max(0.) / 3.;
            let grow = |h: f32, present: bool| if present { h + extra } else { h };
            (
                header + extra,
                grow(body_natural, has_body),
                grow(footer_natural, p.footer.is_some()),
            )
        };
    let bar_top = top + header_h + body_h;
    if update {
        // The bar's sunken fill follows the card's bottom corners.
        let fill = rect(left, bar_top, right, bottom);
        unsafe { dc.PushAxisAlignedClip(&fill, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE) };
        let filled = gfx::fill_round(
            dc,
            rect(left, top, right, bottom),
            radius - 1.,
            c("surface-sunken"),
        );
        unsafe { dc.PopAxisAlignedClip() };
        filled?;
        gfx::fill(
            dc,
            rect(left, bar_top, right, bar_top + 1.),
            c("border-subtle"),
        )?;
    }

    // ---- header ----
    let y0 = top;
    let mid = y0 + s6 + (header_h - s6 - s5) / 2.;
    let icon = rect(left + s6, mid - 18., left + s6 + 36., mid + 18.);
    if !update {
        let (fill, ink) = match p.icon_tone {
            IconTone::Accent => ("theme-accent", "theme-accent-ink"),
            IconTone::Positive => ("positive", "positive-ink"),
            IconTone::Signal => ("theme-signal", "theme-signal-ink"),
            IconTone::Neutral => ("surface-sunken", "text"),
        };
        gfx::fill_round(dc, icon, n("r-lg"), c(fill))?;
        if p.icon_tone == IconTone::Neutral {
            gfx::border(dc, icon, n("r-lg"), 1., c("border"))?;
        }
        let (cx, cy) = (icon.left + 18., mid);
        match p.icon {
            Icon::None => {}
            Icon::Spinner => spinner(
                dc,
                g,
                (cx, cy, 6.),
                ui.time * std::f32::consts::TAU / 0.72,
                c(ink),
            )?,
            icon => {
                let (name, filled): (&str, &[usize]) = match icon {
                    Icon::Check => ("check", &[]),
                    Icon::Warning => ("warning", &[]),
                    _ => ("capture", &[1]),
                };
                gfx::icon(
                    dc,
                    &g.factory,
                    name,
                    filled,
                    (cx - 9., cy - 9., 18.),
                    1.8,
                    c(ink),
                )?;
            }
        }
    }
    let copy_x = if update { left + s6 } else { icon.right + s4 };
    let copy_y = mid - copy_height / 2.;
    gfx::text(dc, &title, copy_x, copy_y, c("text"))?;
    let desc_y = copy_y + title.height + 2.;
    let mut version_link = None;
    if let Some((text_range, range, arrow, _)) = &link {
        // A plain external link: the description's colour at rest, fading
        // to the text colour with an underline on hover.
        let color = mix(c("text-subtle"), c("text"), ui.link);
        unsafe {
            let brush: IUnknown = gfx::brush(dc, color)?.cast()?;
            description.layout.SetDrawingEffect(&brush, *text_range)?;
            let clear: IUnknown = gfx::brush(dc, D2D1_COLOR_F::default())?.cast()?;
            description.layout.SetDrawingEffect(&clear, *arrow)?;
        }
        // Caper's icon after the version's last line: centred, or raised to
        // its top right on an update's card.
        if let Some(b) = range_rects(&description, *text_range).last() {
            let b = offset(*b, copy_x, desc_y);
            let side = link_side.min(b.bottom - b.top);
            let y = if update {
                b.top + 1.
            } else {
                (b.top + b.bottom - side) / 2.
            };
            let x = b.right + link_gap;
            gfx::icon(dc, &g.factory, "release-link", &[], (x, y, side), 2., color)?;
        }
        if ui.link > 0. {
            let baseline = first_baseline(&description);
            let mut line = c("text");
            line.a *= ui.link;
            for b in range_rects(&description, *text_range) {
                let b = offset(b, copy_x, desc_y);
                let y = b.top + baseline + 1.;
                gfx::fill(dc, rect(b.left, y, b.right, y + 1.), line)?;
            }
        }
        version_link = union(
            range_rects(&description, *range)
                .into_iter()
                .map(|b| offset(b, copy_x, desc_y)),
        );
    }
    gfx::text(dc, &description, copy_x, desc_y, c("text-subtle"))?;
    // `role="dialog"` with `aria-labelledby`/`aria-describedby`.
    part(
        Role::Heading(1),
        &p.title,
        rect(copy_x, copy_y, copy_x + title.width, copy_y + title.height),
        None,
        true,
    );
    part(
        Role::Text,
        &p.description,
        rect(
            copy_x,
            desc_y,
            copy_x + description.width,
            desc_y + description.height,
        ),
        None,
        true,
    );
    let version_link = match (version_link, &link) {
        (Some(hit), Some((_, _, _, link))) => Some(part(
            Role::Link,
            &format!("Open release {}", link.text),
            hit,
            // Any external page: the same `open_url` as a pull request.
            Some(Action::OpenPullRequest {
                url: link.url.clone(),
            }),
            true,
        )),
        _ => None,
    };

    // ---- body ----
    let body_top = y0 + header_h;
    let mut y = body_top;
    let (bx0, bx1) = (left + s6, right - s6);
    let mut scroll_max = 0.;
    let mut notes_view = None;
    // An update's body at its content's natural height, for `Painted::fitted`.
    let mut body_need = 0.;
    if let Some((notes, heading, intro, empty, groups)) = &notes {
        let (section, ix, mut iy) = if update {
            // One list under a hairline, straight on the card.
            let section = rect(bx0 - 1. - s5, y, bx1 + 1. + s5, body_top + body_h);
            gfx::fill(dc, rect(bx0, y, bx1, y + 1.), c("border-subtle"))?;
            (section, bx0, y + 1. + s5)
        } else {
            let section = rect(bx0, y, bx1, body_top + body_h - s4);
            gfx::fill_round(dc, section, n("r-lg"), c("surface-sunken"))?;
            gfx::border(dc, section, n("r-lg"), 1., c("border-subtle"))?;
            let ix = bx0 + 1. + s5;
            let iy = y + 1. + s5;
            let row = heading.height;
            gfx::text(
                dc,
                heading,
                ix,
                iy + (row - heading.height) / 2.,
                c("text-subtle"),
            )?;
            part(
                Role::Heading(2),
                &notes.heading,
                rect(ix, iy, ix + heading.width, iy + row),
                None,
                true,
            );
            (section, ix, iy + row + s4)
        };
        if let Some(intro) = intro {
            gfx::text(dc, intro, ix, iy, c("text-subtle"))?;
            part(
                Role::Text,
                notes.intro.as_deref().unwrap_or_default(),
                rect(ix, iy, ix + intro.width, iy + intro.height),
                None,
                true,
            );
            iy += intro.height + s4;
        }
        if let Some((empty, _)) = empty {
            body_need = iy - body_top + empty.height + 1. + s5;
        }
        if let Some((empty, range)) = empty {
            part(
                Role::Text,
                notes.empty.as_deref().unwrap_or_default(),
                rect(ix, iy, ix + empty.width, iy + empty.height),
                None,
                true,
            );
            match (range, &notes.empty_link) {
                (Some(range), Some(link)) => {
                    let mut color = "text";
                    let hit = union(
                        range_rects(empty, *range)
                            .into_iter()
                            .map(|b| offset(b, ix, iy)),
                    );
                    if let Some(hit) = hit {
                        let key = part(
                            Role::Link,
                            &link.text,
                            hit,
                            // Any external page: the same `open_url` as a pull request.
                            Some(Action::OpenPullRequest {
                                url: link.url.clone(),
                            }),
                            true,
                        );
                        if hovered(key) {
                            color = "text-muted";
                        }
                    }
                    link_text(dc, empty, *range, (ix, iy), (c("text-subtle"), c(color)))?;
                }
                _ => gfx::text(dc, empty, ix, iy, c("text-subtle"))?,
            }
        } else {
            // The scroller: the rest of the section, clipped.
            // An update's list spans the inner width, as its text was laid out.
            let view_right = if update { bx1 } else { bx1 - 1. - s5 };
            let view = rect(ix, iy, view_right, section.bottom - 1. - s5);
            // One list continues across releases without a rule between them.
            let group_gap = if update { s4 } else { s5 + 1. + s4 };
            let mut natural = 0.;
            for (index, group) in groups.iter().enumerate() {
                if index > 0 {
                    natural += group_gap;
                }
                if let Some(h3) = &group.heading {
                    natural += h3.height + s3;
                }
                for (i, item) in group.items.iter().enumerate() {
                    natural += item_gap(&group.items, i) + item_height(item);
                }
                if let Some(empty) = &group.empty {
                    natural += empty.height;
                }
            }
            scroll_max = (natural - (view.bottom - view.top)).max(0.);
            body_need = iy - body_top + natural + 1. + s5;
            let scroll = ui.scroll.clamp(0., scroll_max);
            notes_view = Some(view);
            unsafe { dc.PushAxisAlignedClip(&view, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE) };
            let mut gy = view.top - scroll;
            let visible = |r: D2D_RECT_F| r.bottom > view.top && r.top < view.bottom;
            let mut result = Ok(());
            for (index, (group, source)) in groups.iter().zip(&notes.groups).enumerate() {
                if index > 0 && update {
                    gy += group_gap;
                } else if index > 0 {
                    gy += s5;
                    result = result.and(gfx::fill(
                        dc,
                        rect(view.left, gy, view.right, gy + 1.),
                        c("border"),
                    ));
                    gy += 1. + s4;
                }
                if let Some(h3) = &group.heading {
                    result = result.and(gfx::text(dc, h3, view.left, gy, c("text")));
                    let r = rect(view.left, gy, view.left + h3.width, gy + h3.height);
                    if visible(r) {
                        part(Role::Heading(3), &source.display_version, r, None, true);
                    }
                    gy += h3.height + s3;
                }
                for (i, (item, note)) in group.items.iter().zip(&source.items).enumerate() {
                    gy += item_gap(&group.items, i);
                    // `li::before`: a 5 px dot at 0.5em.
                    let dot = gy + sm / 2. + 2.5;
                    result = result.and(gfx::fill_round(
                        dc,
                        rect(view.left, dot - 2.5, view.left + 5., dot + 2.5),
                        2.5,
                        c("text-faint"),
                    ));
                    let x = view.left + s5;
                    // An older release's version tag, on its title or text.
                    macro_rules! tagged {
                        ($text:expr, $y:expr) => {
                            if let Some(tag) = &item.tag {
                                let all = DWRITE_TEXT_RANGE {
                                    startPosition: tag.text.startPosition,
                                    length: tag.text.length + tag.arrow.length,
                                };
                                let hit = union(
                                    range_rects($text, all)
                                        .into_iter()
                                        .map(|b| offset(b, x, $y)),
                                )
                                .filter(|hit| visible(*hit));
                                let hover = hit.is_some_and(|hit| {
                                    hovered(part(
                                        Role::Link,
                                        &format!("Open release {}", tag.name),
                                        hit,
                                        Some(Action::OpenPullRequest {
                                            url: tag.url.clone(),
                                        }),
                                        true,
                                    ))
                                });
                                let color = c(if hover { "text" } else { "text-faint" });
                                result = result.and(paint_tag(
                                    dc,
                                    g,
                                    $text,
                                    tag,
                                    (x, $y),
                                    (color, hover),
                                ));
                            }
                        };
                    }
                    if let (Some(title), Some(copy)) = (&item.title, &note.title) {
                        tagged!(title, gy);
                        result = result.and(gfx::text(dc, title, x, gy, c("text")));
                        let r = rect(x, gy, x + title.width, gy + title.height);
                        if visible(r) {
                            part(Role::Text, copy, r, None, true);
                        }
                        gy += title.height + s1;
                    }
                    let r = rect(x, gy, x + item.text.width, gy + item.text.height);
                    if visible(r) {
                        part(Role::Text, &note.text, r, None, true);
                    }
                    if item.title.is_none() {
                        tagged!(&item.text, gy);
                    }
                    match &item.link {
                        Some((range, number, url)) => {
                            let boxes = range_rects(&item.text, *range);
                            let hit = boxes.iter().fold(None::<D2D_RECT_F>, |acc, b| {
                                let b = offset(*b, x, gy);
                                Some(acc.map_or(b, |a| {
                                    rect(
                                        a.left.min(b.left),
                                        a.top.min(b.top),
                                        a.right.max(b.right),
                                        a.bottom.max(b.bottom),
                                    )
                                }))
                            });
                            let mut link = "text";
                            if let Some(hit) = hit.filter(|hit| visible(*hit)) {
                                let key = part(
                                    Role::Button,
                                    &format!("Open pull request {number}"),
                                    hit,
                                    Some(Action::OpenPullRequest { url: url.clone() }),
                                    true,
                                );
                                if hovered(key) {
                                    link = "text-subtle";
                                }
                            }
                            result = result.and(link_text(
                                dc,
                                &item.text,
                                *range,
                                (x, gy),
                                (c("text-muted"), c(link)),
                            ));
                        }
                        None => {
                            result = result.and(gfx::text(dc, &item.text, x, gy, c("text-muted")))
                        }
                    }
                    gy += item.text.height;
                }
                if let Some(empty) = &group.empty {
                    result = result.and(gfx::text(dc, empty, view.left, gy, c("text-subtle")));
                    gy += empty.height;
                }
            }
            unsafe { dc.PopAxisAlignedClip() };
            result?;
        }
    }
    let sunken_box = |y: f32, content: f32| -> Result<D2D_RECT_F> {
        let r = rect(bx0, y, bx1, y + sunken(content));
        gfx::fill_round(dc, r, n("r-lg"), c("surface-sunken"))?;
        gfx::border(dc, r, n("r-lg"), 1., c("border-subtle"))?;
        Ok(r)
    };
    // On an update's card, progress sits in the footer bar under its note.
    let (ix, iw) = if update {
        (bx0, inner)
    } else {
        (bx0 + 1. + s5, boxed)
    };
    let mut py = y;
    if update {
        py = bar_top + s5;
        if let Some((copy, text)) = &note {
            gfx::icon(
                dc,
                &g.factory,
                "restart",
                &[],
                (bx0, py + 2., 12.),
                2.,
                c("text-faint"),
            )?;
            gfx::text(dc, text, bx0 + 12. + s3, py, c("text-faint"))?;
            part(
                Role::Text,
                copy,
                rect(bx0, py, bx0 + 12. + s3 + text.width, py + text.height),
                None,
                true,
            );
            py += text.height + s4;
        }
    }
    let progress_box = |py: f32, content: f32| -> Result<(D2D_RECT_F, f32)> {
        if update {
            Ok((rect(bx0, py, bx1, py + content), py))
        } else {
            let r = sunken_box(py, content)?;
            Ok((r, r.top + 1. + s5))
        }
    };
    if let Some((download, label, detail, percent)) = &download {
        let (r, iy) = progress_box(py, label.height + s4 + bar)?;
        gfx::text(dc, label, ix, iy, c("text-subtle"))?;
        gfx::text(dc, detail, ix + label.width + n("s-2"), iy, c("text-faint"))?;
        if let Some(percent) = percent {
            gfx::text(dc, percent, ix + iw - percent.width, iy, c("text"))?;
        }
        let track = rect(
            ix,
            iy + label.height + s4,
            ix + iw,
            iy + label.height + s4 + bar,
        );
        progress(
            dc,
            t,
            track,
            download.percent.map(|p| f32::from(p) / 100.),
            ui.time,
        )?;
        // `role="progressbar"` with its `aria-valuetext`.
        part(
            Role::Status,
            &format!("Downloading update, {}", download.accessible_value),
            r,
            None,
            true,
        );
        py = r.bottom;
    }
    if let (Some(message), Some(source)) = (&restart, &p.restart) {
        let gap = if update { s4 } else { 0. };
        let (r, iy) = progress_box(py, message.height + gap + bar)?;
        gfx::text(dc, message, ix, iy, c("text-subtle"))?;
        // `update-restart-progress`: full, emptying over the countdown.
        let ty = iy + message.height + gap;
        let track = rect(ix, ty, ix + iw, ty + bar);
        gfx::fill_round(dc, track, bar / 2., c("surface-canvas"))?;
        let w = (track.right - track.left) * ui.restart;
        if w > 0. {
            gfx::fill_round(
                dc,
                rect(track.left, track.top, track.left + w, track.bottom),
                bar / 2.,
                c("info"),
            )?;
        }
        part(Role::Status, &source.message, r, None, true);
        py = r.bottom;
    }
    if !update {
        y = py;
    }
    if let (Some((message, fallback, range, link)), Some(source)) = (&error, &p.error) {
        let r = rect(bx0, y, bx1, y + sunken(message.height));
        gfx::fill_round(dc, r, n("r-lg"), c("danger-surface"))?;
        gfx::border(dc, r, n("r-lg"), 1., c("danger-border"))?;
        gfx::text(dc, message, ix, r.top + 1. + s5, c("danger-text"))?;
        part(Role::Alert, &source.message, r, None, true);
        let fy = r.bottom + s3;
        match range {
            Some(range) => {
                let hit = range_rects(fallback, *range)
                    .into_iter()
                    .map(|b| offset(b, bx0, fy))
                    .reduce(|a, b| {
                        rect(
                            a.left.min(b.left),
                            a.top.min(b.top),
                            a.right.max(b.right),
                            a.bottom.max(b.bottom),
                        )
                    });
                let mut color = "text";
                if let Some(hit) = hit {
                    let key = part(
                        Role::Button,
                        link,
                        hit,
                        Some(Action::OpenDownloadPage),
                        true,
                    );
                    if hovered(key) {
                        color = "text-muted";
                    }
                }
                link_text(
                    dc,
                    fallback,
                    *range,
                    (bx0, fy),
                    (c("text-subtle"), c(color)),
                )?;
            }
            None => gfx::text(dc, fallback, bx0, fy, c("text-subtle"))?,
        }
        y = fy + fallback.height;
    }
    if let Some((message, text)) = &status {
        gfx::text(dc, text, bx0, y, c("text-subtle"))?;
        part(
            Role::Status,
            message,
            rect(bx0, y, bx0 + text.width, y + text.height),
            None,
            true,
        );
    }

    // ---- footer ----
    if let Some(footer) = &p.footer {
        let h = n("h-md");
        let mid = if update {
            bottom - s6 - h / 2.
        } else {
            let fy0 = body_top + body_h;
            fy0 + s4 + (footer_h - s4 - s6) / 2.
        };
        let label = |button: &notice::Button, weight: &str| {
            para(g, t, &button.label, (sm, weight), None, None, 10_000.)
        };
        let dismiss = label(&footer.dismiss, "weight-medium")?;
        let dismiss_w = dismiss.width + 2. * s5 + 2.;
        let dismiss_r = if footer.primary.is_some() {
            rect(bx0, mid - h / 2., bx0 + dismiss_w, mid + h / 2.)
        } else {
            rect(bx1 - dismiss_w, mid - h / 2., bx1, mid + h / 2.)
        };
        let key = part(
            Role::Button,
            &footer.dismiss.label,
            dismiss_r,
            Some(footer.dismiss.action.clone()),
            footer.dismiss.enabled,
        );
        // `.update-dismiss`: ghost on the card color; hover mixes in text.
        let hover = hovered(key) && footer.dismiss.enabled;
        let raised = c("surface-raised");
        let fill = if hover {
            mix(raised, c("text"), 0.08)
        } else {
            raised
        };
        gfx::fill_round(dc, dismiss_r, n("r-md"), fade(fill, footer.dismiss.enabled))?;
        if update {
            // On the footer bar the raised button keeps its edge.
            gfx::border(dc, dismiss_r, n("r-md"), 1., c("border"))?;
        }
        gfx::text(
            dc,
            &dismiss,
            dismiss_r.left + 1. + s5,
            mid - dismiss.height / 2.,
            fade(
                c(if hover { "text" } else { "text-muted" }),
                footer.dismiss.enabled,
            ),
        )?;
        if let Some(primary) = &footer.primary {
            // `.primary`: the solid neutral action, at least 104 px.
            let text = label(primary, "weight-semibold")?;
            let icon_w = primary
                .trailing_icon
                .as_ref()
                .map_or(0., |_| s3 + gfx::TRAILING_ICON);
            let w = (text.width + icon_w + 2. * s5 + 2.).max(104.);
            let r = rect(bx1 - w, mid - h / 2., bx1, mid + h / 2.);
            let key = part(
                Role::Button,
                &primary.label,
                r,
                Some(primary.action.clone()),
                primary.enabled,
            );
            let fill = if hovered(key) && primary.enabled {
                "solid-hover"
            } else {
                "solid"
            };
            gfx::fill_round(dc, r, n("r-md"), fade(c(fill), primary.enabled))?;
            let ink = fade(c("solid-ink"), primary.enabled);
            let x = r.left + (w - text.width - icon_w) / 2.;
            gfx::text(dc, &text, x, mid - text.height / 2., ink)?;
            if let Some(icon) = &primary.trailing_icon {
                gfx::trailing_icon(dc, &g.factory, icon, (x + text.width + s3, mid), ink)?;
            }
        }
    }
    Ok(Painted {
        parts,
        scroll_max,
        notes: notes_view,
        link: version_link,
        // Inside the 1-DIP border.
        fitted: update.then_some(header_h + body_need + footer_h + 2.),
    })
}

// --------------------------------------------------------------- window ----

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_TIMER if wp.0 == ANIMATION_TIMER => {
                with_notice(|n| n.render_logged());
                LRESULT(0)
            }
            WM_TIMER if wp.0 == RETRY_TIMER => {
                with_notice(|n| n.retry());
                LRESULT(0)
            }
            WM_TIMER if wp.0 == ANNOUNCE_TIMER => {
                let _ = KillTimer(Some(hwnd), ANNOUNCE_TIMER);
                announce();
                LRESULT(0)
            }
            WM_GETOBJECT if lp.0 as i32 == UiaRootObjectId => UiaReturnRawElementProvider(
                hwnd,
                wp,
                lp,
                &panel_uia::node(panel_uia::Window::Update, None),
            ),
            WM_DESTROY => {
                let _ = UiaReturnRawElementProvider(hwnd, WPARAM(0), LPARAM(0), None);
                LRESULT(0)
            }
            // Never activates: a click works without taking focus.
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            WM_MOUSEMOVE | WM_LBUTTONDOWN | WM_LBUTTONUP => {
                let (x, y) = (lp.0 as u16 as i16 as f32, (lp.0 >> 16) as u16 as i16 as f32);
                let clicked = with_notice(|n| {
                    let (x, y) = (x / n.scale, y / n.scale);
                    let over = n.part_at(x, y).map(|part| part.key);
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
                        n.render_logged();
                    }
                    match msg {
                        WM_LBUTTONDOWN => {
                            n.pressed = over;
                            None
                        }
                        WM_LBUTTONUP if n.pressed.take() == over => over.and_then(|key| {
                            n.parts
                                .iter()
                                .find(|part| part.key == key && part.enabled)
                                .and_then(|part| part.action.clone())
                        }),
                        _ => None,
                    }
                })
                .flatten();
                if let Some(action) = clicked {
                    let before = panel_uia::watch(panel_uia::Window::Update);
                    act(action);
                    panel_uia::raise(panel_uia::Window::Update, before);
                }
                LRESULT(0)
            }
            WM_MOUSEWHEEL => {
                let delta = (wp.0 >> 16) as u16 as i16 as f32 / 120.;
                let mut point = POINT {
                    x: lp.0 as u16 as i16 as i32,
                    y: (lp.0 >> 16) as u16 as i16 as i32,
                };
                let _ = windows::Win32::Graphics::Gdi::ScreenToClient(hwnd, &mut point);
                with_notice(|n| {
                    let (x, y) = (point.x as f32 / n.scale, point.y as f32 / n.scale);
                    let inside = n
                        .notes
                        .is_some_and(|v| x >= v.left && x < v.right && y >= v.top && y < v.bottom);
                    if inside && n.scroll_max > 0. {
                        let lines = crate::appearance::wheel_lines().max(1.);
                        n.scroll = (n.scroll - delta * lines * 16.).clamp(0., n.scroll_max);
                        n.render_logged();
                    }
                });
                LRESULT(0)
            }
            WM_MOUSELEAVE => {
                with_notice(|n| {
                    n.tracking = false;
                    n.pressed = None;
                    if n.hover.take().is_some() {
                        n.render_logged();
                    }
                });
                LRESULT(0)
            }
            WM_SETCURSOR => {
                let hover = with_notice(|n| n.hover.is_some()).unwrap_or(false);
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

// ------------------------------------------------------------ offscreen ----

/// `--scene update --update NAME`: the shared fixture on the canvas, above a
/// tray icon at the bottom right (`--tool fallback`: no icon, the caretless
/// corner). `--select notes-hidden` collapses What's new.
pub fn render_offscreen(
    options: &crate::Options,
    g: Gfx,
    t: Tokens,
    size: (f32, f32),
) -> Result<impl Fn(&ID2D1DeviceContext) -> Result<()> + use<>> {
    let name = options.update.as_deref().unwrap_or("available");
    let status =
        notice::fixture(name).ok_or_else(|| Error::new(E_INVALIDARG, "unknown --update"))?;
    let view = ViewState {
        show_changelog: options.select.as_deref() != Some("notes-hidden"),
        ..ViewState::default()
    };
    let presentation = notice::present(Some(&status), &view);
    let (w, h) = (f64::from(size.0), f64::from(size.1));
    let screen = LogicalRect::new(0., 0., w, h);
    let work = LogicalRect::new(0., 0., w, h - 48.);
    let tray = (options.tool.as_deref() != Some("fallback"))
        .then(|| LogicalRect::new(w - 160., h - 44., 24., 40.));
    let place =
        move |height: f64| notice::placement(screen, work, tray, false, notice::CARD_WIDTH, height);
    let placement = place(presentation.card_height);
    debug_assert!(tray.is_none() || placement.caret == Caret::Bottom);
    Ok(move |dc: &ID2D1DeviceContext| {
        let ui = Ui {
            hover: None,
            scroll: 0.,
            time: 0.,
            restart: 1.,
            link: 0.,
        };
        // As the live notice: measure an update's card at the estimate
        // (clipped to nothing), then paint it at its fitted height.
        let nothing = rect(0., 0., 0., 0.);
        unsafe { dc.PushAxisAlignedClip(&nothing, D2D1_ANTIALIAS_MODE_ALIASED) };
        let measured = paint(dc, &g, &t, &presentation, &placement, &ui);
        unsafe { dc.PopAxisAlignedClip() };
        let placement = match measured?.fitted {
            Some(fitted) => place(f64::from(fitted).ceil().min(presentation.card_height)),
            None => placement,
        };
        let mut base = windows_numerics::Matrix3x2::identity();
        unsafe { dc.GetTransform(&mut base) };
        let at = windows_numerics::Matrix3x2::translation(placement.x as f32, placement.y as f32);
        unsafe { dc.SetTransform(&(at * base)) };
        let result = paint(dc, &g, &t, &presentation, &placement, &ui).map(|_| ());
        unsafe { dc.SetTransform(&base) };
        result
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use captures_app::updater::{Renderer, Target};

    fn release(version: &str) -> ReleaseInfo {
        ReleaseInfo {
            renderer: Renderer::Wgpu,
            target: Target::WindowsX64,
            version: version.into(),
            notes: Some("* Fix the tray caret (#600)".into()),
            size: 104_000_000,
            changelog: Vec::new(),
        }
    }

    fn mapped(check: &CheckStatus) -> UpdateStatus {
        status("2026.11.1201", check, false, true, false)
    }

    #[test]
    fn a_failed_tray_lookup_keeps_the_caret_and_position_across_heights() {
        use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
        let (w, h) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
        // An icon in the taskbar's notification area, bottom right.
        let icon = RECT {
            left: w - 200,
            top: h - 40,
            right: w - 176,
            bottom: h - 4,
        };
        let mut anchor = None;
        let (shown, _) = anchored(Some(icon), &mut anchor, 420.);
        assert_eq!(shown.caret, Caret::Bottom);
        assert_eq!(anchor, Some(icon));
        // Hide collapses the notes; the lookup fails this time.
        let (hidden, _) = anchored(None, &mut anchor, 180.);
        assert_eq!(hidden.caret, Caret::Bottom);
        assert_eq!((hidden.x, hidden.caret_x), (shown.x, shown.caret_x));
        assert_eq!(hidden.y + hidden.height, shown.y + shown.height);
        // Without any usable rect yet, it is the caretless fallback.
        let mut none = None;
        assert_eq!(anchored(None, &mut none, 180.).0.caret, Caret::None);
        assert_eq!(none, None);
    }

    #[test]
    fn an_overflow_flyout_delays_the_show_and_a_fallback_show_re_places_later() {
        let tries = tray_notice::STARTUP_NOTICE_TRAY_RETRY_ATTEMPTS;
        // Before the first show: wait out the open flyout (bounded) ...
        assert_eq!(next_poll(false, 1, 0, true, false), Poll::Wait);
        assert_eq!(
            next_poll(false, MENU_WAIT_POLLS - 1, 0, true, false),
            Poll::Wait
        );
        // ... then the tray rect retries begin afresh once it closes.
        assert_eq!(
            next_poll(false, MENU_WAIT_POLLS - 1, 1, false, false),
            Poll::Wait
        );
        assert_eq!(next_poll(false, 3, 2, false, true), Poll::Show);
        assert_eq!(next_poll(false, 0, tries, false, false), Poll::Show);
        // A flyout still open past the bound: the caretless fallback shows.
        assert_eq!(
            next_poll(false, MENU_WAIT_POLLS, 0, true, false),
            Poll::Show
        );
        // Fallback-placed: keep polling while the flyout stays open, then
        // move to the chevron's rect once it appears.
        assert_eq!(next_poll(true, 30, 0, true, false), Poll::Wait);
        assert_eq!(next_poll(true, 30, 5, false, false), Poll::Wait);
        assert_eq!(next_poll(true, 30, 6, false, true), Poll::Replace);
        assert_eq!(
            next_poll(true, REPLACE_POLLS - 1, 1, false, false),
            Poll::Stop
        );
    }

    #[test]
    fn worker_states_map_to_shipping_statuses() {
        let available = mapped(&CheckStatus::Available {
            release: release("2026.11.1302"),
        });
        let notes = ViewState {
            show_changelog: true,
            ..ViewState::default()
        };
        let p = notice::present(Some(&available), &notes);
        assert_eq!(p.title, "Captures update available");
        assert_eq!(p.description, "Version 2026.11.13.2 · 104 MB");
        let footer = p.footer.unwrap();
        assert_eq!(footer.dismiss.label, "Not now");
        assert_eq!(footer.primary.unwrap().label, "Update");
        assert_eq!(
            p.notes.unwrap().groups[0].items[0]
                .pull_request
                .as_ref()
                .map(|pr| pr.number),
            Some(600)
        );
        // A copy that cannot stage packages offers View release.
        let manual = status(
            "2026.11.1201",
            &CheckStatus::Available {
                release: release("2026.11.1302"),
            },
            false,
            false,
            true,
        );
        let p = notice::present(Some(&manual), &ViewState::default());
        assert_eq!(p.footer.unwrap().primary.unwrap().label, "View release");
        assert_eq!(
            p.footer_note.as_deref(),
            Some("Open captures will close. Unsaved edits are kept as drafts.")
        );
        // Verification reads as a finished download; Update restarts.
        let verifying = mapped(&CheckStatus::Verifying {
            release: release("2026.11.1302"),
        });
        let p = notice::present(Some(&verifying), &ViewState::default());
        assert_eq!(p.download.unwrap().percent, Some(100));
        assert!(p.dismiss_blocked);
        let staged = CheckStatus::Staged {
            release: release("2026.11.1302"),
            sha256: String::new(),
        };
        assert!(matches!(
            status("2026.11.1201", &staged, true, true, false),
            UpdateStatus::Restarting {
                seconds_remaining: 3,
                ..
            }
        ));
        assert!(matches!(mapped(&staged), UpdateStatus::Available { .. }));
        // An install failure retries the install; a check failure checks.
        let failed = mapped(&CheckStatus::DownloadError {
            release: release("2026.11.1302"),
            message: "signature verification failed".into(),
        });
        let p = notice::present(Some(&failed), &ViewState::default());
        assert_eq!(p.title, "Update failed");
        assert_eq!(
            p.error.unwrap().message,
            "Could not install the update: signature verification failed"
        );
        assert_eq!(p.footer.unwrap().primary.unwrap().action, Action::Install);
        let check = mapped(&CheckStatus::Error {
            message: "offline".into(),
        });
        let p = notice::present(Some(&check), &ViewState::default());
        assert_eq!(p.footer.unwrap().primary.unwrap().action, Action::Check);
    }

    #[test]
    fn notice_disposition_matches_shipping() {
        let available = mapped(&CheckStatus::Available {
            release: release("2026.11.1302"),
        });
        let error = mapped(&CheckStatus::Error {
            message: "offline".into(),
        });
        assert_eq!(disposition(&available, false), Disposition::Show);
        assert_eq!(disposition(&available, true), Disposition::Defer);
        assert_eq!(disposition(&error, true), Disposition::Show);
        assert_eq!(
            disposition(&mapped(&CheckStatus::UpToDate), false),
            Disposition::Ignore
        );
        assert_eq!(
            disposition(&mapped(&CheckStatus::Checking), false),
            Disposition::Ignore
        );
    }

    #[test]
    fn each_version_opens_once_per_session_after_the_capture() {
        let mut schedule = Schedule::default();
        let v2 = mapped(&CheckStatus::Available {
            release: release("2026.11.1302"),
        });
        // Found during a recording: deferred, then shown when it ends.
        assert!(!schedule.observe(&v2, true, false));
        assert!(!schedule.observe(&v2, true, false));
        assert!(schedule.observe(&v2, false, false));
        // Later hid it: the same version never reopens it.
        assert!(!schedule.observe(&v2, false, false));
        assert!(!schedule.observe(&mapped(&CheckStatus::Checking), false, false));
        assert!(!schedule.observe(&v2, false, false));
        // A newer version does.
        let v3 = mapped(&CheckStatus::Available {
            release: release("2026.11.1303"),
        });
        assert!(schedule.observe(&v3, false, false));
        // Up to date forgets it, so it can show again.
        assert!(!schedule.observe(&mapped(&CheckStatus::UpToDate), false, false));
        assert!(schedule.observe(&v3, false, false));
        // Already on screen: nothing to open.
        let v4 = mapped(&CheckStatus::Available {
            release: release("2026.11.1304"),
        });
        assert!(!schedule.observe(&v4, false, true));
        assert!(!schedule.observe(&v4, false, false));
        // A version gone before the capture ended is not shown.
        let v5 = mapped(&CheckStatus::Available {
            release: release("2026.11.1305"),
        });
        assert!(!schedule.observe(&v5, true, false));
        assert!(!schedule.observe(&mapped(&CheckStatus::UpToDate), false, false));
    }

    #[test]
    fn only_a_failed_install_from_the_notice_reopens_it_even_mid_capture() {
        let downloading = mapped(&CheckStatus::Downloading {
            release: release("2026.11.1302"),
            downloaded: 1,
            total: 2,
        });
        let failed = mapped(&CheckStatus::DownloadError {
            release: release("2026.11.1302"),
            message: "403".into(),
        });
        // From Preferences: the card reports it, the notice stays closed.
        let mut schedule = Schedule::default();
        assert!(!schedule.observe(&downloading, false, false));
        assert!(!schedule.observe(&failed, false, false));
        // From the notice: the failure shows, during a capture too.
        let mut schedule = Schedule::default();
        schedule.install_requested();
        assert!(!schedule.observe(&downloading, false, true));
        assert!(schedule.observe(&failed, true, false));
        assert!(!schedule.observe(&failed, true, false));
    }
}
