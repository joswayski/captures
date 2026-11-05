//! The Win32 window: per-monitor-v2 DPI, one DirectComposition visual per
//! scene node and one retained surface per node with content, input, and the
//! login item worker.
//!
//! Damage model: a frame redraws only nodes whose `dirty` flag is set and
//! updates offsets, opacity and visibility only for nodes whose `props` flag
//! is set. Hover and toggle transitions are compositor animations; scrolling
//! moves the card column's visual without redrawing it.
use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};

use captures_app::preferences;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::DirectComposition::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

use crate::gfx::{self, Gfx};
use crate::scene::{Action, Event, Prop, RowKey, Scene, Target};
use crate::{Options, appearance};

const WM_APP_LOGIN: u32 = WM_APP + 1;
const WM_MOUSELEAVE: u32 = 0x02A3;
const TIMER_MOTION: usize = 1;
const SEGMENTS: usize = 16;

thread_local! {
    static HOST: RefCell<Option<Host>> = const { RefCell::new(None) };
}

/// Run `f` on the host unless it is borrowed (re-entrant messages, UIA calls
/// during a frame) or not created yet.
pub fn with_host<R>(f: impl FnOnce(&mut Host) -> R) -> Option<R> {
    HOST.with(|h| h.try_borrow_mut().ok().and_then(|mut h| h.as_mut().map(f)))
}

struct Login {
    root: PathBuf,
    settings_file: PathBuf,
    results: Receiver<(Option<bool>, std::result::Result<bool, String>)>,
    sender: Sender<(Option<bool>, std::result::Result<bool, String>)>,
}

pub struct Host {
    pub hwnd: HWND,
    pub g: Gfx,
    pub scene: Scene,
    pub need: bool,
    _target: IDCompositionTarget,
    visuals: Vec<IDCompositionVisual3>,
    surfaces: Vec<Option<(IDCompositionSurface, (u32, u32))>>,
    /// Static visual offsets in pixels, relative to the parent visual.
    offsets: Vec<(f32, f32)>,
    layout: bool,
    tracking: bool,
    timer: bool,
    pointer: Option<(f32, f32)>,
    login: Option<Login>,
    appearance: Option<String>,
}

impl Host {
    fn new(hwnd: HWND, options: &Options) -> Result<Self> {
        let settings = captures_settings::load(&options.settings_file).unwrap_or_default();
        let tokens = appearance::tokens(options.appearance.as_deref(), &settings);
        let g = Gfx::new(&tokens, true)?;
        let platform = captures_app::shortcuts::ShortcutPlatform::Windows;
        let detail = match &options.history_root {
            Some(_) => preferences::LOGIN_ITEM_CHECKING,
            None => preferences::login_item_unavailable(platform),
        };
        let mut scene = Scene::new(tokens, options.settings_file.clone(), detail.into());
        scene.scale = unsafe { GetDpiForWindow(hwnd) } as f32 / 96.;
        scene.reduced_motion = appearance::reduced_motion();
        let dcomp = g.dcomp.clone().ok_or_else(Error::empty)?;
        unsafe {
            let target = dcomp.CreateTargetForHwnd(hwnd, true)?;
            let mut visuals: Vec<IDCompositionVisual3> = Vec::new();
            for (index, node) in scene.nodes.iter().enumerate() {
                let visual: IDCompositionVisual3 = dcomp.CreateVisual()?.cast()?;
                if node.group_opacity {
                    visual.SetOpacityMode(DCOMPOSITION_OPACITY_MODE_LAYER)?;
                }
                match node.parent {
                    // Siblings stack in node order, each above the one before.
                    Some(parent) => {
                        let siblings = &scene.children[parent];
                        let position = siblings.iter().position(|&c| c == index).expect("child");
                        match position.checked_sub(1).map(|p| &visuals[siblings[p]]) {
                            Some(below) => visuals[parent].AddVisual(&visual, true, below)?,
                            None => visuals[parent].AddVisual(
                                &visual,
                                false,
                                None::<&IDCompositionVisual>,
                            )?,
                        }
                    }
                    None => target.SetRoot(&visual)?,
                }
                visuals.push(visual);
            }
            let count = scene.nodes.len();
            let (sender, results) = channel();
            let login = options.history_root.clone().map(|root| Login {
                root,
                settings_file: options.settings_file.clone(),
                results,
                sender,
            });
            appearance::title_bar(hwnd, &scene.t);
            Ok(Self {
                hwnd,
                g,
                scene,
                need: true,
                _target: target,
                visuals,
                surfaces: (0..count).map(|_| None).collect(),
                offsets: vec![(0., 0.); count],
                layout: true,
                tracking: false,
                timer: false,
                pointer: None,
                login,
                appearance: options.appearance.clone(),
            })
        }
    }

    fn client_dips(&self) -> (f32, f32) {
        let mut rc = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut rc);
        }
        let s = self.scene.scale;
        (rc.right as f32 / s, rc.bottom as f32 / s)
    }

    fn frame(&mut self) -> Result<()> {
        if self.layout {
            self.layout = false;
            let (w, h) = self.client_dips();
            self.scene.layout(&self.g, w, h)?;
        }
        let s = self.scene.scale;
        let count = self.scene.nodes.len();
        // Visual origins in pixels: rounded rect origin minus rounded bleed.
        let mut abs = vec![(0f32, 0f32); count];
        let mut origin = vec![(0f32, 0f32); count];
        for i in 0..count {
            let node = &self.scene.nodes[i];
            let base = node.parent.map_or((0., 0.), |p| abs[p]);
            abs[i] = (base.0 + node.rect.x, base.1 + node.rect.y);
            let bleed = (node.bleed * s).round();
            origin[i] = (
                (abs[i].0 * s).round() - bleed,
                (abs[i].1 * s).round() - bleed,
            );
        }
        let mut draws = Vec::new();
        unsafe {
            for i in 0..count {
                let node = &mut self.scene.nodes[i];
                if node.props {
                    node.props = false;
                    let parent = node.parent.map_or((0., 0.), |p| origin[p]);
                    let offset = (origin[i].0 - parent.0, origin[i].1 - parent.1);
                    self.offsets[i] = offset;
                    let visual = &self.visuals[i];
                    visual.SetOffsetX2(offset.0)?;
                    visual.SetOffsetY2(offset.1)?;
                    visual.SetOpacity2(node.opacity)?;
                    visual.SetVisible(node.visible)?;
                    if node.clip {
                        let bleed = (node.bleed * s).round();
                        visual.SetClip2(&D2D_RECT_F {
                            left: bleed,
                            top: bleed,
                            right: bleed + (node.rect.w * s).round(),
                            bottom: bleed + (node.rect.h * s).round(),
                        })?;
                    }
                }
                if !node.kind.has_content() {
                    continue;
                }
                let size = (
                    ((node.rect.w + 2. * node.bleed) * s).ceil().max(1.) as u32,
                    ((node.rect.h + 2. * node.bleed) * s).ceil().max(1.) as u32,
                );
                if self.surfaces[i]
                    .as_ref()
                    .is_none_or(|(_, have)| *have != size)
                {
                    let surface = self.g.surface(size.0, size.1)?;
                    self.visuals[i].SetContent(&surface)?;
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
            let bleed = (self.scene.nodes[i].bleed * s).round();
            let (scene, g) = (&self.scene, &self.g);
            gfx::draw_surface(surface, *size, s, bleed, |dc| scene.draw(dc, g, i))?;
        }
        self.start_motions()?;
        unsafe { self.g.dcomp.as_ref().ok_or_else(Error::empty)?.Commit()? };
        Ok(())
    }

    /// Hand new scene motions to the compositor as piecewise-linear samples
    /// of the token easing curve.
    fn start_motions(&mut self) -> Result<()> {
        let dcomp = self.g.dcomp.clone().ok_or_else(Error::empty)?;
        let s = self.scene.scale;
        let mut any = false;
        for motion in self.scene.motions.iter_mut().filter(|m| !m.applied) {
            motion.applied = true;
            any = true;
            // Offsets are static pixel offsets (at the target) plus the delta.
            let (base, mul) = match motion.prop {
                Prop::Opacity => (0., 1.),
                Prop::OffsetX => (self.offsets[motion.node].0 - motion.to * s, s),
                Prop::OffsetY => (self.offsets[motion.node].1 - motion.to * s, s),
            };
            let value = |p: f32| base + motion.value_at(p) * mul;
            let elapsed = motion.start.elapsed().as_secs_f32().min(motion.seconds);
            let remaining = (motion.seconds - elapsed).max(0.001);
            let start = elapsed / motion.seconds;
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
                let visual = &self.visuals[motion.node];
                match motion.prop {
                    Prop::Opacity => visual.SetOpacity(&animation)?,
                    Prop::OffsetX => visual.SetOffsetX(&animation)?,
                    Prop::OffsetY => visual.SetOffsetY(&animation)?,
                }
            }
        }
        if any && !self.timer {
            // Only to retire motions and follow a smooth scroll.
            unsafe { SetTimer(Some(self.hwnd), TIMER_MOTION, 16, None) };
            self.timer = true;
        }
        Ok(())
    }

    pub fn activate(&mut self, target: Target) {
        if let Some(Action::Login(on)) = self.scene.activate(target) {
            self.request_login(Some(on));
        }
        self.need = true;
    }

    fn request_login(&mut self, enabled: Option<bool>) {
        let Some(login) = &self.login else { return };
        let (root, settings, sender) = (
            login.root.clone(),
            login.settings_file.clone(),
            login.sender.clone(),
        );
        let hwnd = self.hwnd.0 as isize;
        std::thread::spawn(move || {
            let result = captures_app::login_item::configure(&root, &settings, enabled);
            let _ = sender.send((enabled, result));
            unsafe {
                let _ = PostMessageW(
                    Some(HWND(hwnd as *mut _)),
                    WM_APP_LOGIN,
                    WPARAM(0),
                    LPARAM(0),
                );
            }
        });
    }

    fn login_results(&mut self) {
        let Some(login) = &self.login else { return };
        let results: Vec<_> = login.results.try_iter().collect();
        let row = self
            .scene
            .rows
            .iter()
            .position(|r| r.key == RowKey::Login)
            .expect("login row");
        for (requested, result) in results {
            let detail = match result {
                Ok(on) => {
                    self.scene.set_row(row, on);
                    self.scene.set_enabled(row, true);
                    preferences::LOGIN_ITEM_DETAIL.to_owned()
                }
                Err(error) => {
                    if let Some(requested) = requested {
                        self.scene.set_row(row, !requested);
                    }
                    preferences::login_item_error(&error)
                }
            };
            if self.scene.set_detail(row, detail) {
                self.layout = true;
            }
        }
        self.need = true;
    }

    fn refresh_appearance(&mut self) {
        let settings = captures_settings::load(&self.scene_settings_file()).unwrap_or_default();
        let tokens = appearance::tokens(self.appearance.as_deref(), &settings);
        self.scene.reduced_motion = appearance::reduced_motion();
        if tokens.color("surface-canvas") != self.scene.t.color("surface-canvas")
            || tokens.color("theme-accent") != self.scene.t.color("theme-accent")
        {
            appearance::title_bar(self.hwnd, &tokens);
            self.scene.t = tokens;
            self.layout = true;
            self.need = true;
        }
    }

    fn scene_settings_file(&self) -> PathBuf {
        self.scene.settings_file().to_owned()
    }

    fn set_pointer(&mut self, pointer: Option<(f32, f32)>) {
        self.pointer = pointer;
        let hit = pointer.and_then(|(x, y)| self.scene.hit(x, y));
        self.scene.set_hover(hit);
        self.need = true;
    }

    fn handle(&mut self, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
        let s = self.scene.scale;
        let x = (lp.0 & 0xffff) as i16 as f32 / s;
        let y = ((lp.0 >> 16) & 0xffff) as i16 as f32 / s;
        let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
        match msg {
            WM_SIZE => {
                if lp.0 & 0xffff != 0 && (lp.0 >> 16) & 0xffff != 0 {
                    self.layout = true;
                    self.need = true;
                }
            }
            WM_TIMER if wp.0 == TIMER_MOTION => {
                if !self.scene.tick() {
                    unsafe {
                        let _ = KillTimer(Some(self.hwnd), TIMER_MOTION);
                    }
                    self.timer = false;
                }
                self.need = true;
            }
            WM_APP_LOGIN => self.login_results(),
            WM_SETTINGCHANGE => self.refresh_appearance(),
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
            WM_LBUTTONDOWN => {
                unsafe { SetCapture(self.hwnd) };
                let hit = self.scene.hit(x, y);
                self.scene.pressed = hit;
                // A click focuses the control without showing the ring.
                let focus = hit.filter(|t| self.scene.focusable().contains(t));
                self.scene.set_focus(focus, false);
                self.need = true;
            }
            WM_LBUTTONUP => {
                unsafe {
                    let _ = ReleaseCapture();
                }
                let hit = self.scene.hit(x, y);
                if let Some(pressed) = self.scene.pressed.take()
                    && hit == Some(pressed)
                {
                    self.activate(pressed);
                }
            }
            WM_MOUSEWHEEL => {
                let delta = ((wp.0 >> 16) & 0xffff) as i16 as f32 / 120.;
                let mut point = POINT {
                    x: (lp.0 & 0xffff) as i16 as i32,
                    y: ((lp.0 >> 16) & 0xffff) as i16 as i32,
                };
                unsafe {
                    let _ = ScreenToClient(self.hwnd, &mut point);
                }
                let (px, py) = (point.x as f32 / s, point.y as f32 / s);
                if self.scene.viewport().contains(px, py) {
                    // Chromium's 100 px per notch at the default three lines.
                    self.scene
                        .scroll_by(-delta * appearance::wheel_lines() * 100. / 3.);
                    self.set_pointer(Some((px, py)));
                }
            }
            WM_SETCURSOR => {
                if (lp.0 & 0xffff) as u32 != HTCLIENT {
                    return None;
                }
                let hand = self
                    .pointer
                    .and_then(|(x, y)| self.scene.hit(x, y))
                    .is_some_and(|t| match t {
                        Target::Row(row) => self.scene.rows[row].enabled,
                        _ => true,
                    });
                unsafe {
                    SetCursor(LoadCursorW(None, if hand { IDC_HAND } else { IDC_ARROW }).ok());
                }
                return Some(LRESULT(1));
            }
            WM_KEYDOWN => {
                let vk = VIRTUAL_KEY(wp.0 as u16);
                let page = self.scene.viewport().h * 0.875;
                let now = self.scene.scroll_now();
                match vk {
                    VK_TAB => self.scene.focus_step(!shift),
                    VK_SPACE | VK_RETURN => {
                        if let Some(focus) = self.scene.focus {
                            self.scene.set_focus(Some(focus), true);
                            self.activate(focus);
                        }
                    }
                    VK_UP => self.scene.scroll_by(-40.),
                    VK_DOWN => self.scene.scroll_by(40.),
                    VK_PRIOR => self.scene.set_scroll(now - page, true),
                    VK_NEXT => self.scene.set_scroll(now + page, true),
                    VK_HOME => self.scene.set_scroll(0., true),
                    VK_END => self.scene.set_scroll(f32::MAX, true),
                    _ => return None,
                }
                self.need = true;
            }
            WM_GETMINMAXINFO => {
                let spec = captures_app::app_windows::PREFERENCES;
                let size = window_size(self.hwnd, spec.min_width, spec.min_height);
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

    /// Scene events with the state they report, for `uia::raise`.
    fn take_events(&mut self) -> Vec<(Event, bool, String)> {
        std::mem::take(&mut self.scene.events)
            .into_iter()
            .map(|event| match event {
                Event::Toggled(row) | Event::Detail(row) => {
                    let row = &self.scene.rows[row];
                    (event, row.on, row.detail.clone())
                }
                Event::Focus(_) => (event, false, String::new()),
            })
            .collect()
    }
}

/// Outer window size for a client area of `w`×`h` DIPs at the window's DPI.
fn window_size(hwnd: HWND, w: f32, h: f32) -> (i32, i32) {
    unsafe {
        let dpi = GetDpiForWindow(hwnd);
        let s = dpi as f32 / 96.;
        let mut r = RECT {
            left: 0,
            top: 0,
            right: (w * s).round() as i32,
            bottom: (h * s).round() as i32,
        };
        let _ = AdjustWindowRectExForDpi(
            &mut r,
            WS_OVERLAPPEDWINDOW,
            false,
            WS_EX_NOREDIRECTIONBITMAP,
            dpi,
        );
        (r.right - r.left, r.bottom - r.top)
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_GETOBJECT => {
                // Outside the host borrow: UIA may call straight back in.
                if lp.0 as i32 == UiaRootObjectId {
                    return UiaReturnRawElementProvider(
                        hwnd,
                        wp,
                        lp,
                        &crate::uia::node(crate::uia::Id::Root),
                    );
                }
                return DefWindowProcW(hwnd, msg, wp, lp);
            }
            WM_DESTROY => {
                let _ = UiaReturnRawElementProvider(hwnd, WPARAM(0), LPARAM(0), None);
                PostQuitMessage(0);
                return LRESULT(0);
            }
            WM_ERASEBKGND => return LRESULT(1),
            WM_PAINT => {
                let _ = ValidateRect(Some(hwnd), None);
                return LRESULT(0);
            }
            WM_DPICHANGED => {
                let dpi = (wp.0 & 0xffff) as f32;
                with_host(|h| {
                    h.scene.scale = dpi / 96.;
                    h.layout = true;
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
        if let Some(Some(result)) = with_host(|h| h.handle(msg, wp, lp)) {
            return result;
        }
        DefWindowProcW(hwnd, msg, wp, lp)
    }
}

pub fn run(options: Options) -> Result<()> {
    unsafe {
        let instance = GetModuleHandleW(None)?;
        let class = WNDCLASSW {
            style: CS_DBLCLKS,
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: w!("CapturesPreferences"),
            ..Default::default()
        };
        RegisterClassW(&class);
        let spec = captures_app::app_windows::PREFERENCES;
        let hwnd = CreateWindowExW(
            WS_EX_NOREDIRECTIONBITMAP,
            w!("CapturesPreferences"),
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
        let (w, h) = options
            .size
            .map_or((spec.width, spec.height), |(w, h)| (w as f32, h as f32));
        let (ww, wh) = window_size(hwnd, w, h);
        let _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            ww,
            wh,
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
        let host = Host::new(hwnd, &options)?;
        HOST.with(|h| *h.borrow_mut() = Some(host));
        with_host(|h| h.request_login(None));
        let _ = ShowWindow(hwnd, SW_SHOW);
        loop {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_QUIT {
                    HOST.with(|h| h.borrow_mut().take());
                    return Ok(());
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            let state = with_host(|h| {
                if h.need {
                    h.need = false;
                    if let Err(error) = h.frame() {
                        eprintln!("frame failed: {error:?}");
                    }
                }
                (h.need, h.take_events())
            });
            let Some((again, events)) = state else {
                continue;
            };
            if !events.is_empty() {
                crate::uia::raise(&events);
            }
            if !again {
                let _ = WaitMessage();
            }
        }
    }
}
