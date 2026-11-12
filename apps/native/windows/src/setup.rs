//! Shipping's first-run setup (`Onboarding.tsx`, the `.onboarding*` rules)
//! as one Direct2D surface: the mark, copy, the screen-capture row and the
//! Start capturing button. Copy and per-state presentation come from
//! `captures_app::onboarding`, and permission checks run on a worker like
//! shipping's async commands. Windows has no permission prompt and no
//! microphone row, so the row only ever shows a status, never an action.
use std::cell::RefCell;
use std::sync::mpsc;

use captures_app::app_windows;
use captures_app::onboarding::{self, Action, Presentation, Session, State};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectComposition::*;
use windows::Win32::Graphics::Gdi::ValidateRect;
use windows::Win32::UI::Accessibility::{UiaReturnRawElementProvider, UiaRootObjectId};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

use crate::gfx::{self, Gfx, Style, rect};
use crate::panel_uia::{self, Act, Item, Role};
use crate::tokens::Tokens;
use crate::{Options, appearance};

const CLASS: PCWSTR = w!("CapturesSetup");
const WM_DONE: u32 = WM_APP + 51;
const WM_MOUSELEAVE: u32 = 0x02A3;

type Done = (Action, Session, std::result::Result<State, String>);

thread_local! {
    static SETUP: RefCell<Option<Setup>> = const { RefCell::new(None) };
}

pub fn exists() -> bool {
    SETUP.with(|s| s.try_borrow().map_or(true, |s| s.is_some()))
}

fn with_setup<R>(f: impl FnOnce(&mut Setup) -> R) -> Option<R> {
    SETUP.with(|s| s.try_borrow_mut().ok()?.as_mut().map(f))
}

pub struct Setup {
    hwnd: HWND,
    g: Gfx,
    t: Tokens,
    options: Options,
    scale: f32,
    size: (f32, f32),
    composition: Option<(
        IDCompositionTarget,
        IDCompositionVisual2,
        IDCompositionSurface,
    )>,
    /// Taken while a worker runs an action.
    session: Option<Session>,
    done: Option<mpsc::Receiver<Done>>,
    /// `None` until the first check returns ("Checking…").
    presentation: Option<Presentation>,
    error: Option<String>,
    finishing: bool,
    hover: bool,
    pressed: bool,
    focused: bool,
    tracking: bool,
    /// Part rects from the last paint, for hit tests and UI Automation.
    rects: Rects,
}

/// Where the last paint put each part, in client DIPs.
#[derive(Clone, Copy)]
pub struct Rects {
    title: D2D_RECT_F,
    lede: Option<D2D_RECT_F>,
    row: D2D_RECT_F,
    error: Option<D2D_RECT_F>,
    button: D2D_RECT_F,
}

const EMPTY: D2D_RECT_F = D2D_RECT_F {
    left: 0.,
    top: 0.,
    right: 0.,
    bottom: 0.,
};

impl Setup {
    fn new(hwnd: HWND, options: &Options, g: Gfx, t: Tokens) -> Self {
        Self {
            hwnd,
            g,
            t,
            options: options.clone(),
            scale: 1.,
            size: (app_windows::SETUP.width, app_windows::SETUP.height),
            composition: None,
            session: Some(Session::new()),
            done: None,
            presentation: None,
            error: None,
            finishing: false,
            hover: false,
            pressed: false,
            focused: false,
            tracking: false,
            rects: Rects {
                title: EMPTY,
                lede: None,
                row: EMPTY,
                error: None,
                button: EMPTY,
            },
        }
    }

    fn busy(&self) -> bool {
        self.session.is_none()
    }

    fn enabled(&self) -> bool {
        !self.busy() && self.presentation.as_ref().is_some_and(|p| p.screen_ready)
    }

    /// Run `action` on a worker; the result arrives as `WM_DONE`.
    fn run(&mut self, action: Action) {
        let Some(mut session) = self.session.take() else {
            return;
        };
        self.finishing = action == Action::Complete;
        let (send, receive) = mpsc::channel();
        self.done = Some(receive);
        let path = self.options.settings_file.clone();
        let hwnd = self.hwnd.0 as isize;
        std::thread::spawn(move || {
            let result = session.execute(&path, action);
            let _ = send.send((action, session, result));
            unsafe {
                let _ = PostMessageW(Some(HWND(hwnd as _)), WM_DONE, WPARAM(0), LPARAM(0));
            }
        });
    }

    /// Apply a worker's result. True when setup finished and the window
    /// should close.
    fn finish(&mut self, (action, session, result): Done) -> bool {
        self.session = Some(session);
        self.finishing = false;
        match (action, result) {
            (Action::Complete, Ok(_)) => return true,
            (Action::Complete, Err(error)) => {
                self.error = Some(format!("Couldn’t finish setup: {error}"));
            }
            (_, Err(error)) => {
                self.error = Some(format!("Couldn’t check setup status: {error}"));
            }
            (_, Ok(state)) => {
                self.error = None;
                self.presentation = Some(state.presentation());
            }
        }
        false
    }

    fn activate(&mut self) {
        if self.enabled() {
            self.run(Action::Complete);
        }
    }

    fn resize(&mut self) -> Result<()> {
        let Some(dcomp) = self.g.dcomp.clone() else {
            return Ok(());
        };
        let px = self.pixels();
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
                *current = surface;
            }
        }
        self.render()
    }

    fn pixels(&self) -> (u32, u32) {
        (
            (self.size.0 * self.scale).round().max(1.) as u32,
            (self.size.1 * self.scale).round().max(1.) as u32,
        )
    }

    fn render(&mut self) -> Result<()> {
        let Some((_, _, surface)) = self.composition.clone() else {
            return Ok(());
        };
        let px = self.pixels();
        let scale = (
            px.0 as f32 / self.size.0.max(1.),
            px.1 as f32 / self.size.1.max(1.),
        );
        let mut rects = self.rects;
        gfx::draw_surface(&surface, px, scale, 0., |dc| {
            rects = self.paint(dc)?;
            Ok(())
        })?;
        self.rects = rects;
        unsafe { self.g.dcomp.as_ref().ok_or_else(Error::empty)?.Commit() }
    }

    /// Paint the whole window at the current size and return where each
    /// part went.
    pub fn paint(&self, dc: &ID2D1DeviceContext) -> Result<Rects> {
        let (g, t) = (&self.g, &self.t);
        let c = |name: &str| t.color(name);
        let n = |name: &str| t.n(name);
        let (w, h) = self.size;
        let copy = onboarding::copy();
        let short = app_windows::short(h);
        gfx::fill(dc, rect(0., 0., w, h), c("surface-canvas"))?;

        // `.onboarding-stage`: at most 620 wide, centred both ways.
        let (pad_y, pad_x, gap) = if short {
            (n("s-6"), n("s-7"), n("s-5"))
        } else {
            (n("s-9"), n("s-8"), n("s-7"))
        };
        let stage = w.min(620.);
        let left = (w - stage) / 2. + pad_x;
        let inner = (stage - 2. * pad_x).max(1.);

        // `.onboarding-copy`
        let mark = 40.;
        let eyebrow = g.text_sized(
            t,
            &copy.eyebrow.to_uppercase(),
            n("text-2xs"),
            "weight-semibold",
            Some("tracking-wide"),
        )?;
        let presentation = self.presentation.as_ref();
        let title_text = presentation.map_or(copy.title, |p| p.title);
        let title = g.text(t, Style::RecordingTitle, title_text, inner)?;
        let lede = if short {
            None
        } else {
            let max = (54. * g.ch(t, Style::SetupLede)?).min(inner);
            Some(g.text(t, Style::SetupLede, copy.lede, max)?)
        };
        let copy_gap = n("s-3");
        let copy_height = mark
            + n("s-2")
            + copy_gap
            + eyebrow.height
            + copy_gap
            + title.height
            + lede.as_ref().map_or(0., |l| copy_gap + l.height);

        // `.onboarding-permission`: icon, copy and status columns.
        let row_pad = n("s-6");
        let col_gap = n("s-5");
        let status = presentation.and_then(|p| p.screen_status);
        let status_text = status
            .map(|s| g.text(t, Style::Button, s.label, 10_000.))
            .transpose()?;
        let check = 14.;
        let status_width = match (&status_text, status) {
            (Some(text), Some(s)) if s.ready => check + n("s-2") + text.width,
            (Some(text), _) => text.width,
            _ => 0.,
        };
        let card_inner = inner - 2. - 2. * row_pad;
        let text_width = (card_inner
            - 22.
            - col_gap
            - if status_text.is_some() {
                col_gap + status_width
            } else {
                0.
            })
        .max(1.);
        let heading = g.text(t, Style::RowTitle, copy.screen_title, text_width)?;
        let description_text = presentation.map_or(copy.checking, |p| p.screen_description);
        let description_max = (44. * g.ch(t, Style::CardDescription)?).min(text_width);
        let description = g.text(t, Style::CardDescription, description_text, description_max)?;
        let row_height = (heading.height + n("s-2") + description.height)
            .max(22.)
            .max(status_text.as_ref().map_or(0., |s| s.height.max(check)));
        let card_height = 2. + 2. * row_pad + row_height;

        let error = self
            .error
            .as_deref()
            .map(|e| g.text(t, Style::CardDescription, e, inner - 2. - 2. * n("s-5")))
            .transpose()?;
        let error_height = error.as_ref().map_or(0., |e| 2. + 2. * n("s-4") + e.height);

        // `.onboarding-primary-button`
        let label_text = if self.finishing {
            copy.finishing
        } else {
            presentation.map_or(copy.start, |p| p.primary_label)
        };
        let label = g.text(t, Style::Action, label_text, 10_000.)?;
        let button_width = (label.width + 2. * n("s-6") + 2.).max(148.);
        let button_height = n("h-xl");

        let panel_gap = n("s-5");
        let panel_height = card_height
            + error.as_ref().map_or(0., |_| panel_gap + error_height)
            + panel_gap
            + button_height;
        let total = copy_height + gap + panel_height;
        // `justify-content: center` overflows both ways, like the CSS.
        let mut y = pad_y + (h - 2. * pad_y - total) / 2.;

        // The mark: accent tile with the capture glyph.
        let tile = rect(left, y, left + mark, y + mark);
        gfx::shadow(dc, tile, n("r-lg"), t.shadow("shadow-sm"))?;
        gfx::fill_round(dc, tile, n("r-lg"), c("theme-accent"))?;
        gfx::icon(
            dc,
            &g.factory,
            "capture",
            &[1],
            (left + 8., y + 8., 24.),
            1.7,
            c("theme-accent-ink"),
        )?;
        y += mark + n("s-2") + copy_gap;
        gfx::text(dc, &eyebrow, left, y, c("text-subtle"))?;
        y += eyebrow.height + copy_gap;
        gfx::text(dc, &title, left, y, c("text"))?;
        let title_rect = rect(left, y, left + title.width, y + title.height);
        y += title.height;
        let mut lede_rect = None;
        if let Some(lede) = &lede {
            y += copy_gap;
            gfx::text(dc, lede, left, y, c("text-subtle"))?;
            lede_rect = Some(rect(left, y, left + lede.width, y + lede.height));
            y += lede.height;
        }
        y += gap;

        // The permissions card.
        let card = rect(left, y, left + inner, y + card_height);
        gfx::shadow(dc, card, n("r-xl"), t.shadow("shadow-sm"))?;
        gfx::fill_round(dc, card, n("r-xl"), c("surface-raised"))?;
        gfx::border(dc, card, n("r-xl"), 1., c("border-subtle"))?;
        let (rx, ry) = (left + 1. + row_pad, y + 1. + row_pad);
        gfx::icon(
            dc,
            &g.factory,
            "screen-access",
            &[],
            (rx + 1., ry + 2., 20.),
            1.7,
            c("text-subtle"),
        )?;
        let tx = rx + 22. + col_gap;
        gfx::text(dc, &heading, tx, ry, c("text"))?;
        gfx::text(
            dc,
            &description,
            tx,
            ry + heading.height + n("s-2"),
            c("text-subtle"),
        )?;
        if let (Some(text), Some(s)) = (&status_text, status) {
            let right = left + inner - 1. - row_pad;
            let color = c(if s.ready {
                "positive-text"
            } else {
                "text-subtle"
            });
            let line = text.height.max(check);
            if s.ready {
                // The 16-unit check at 14 px.
                gfx::icon(
                    dc,
                    &g.factory,
                    "setup-check",
                    &[],
                    (right - status_width, ry + (line - check) / 2., check * 1.5),
                    2.,
                    color,
                )?;
            }
            gfx::text(
                dc,
                text,
                right - text.width,
                ry + (line - text.height) / 2.,
                color,
            )?;
        }
        y += card_height + panel_gap;

        let mut error_rect = None;
        if let Some(error) = &error {
            let r = rect(left, y, left + inner, y + error_height);
            error_rect = Some(r);
            gfx::fill_round(dc, r, n("r-md"), c("danger-surface"))?;
            gfx::border(dc, r, n("r-md"), 1., c("danger-border"))?;
            gfx::text(
                dc,
                error,
                left + 1. + n("s-5"),
                y + 1. + n("s-4"),
                c("danger-text"),
            )?;
            y += error_height + panel_gap;
        }

        let right = left + inner;
        let button = rect(right - button_width, y, right, y + button_height);
        let alpha = if self.enabled() { 1. } else { 0.4 };
        let fill = if self.hover && self.enabled() {
            "solid-hover"
        } else {
            "solid"
        };
        if self.focused {
            gfx::ring(dc, button, n("r-md"), 0., 2., c("surface-canvas"))?;
            gfx::ring(
                dc,
                button,
                n("r-md"),
                2.,
                4.,
                t.color_alpha("theme-accent", 0.55),
            )?;
        }
        gfx::fill_round(dc, button, n("r-md"), t.color_alpha(fill, alpha))?;
        gfx::text(
            dc,
            &label,
            button.left + (button_width - label.width) / 2.,
            y + (button_height - label.height) / 2.,
            t.color_alpha("solid-ink", alpha),
        )?;
        Ok(Rects {
            title: title_rect,
            lede: lede_rect,
            row: card,
            error: error_rect,
            button,
        })
    }

    /// The parts in shipping's DOM order: `<h1>`, the lede, the
    /// `aria-live="polite"` permission row, the `role="alert"` error and the
    /// primary button.
    fn uia_items(&self) -> Vec<Item> {
        let copy = onboarding::copy();
        let presentation = self.presentation.as_ref();
        let r = self.rects;
        let mut items = vec![Item::new(
            0,
            Role::Heading(1),
            presentation.map_or(copy.title, |p| p.title),
            r.title,
        )];
        if let Some(lede) = r.lede {
            items.push(Item::new(1, Role::Text, copy.lede, lede));
        }
        let description = presentation.map_or(copy.checking, |p| p.screen_description);
        let row = match presentation.and_then(|p| p.screen_status) {
            Some(status) => format!("{}: {}. {description}", copy.screen_title, status.label),
            None => format!("{}. {description}", copy.screen_title),
        };
        items.push(Item::new(2, Role::Status, row, r.row));
        if let (Some(error), Some(rect)) = (&self.error, r.error) {
            items.push(Item::new(3, Role::Alert, error.as_str(), rect));
        }
        let mut button = Item::new(
            4,
            Role::Button,
            if self.finishing {
                copy.finishing
            } else {
                presentation.map_or(copy.start, |p| p.primary_label)
            },
            r.button,
        );
        button.enabled = self.enabled();
        button.focused = self.focused;
        items.push(button);
        items
    }

    fn hit(&self, x: f32, y: f32) -> bool {
        let b = self.rects.button;
        x >= b.left && x < b.right && y >= b.top && y < b.bottom
    }

    /// Handle a message; `Some(true)` asks the caller to close the window.
    fn handle(&mut self, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<bool> {
        match msg {
            WM_DONE => {
                let done = self.done.as_ref()?.try_recv().ok()?;
                self.done = None;
                if self.finish(done) {
                    return Some(true);
                }
            }
            WM_SIZE if wp.0 != SIZE_MINIMIZED as usize => {
                self.size = (
                    (lp.0 as u32 & 0xffff) as f32 / self.scale,
                    ((lp.0 as u32 >> 16) & 0xffff) as f32 / self.scale,
                );
                let _ = self.resize();
                return Some(false);
            }
            WM_SETTINGCHANGE | WM_THEMECHANGED => {
                let settings =
                    captures_settings::load(&self.options.settings_file).unwrap_or_default();
                self.t = appearance::tokens(self.options.appearance.as_deref(), &settings);
                appearance::title_bar(self.hwnd, &self.t);
            }
            WM_GETMINMAXINFO => unsafe {
                let spec = app_windows::SETUP;
                let (x, y) = crate::host::window_size(self.hwnd, spec.min_width, spec.min_height);
                (*(lp.0 as *mut MINMAXINFO)).ptMinTrackSize = POINT { x, y };
                return None;
            },
            WM_MOUSEMOVE | WM_LBUTTONDOWN | WM_LBUTTONDBLCLK | WM_LBUTTONUP => {
                let (x, y) = (
                    (lp.0 as u16 as i16) as f32 / self.scale,
                    ((lp.0 >> 16) as u16 as i16) as f32 / self.scale,
                );
                let over = self.hit(x, y);
                self.hover = over;
                match msg {
                    WM_MOUSEMOVE if !self.tracking => unsafe {
                        let _ = TrackMouseEvent(&mut TRACKMOUSEEVENT {
                            cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                            dwFlags: TME_LEAVE,
                            hwndTrack: self.hwnd,
                            ..Default::default()
                        });
                        self.tracking = true;
                    },
                    WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => unsafe {
                        // A click is not keyboard focus: no ring, like
                        // `:focus-visible`.
                        self.focused = false;
                        self.pressed = over && self.enabled();
                        if self.pressed {
                            SetCapture(self.hwnd);
                        }
                    },
                    WM_LBUTTONUP => {
                        let pressed = std::mem::take(&mut self.pressed);
                        unsafe {
                            let _ = ReleaseCapture();
                        }
                        if pressed && over {
                            self.activate();
                        }
                    }
                    _ => {}
                }
            }
            WM_SETCURSOR if (lp.0 & 0xffff) as u32 == HTCLIENT => unsafe {
                let cursor = if self.hover && !self.enabled() && self.presentation.is_some() {
                    IDC_NO
                } else if self.hover && self.enabled() {
                    IDC_HAND
                } else {
                    IDC_ARROW
                };
                SetCursor(LoadCursorW(None, cursor).ok());
                return Some(false);
            },
            WM_MOUSELEAVE => {
                self.tracking = false;
                self.hover = false;
            }
            WM_CAPTURECHANGED => self.pressed = false,
            WM_KEYDOWN => match VIRTUAL_KEY(wp.0 as u16) {
                // The button is the only control: Tab either way focuses it.
                VK_TAB => self.focused = true,
                VK_RETURN | VK_SPACE if self.focused => self.activate(),
                _ => return None,
            },
            WM_KILLFOCUS => self.focused = false,
            _ => return None,
        }
        let _ = self.render();
        Some(false)
    }
}

/// Setup's parts for UI Automation, while the window is open.
pub fn uia_snapshot() -> Option<panel_uia::Snapshot> {
    with_setup(|s| panel_uia::Snapshot {
        hwnd: s.hwnd,
        scale: s.scale,
        size: s.size,
        name: app_windows::SETUP.title,
        focusable: true,
        items: s.uia_items(),
    })
}

/// The button is setup's only control: Invoke presses it, Focus focuses it.
pub fn uia_act(key: u32, act: Act) -> Option<()> {
    let hwnd = with_setup(|s| {
        if key == 4 {
            match act {
                Act::Invoke => s.activate(),
                Act::Focus => s.focused = true,
                _ => {}
            }
        }
        let _ = s.render();
        s.hwnd
    })?;
    if matches!(act, Act::Focus) {
        unsafe {
            let _ = SetFocus(Some(hwnd));
        }
    }
    Some(())
}

pub fn register(instance: HINSTANCE) -> Result<()> {
    unsafe {
        RegisterClassW(&WNDCLASSW {
            style: CS_DBLCLKS,
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: CLASS,
            ..Default::default()
        });
    }
    Ok(())
}

/// Open first-run setup, or bring it forward when it is already open.
pub fn open(options: &Options) -> Result<()> {
    if let Some(hwnd) = with_setup(|s| s.hwnd) {
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
            let _ = SetForegroundWindow(hwnd);
        }
        return Ok(());
    }
    let settings = captures_settings::load(&options.settings_file).unwrap_or_default();
    let tokens = appearance::tokens(options.appearance.as_deref(), &settings);
    let g = Gfx::new(&tokens, true)?;
    let spec = app_windows::SETUP;
    unsafe {
        let instance = windows::Win32::System::LibraryLoader::GetModuleHandleW(None)?;
        let hwnd = CreateWindowExW(
            WS_EX_NOREDIRECTIONBITMAP,
            CLASS,
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
        let mut setup = Setup::new(hwnd, options, g, tokens);
        setup.scale = GetDpiForWindow(hwnd).max(96) as f32 / 96.;
        appearance::title_bar(hwnd, &setup.t);
        setup.run(Action::Check);
        SETUP.with(|s| *s.borrow_mut() = Some(setup));
        let (w, h) = options
            .size
            .filter(|_| options.scene == crate::Scene::Setup)
            .map_or((spec.width, spec.height), |(w, h)| (w as f32, h as f32));
        let (ww, wh) = crate::host::window_size(hwnd, w, h);
        // Sizing sends WM_SIZE, which creates the surface and paints.
        let _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            ww,
            wh,
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
    }
    Ok(())
}

/// `--scene setup --screenshot`: the checked state of `options`' settings,
/// `--select checking` before the first check, `--select error` with a
/// failed finish and `--select focus` with the button focused.
pub fn render_offscreen(options: &Options, g: Gfx, t: Tokens, size: (f32, f32)) -> Result<Setup> {
    let mut setup = Setup::new(HWND::default(), options, g, t);
    setup.size = size;
    let select = options.select.as_deref();
    if select != Some("checking") {
        // A failed check renders like the window shows it.
        let checked = Session::new().execute(&options.settings_file, Action::Check);
        setup.finish((Action::Check, Session::new(), checked));
    }
    if select == Some("error") {
        setup.error = Some("Couldn’t finish setup: settings file is read-only".into());
    }
    setup.focused = select == Some("focus");
    setup.hover = select == Some("hover");
    Ok(setup)
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_GETOBJECT if lp.0 as i32 == UiaRootObjectId => {
                return UiaReturnRawElementProvider(
                    hwnd,
                    wp,
                    lp,
                    &panel_uia::node(panel_uia::Window::Setup, None),
                );
            }
            WM_DESTROY => {
                let _ = UiaReturnRawElementProvider(hwnd, WPARAM(0), LPARAM(0), None);
                SETUP.with(|s| {
                    if let Ok(mut s) = s.try_borrow_mut() {
                        s.take();
                    }
                });
                if !crate::host::exists()
                    && !crate::history::exists()
                    && !crate::editor::exists()
                    && !crate::recording::exists()
                    && !crate::feedback::exists()
                    && !crate::live::resident()
                {
                    PostQuitMessage(0);
                }
                return LRESULT(0);
            }
            WM_ERASEBKGND => return LRESULT(1),
            WM_PAINT => {
                let _ = ValidateRect(Some(hwnd), None);
                return LRESULT(0);
            }
            WM_DPICHANGED => {
                with_setup(|s| s.scale = (wp.0 & 0xffff) as f32 / 96.);
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
        let before = panel_uia::watch(panel_uia::Window::Setup);
        let handled = with_setup(|s| s.handle(msg, wp, lp)).flatten();
        if handled == Some(false) {
            panel_uia::raise(panel_uia::Window::Setup, before);
        }
        match handled {
            Some(true) => {
                let _ = DestroyWindow(hwnd);
                crate::live::setup_finished();
                LRESULT(0)
            }
            Some(false) if msg == WM_SETCURSOR => LRESULT(1),
            Some(false) => LRESULT(0),
            None => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}
