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

use captures_app::tray_notice::{self, Caret, LogicalRect, Placement};
use captures_app::update_notice::{
    self as notice, Action, Icon, IconTone, Presentation, UpdateStatus, ViewState,
};
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
/// Polls the tray icon rect before the first show, like the launch notice.
const RETRY_TIMER: usize = 2;
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
    let available = |version: &str, notes: Option<String>, size: u64| UpdateStatus::Available {
        current_version: current_version.clone(),
        current_display_version: current_display_version.clone(),
        version: version.to_owned(),
        display_version: display_version(version),
        notes,
        changelog: Vec::new(),
        installable,
        manual_download_url: (!installable).then(|| RELEASES_URL.to_owned()),
        download_size: (size > 0).then_some(size),
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
        CheckStatus::Available { release } => {
            available(&release.version, release.notes.clone(), release.size)
        }
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
        CheckStatus::Staged { release, .. } => {
            available(&release.version, release.notes.clone(), release.size)
        }
        CheckStatus::Error { message } => UpdateStatus::Error {
            current_version,
            current_display_version,
            message: message.clone(),
            retry_install: false,
        },
        CheckStatus::DownloadError { message, .. } => UpdateStatus::Error {
            current_version,
            current_display_version,
            message: format!("Could not install the update: {message}"),
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
            }
            if blocked.is_some() {
                n.view.action_error = blocked;
            }
            n.status = live.status;
            n.step = live.step;
            n.refresh(changed);
        });
        panel_uia::raise(panel_uia::Window::Update, before);
    } else if show && let Err(error) = open(live) {
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

/// Hide the notice (Later/Close). True when one was up.
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
    settings_file: PathBuf,
    tray: TrayRect,
    /// The last usable tray icon rect. A re-placement keeps it when a later
    /// lookup fails, so the caret and position stay put.
    anchor: Option<RECT>,
    /// Tray rect polls before the first show.
    attempts: u32,
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
    started: Instant,
    restart_at: Option<Instant>,
    animating: bool,
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
    let g = Gfx::new(&t, true)?;
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
        let (placement, scale) = anchored((live.tray)(), &mut anchor, presentation.card_height);
        NOTICE.with(|n| {
            *n.borrow_mut() = Some(Notice {
                hwnd,
                g,
                t,
                appearance: live.appearance,
                settings_file: live.settings_file,
                tray: live.tray,
                anchor,
                attempts: 0,
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
                started: Instant::now(),
                restart_at: None,
                animating: false,
            })
        });
        if placement.caret == Caret::None && tray_notice::should_retry_tray_rect() {
            // As the launch notice: the icon may have no rect yet (a quiet
            // launch) or while the tray menu that chose Check for Updates
            // closes; poll before falling back above the taskbar.
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
    /// physical recheck lost the caret and moved the card 8 pt left after
    /// Hide); the icon has not moved, so neither does the notice.
    fn replace(&mut self) {
        let height = self.presentation.card_height;
        (self.placement, self.scale) = anchored((self.tray)(), &mut self.anchor, height);
        self.place();
    }

    /// Place and show the notice without activating it, then announce it.
    fn present(&mut self) {
        self.place();
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
            // After the tray menu (or another window) has handed focus back,
            // so the focus change does not talk over the announcement.
            SetTimer(Some(self.hwnd), ANNOUNCE_TIMER, ANNOUNCE_DELAY_MS, None);
        }
    }

    /// One tray rect poll before the first show, as the launch notice.
    fn retry(&mut self) {
        self.attempts += 1;
        let height = self.presentation.card_height;
        (self.placement, self.scale) = anchored((self.tray)(), &mut self.anchor, height);
        if self.placement.caret != Caret::None
            || self.attempts >= tray_notice::STARTUP_NOTICE_TRAY_RETRY_ATTEMPTS
        {
            unsafe {
                let _ = KillTimer(Some(self.hwnd), RETRY_TIMER);
            }
            self.present();
        }
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
        if let Err(error) = self.render() {
            eprintln!("failed to draw the Captures update notice: {error}");
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
        let ui = Ui {
            hover: self.hover,
            scroll: self.scroll,
            time: elapsed,
            restart: restart.clamp(0., 1.),
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
            self.scroll = self.scroll.min(self.scroll_max);
        }
        // The spinner, an indeterminate bar and the restart bar move.
        let animating = !reduced
            && (self.presentation.icon == Icon::Spinner
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
        Action::ShowNotes | Action::HideNotes => {
            let show = action == Action::ShowNotes;
            let file = with_notice(|n| {
                n.view.show_changelog = show;
                n.refresh(false);
                n.settings_file.clone()
            });
            // Shipping persists the choice. Preferences, when open, saves it
            // and moves its own What's new switch.
            let saved = crate::host::with_host(|h| {
                h.scene.set_show_update_changelog(show);
                h.need = true;
            });
            // The fixture never writes settings.
            if saved.is_none()
                && !FIXTURE.get()
                && let Some(file) = file
                && let Ok(mut settings) = captures_settings::load(&file)
                && settings.show_update_changelog != show
            {
                settings.show_update_changelog = show;
                if let Err(error) = captures_settings::save(&file, &settings) {
                    eprintln!("Could not save the What's new choice: {error}");
                }
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
}

pub struct Painted {
    pub parts: Vec<Part>,
    pub scroll_max: f32,
    /// The notes scroller, for the wheel.
    pub notes: Option<D2D_RECT_F>,
}

/// Wrapped text at `size`/`weight`, with a CSS `line-height` multiplier.
fn para(
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

/// A laid-out release note: its text (with the pull request appended) and
/// the link's range.
struct NoteLine {
    text: Text,
    link: Option<(DWRITE_TEXT_RANGE, u64, String)>,
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
    let (sm, xs, xxs) = (n("text-sm"), n("text-xs"), n("text-2xs"));
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
            c("surface-raised"),
        )?;
    }
    // The grid inside the border.
    let (left, right) = (card.left + 1., card.right - 1.);
    let (top, bottom) = (card.top + 1., card.bottom - 1.);
    let width = right - left;
    let inner = width - 2. * s6;

    // ---- measure ----
    // `.update-notice-header`: 36 px icon, copy, optional What's new.
    let reveal = match &p.reveal_notes {
        Some(button) => Some((
            button,
            para(
                g,
                t,
                &button.label.to_uppercase(),
                (xs, "weight-semibold"),
                Some("tracking-wide"),
                None,
                10_000.,
            )?,
        )),
        None => None,
    };
    let reveal_size = reveal
        .as_ref()
        .map(|(_, text)| (text.width + 2. * s3, text.height + 2. * n("s-2")));
    let copy_width = inner - 36. - s4 - reveal_size.map_or(0., |(w, _)| w + s4);
    let title = para(
        g,
        t,
        &p.title,
        (n("text-lg"), "weight-semibold"),
        Some("tracking-tight"),
        Some(tight),
        copy_width,
    )?;
    let description = para(
        g,
        t,
        &p.description,
        (sm, "weight-regular"),
        None,
        None,
        copy_width,
    )?;
    let copy_height = title.height + 2. + description.height;
    let header_content = copy_height.max(36.).max(reveal_size.map_or(0., |(_, h)| h));
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
            let hide = para(
                g,
                t,
                &notes.hide.label.to_uppercase(),
                (xs, "weight-semibold"),
                Some("tracking-wide"),
                None,
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
            let empty = match &notes.empty {
                Some(empty) => Some(para(
                    g,
                    t,
                    empty,
                    (sm, "weight-regular"),
                    None,
                    Some(snug),
                    boxed,
                )?),
                None => None,
            };
            // `.update-notes-scroll { padding-right: var(--s-1) }`.
            let list = boxed - if notes.stacked { n("s-1") } else { 0. };
            let mut groups = Vec::new();
            for group in &notes.groups {
                let heading = if notes.stacked {
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
                    let (copy, link) = match &item.pull_request {
                        Some(pull) => {
                            let label = format!("#{}", pull.number);
                            let copy = format!("{} {label}", item.text);
                            let range = utf16_range(&copy, item.text.len() + 1, label.len());
                            (copy, Some((range, pull.number, pull.url.clone())))
                        }
                        None => (item.text.clone(), None),
                    };
                    let text = para(
                        g,
                        t,
                        &copy,
                        (sm, "weight-regular"),
                        None,
                        Some(snug),
                        list - s5,
                    )?;
                    items.push(NoteLine { text, link });
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
            Some((notes, heading, hide, intro, empty, groups))
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
    if let Some((_, label, ..)) = &download {
        body_natural += sunken(label.height + s4 + bar);
    }
    if let Some(message) = &restart {
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

    // `.update-close-warning`: icon 16 beside wrapped copy.
    let warning = match &p.close_warning {
        Some(warning) => Some((
            warning,
            para(
                g,
                t,
                warning,
                (sm, "weight-regular"),
                None,
                Some(snug),
                inner - 2. - 2. * s4 - 16. - s3,
            )?,
        )),
        None => None,
    };
    let warning_natural = warning
        .as_ref()
        .map_or(0., |(_, text)| text.height.max(17.) + 2. * s3 + 2. + s3);

    // `.update-notice-footer`: Later/Close, then the primary.
    let footer_natural = if p.footer.is_some() {
        56f32.max(s4 + n("h-md") + s6)
    } else {
        0.
    };

    // ---- rows ----
    // Shipping's four grid rows: compact cards share the spare height
    // between every track (empty ones too); with notes, the body takes it.
    let content = bottom - top;
    let (header_h, body_h, warning_h, footer_h) = if notes.is_some() {
        let body = (content - header - warning_natural - footer_natural).max(0.);
        (header, body, warning_natural, footer_natural)
    } else {
        let used = header + body_natural + warning_natural + footer_natural;
        let extra = (content - used).max(0.) / 4.;
        let grow = |h: f32, present: bool| if present { h + extra } else { h };
        (
            header + extra,
            grow(body_natural, has_body),
            grow(warning_natural, warning.is_some()),
            grow(footer_natural, p.footer.is_some()),
        )
    };

    // ---- header ----
    let y0 = top;
    let mid = y0 + s6 + (header_h - s6 - s5) / 2.;
    let icon = rect(left + s6, mid - 18., left + s6 + 36., mid + 18.);
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
    let copy_x = icon.right + s4;
    let copy_y = mid - copy_height / 2.;
    gfx::text(dc, &title, copy_x, copy_y, c("text"))?;
    let desc_y = copy_y + title.height + 2.;
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
    if let (Some((button, text)), Some((w, h))) = (&reveal, reveal_size) {
        let r = rect(right - s6 - w, mid - h / 2., right - s6, mid + h / 2.);
        let key = part(
            Role::Button,
            &button.label,
            r,
            Some(button.action.clone()),
            button.enabled,
        );
        let color = if hovered(key) { "text" } else { "text-subtle" };
        gfx::text(dc, text, r.left + s3, r.top + n("s-2"), c(color))?;
    }

    // ---- body ----
    let body_top = y0 + header_h;
    let mut y = body_top;
    let (bx0, bx1) = (left + s6, right - s6);
    let mut scroll_max = 0.;
    let mut notes_view = None;
    if let Some((notes, heading, hide, intro, empty, groups)) = &notes {
        let section = rect(bx0, y, bx1, body_top + body_h - s4);
        gfx::fill_round(dc, section, n("r-lg"), c("surface-sunken"))?;
        gfx::border(dc, section, n("r-lg"), 1., c("border-subtle"))?;
        let ix = bx0 + 1. + s5;
        let mut iy = y + 1. + s5;
        let row = heading.height.max(hide.height);
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
        let hide_r = rect(
            bx1 - 1. - s5 - hide.width,
            iy + (row - hide.height) / 2.,
            bx1 - 1. - s5,
            iy + (row + hide.height) / 2.,
        );
        let key = part(
            Role::Button,
            &notes.hide.label,
            hide_r,
            Some(notes.hide.action.clone()),
            notes.hide.enabled,
        );
        let color = if hovered(key) { "text" } else { "text-subtle" };
        gfx::text(dc, hide, hide_r.left, hide_r.top, c(color))?;
        iy += row + s4;
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
        if let Some(empty) = empty {
            gfx::text(dc, empty, ix, iy, c("text-subtle"))?;
            part(
                Role::Text,
                notes.empty.as_deref().unwrap_or_default(),
                rect(ix, iy, ix + empty.width, iy + empty.height),
                None,
                true,
            );
        } else {
            // The scroller: the rest of the section, clipped.
            let view = rect(ix, iy, bx1 - 1. - s5, section.bottom - 1. - s5);
            let mut natural = 0.;
            for (index, group) in groups.iter().enumerate() {
                if index > 0 {
                    natural += s5 + 1. + s4;
                }
                if let Some(h3) = &group.heading {
                    natural += h3.height + s3;
                }
                for (i, item) in group.items.iter().enumerate() {
                    natural += item.text.height + if i > 0 { s3 } else { 0. };
                }
                if let Some(empty) = &group.empty {
                    natural += empty.height;
                }
            }
            scroll_max = (natural - (view.bottom - view.top)).max(0.);
            let scroll = ui.scroll.clamp(0., scroll_max);
            notes_view = Some(view);
            unsafe { dc.PushAxisAlignedClip(&view, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE) };
            let mut gy = view.top - scroll;
            let visible = |r: D2D_RECT_F| r.bottom > view.top && r.top < view.bottom;
            let mut result = Ok(());
            for (index, (group, source)) in groups.iter().zip(&notes.groups).enumerate() {
                if index > 0 {
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
                    if i > 0 {
                        gy += s3;
                    }
                    // `li::before`: a 5 px dot at 0.5em.
                    let dot = gy + sm / 2. + 2.5;
                    result = result.and(gfx::fill_round(
                        dc,
                        rect(view.left, dot - 2.5, view.left + 5., dot + 2.5),
                        2.5,
                        c("text-faint"),
                    ));
                    let x = view.left + s5;
                    let r = rect(x, gy, x + item.text.width, gy + item.text.height);
                    if visible(r) {
                        part(Role::Text, &note.text, r, None, true);
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
    let ix = bx0 + 1. + s5;
    let iw = boxed;
    if let Some((download, label, detail, percent)) = &download {
        let r = sunken_box(y, label.height + s4 + bar)?;
        let iy = r.top + 1. + s5;
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
        y = r.bottom;
    }
    if let (Some(message), Some(source)) = (&restart, &p.restart) {
        let r = sunken_box(y, message.height + bar)?;
        let iy = r.top + 1. + s5;
        gfx::text(dc, message, ix, iy, c("text-subtle"))?;
        // `update-restart-progress`: full, emptying over the countdown.
        let track = rect(ix, iy + message.height, ix + iw, iy + message.height + bar);
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
        y = r.bottom;
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

    // ---- close warning ----
    let warning_top = body_top + body_h;
    if let Some((copy, text)) = &warning {
        let r = rect(bx0, warning_top, bx1, warning_top + warning_h - s3);
        gfx::fill_round(dc, r, n("r-md"), c("caution-surface"))?;
        gfx::border(dc, r, n("r-md"), 1., t.color_alpha("caution-text", 0.32))?;
        let ink = c("caution-text");
        let gx = r.left + 1. + s4;
        let gy = r.top + 1. + s3 + 1.;
        gfx::icon(dc, &g.factory, "warning", &[], (gx, gy, 16.), 1.8, ink)?;
        gfx::text(dc, text, gx + 16. + s3, r.top + 1. + s3, ink)?;
        part(Role::Status, copy, r, None, true);
    }

    // ---- footer ----
    if let Some(footer) = &p.footer {
        let fy0 = warning_top + warning_h;
        let mid = fy0 + s4 + (footer_h - s4 - s6) / 2.;
        let h = n("h-md");
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
            let w = (text.width + 2. * s5 + 2.).max(104.);
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
            gfx::text(
                dc,
                &text,
                r.left + (w - text.width) / 2.,
                mid - text.height / 2.,
                fade(c("solid-ink"), primary.enabled),
            )?;
        }
    }
    Ok(Painted {
        parts,
        scroll_max,
        notes: notes_view,
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
    let placement = notice::placement(
        screen,
        work,
        tray,
        false,
        notice::CARD_WIDTH,
        presentation.card_height,
    );
    debug_assert!(tray.is_none() || placement.caret == Caret::Bottom);
    Ok(move |dc: &ID2D1DeviceContext| {
        let mut base = windows_numerics::Matrix3x2::identity();
        unsafe { dc.GetTransform(&mut base) };
        let at = windows_numerics::Matrix3x2::translation(placement.x as f32, placement.y as f32);
        unsafe { dc.SetTransform(&(at * base)) };
        let ui = Ui {
            hover: None,
            scroll: 0.,
            time: 0.,
            restart: 1.,
        };
        let result = paint(dc, &g, &t, &presentation, &placement, &ui).map(|_| ());
        unsafe { dc.SetTransform(&base) };
        result
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use captures_app::updater::{ReleaseInfo, Renderer, Target};

    fn release(version: &str) -> ReleaseInfo {
        ReleaseInfo {
            renderer: Renderer::Wgpu,
            target: Target::WindowsX64,
            version: version.into(),
            notes: Some("* Fix the tray caret (#600)".into()),
            size: 104_000_000,
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
    fn worker_states_map_to_shipping_statuses() {
        let available = mapped(&CheckStatus::Available {
            release: release("2026.11.1302"),
        });
        let notes = ViewState {
            show_changelog: true,
            ..ViewState::default()
        };
        let p = notice::present(Some(&available), &notes);
        assert_eq!(p.title, "Update available");
        assert_eq!(p.description, "Version 2026.11.13.2 · 104 MB");
        let footer = p.footer.unwrap();
        assert_eq!(footer.dismiss.label, "Later");
        assert_eq!(footer.primary.unwrap().label, "Update now");
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
            p.close_warning.as_deref(),
            Some("Open captures will close. Unsaved edits are kept as drafts.")
        );
        // Verification reads as a finished download; Update now restarts.
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
