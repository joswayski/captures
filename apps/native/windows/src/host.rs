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

use captures_app::controls::select;
use captures_app::preferences;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::DirectComposition::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::INFINITE;
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::Ime::ISC_SHOWUICOMPOSITIONWINDOW;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

use crate::gfx::{self, Gfx};
use crate::scene::{Action, Event, FieldId, Kind, Prop, RowKey, Scene, Target};
use crate::textfield::{self, Key};
use crate::{Options, appearance};

const WM_APP_LOGIN: u32 = WM_APP + 1;
/// Wakes the run loop after a handler marked the frame dirty.
const WM_APP_WAKE: u32 = WM_APP + 2;
/// Microphones from the shared recording stack are ready.
const WM_APP_MICROPHONES: u32 = WM_APP + 3;
const WM_MOUSELEAVE: u32 = 0x02A3;
const TIMER_MOTION: usize = 1;
/// "Changes saved" returns to idle after shipping's two seconds.
const TIMER_SAVED: usize = 2;
const SEGMENTS: usize = 16;

thread_local! {
    static HOST: RefCell<Option<Host>> = const { RefCell::new(None) };
}

/// Run `f` on the host unless it is borrowed (re-entrant messages, UIA calls
/// during a frame) or not created yet. Whatever `f` leaves dirty is
/// guaranteed a frame: messages sent from other threads (an external resize,
/// UIA) are dispatched inside the loop's wait, which would otherwise keep
/// blocking with `need` set.
/// Preferences is open.
pub fn exists() -> bool {
    HOST.with(|h| h.try_borrow().map_or(true, |h| h.is_some()))
}

pub fn with_host<R>(f: impl FnOnce(&mut Host) -> R) -> Option<R> {
    HOST.with(|h| {
        let mut h = h.try_borrow_mut().ok()?;
        let host = h.as_mut()?;
        let result = f(host);
        host.wake();
        Some(result)
    })
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
    /// The select chevrons' compositor rotation.
    rotations: Vec<Option<IDCompositionRotateTransform>>,
    microphones: Receiver<Vec<(String, String)>>,
    surfaces: Vec<Option<(IDCompositionSurface, (u32, u32))>>,
    /// Static visual offsets in pixels, relative to the parent visual.
    offsets: Vec<(f32, f32)>,
    layout: bool,
    tracking: bool,
    timer: bool,
    pointer: Option<(f32, f32)>,
    login: Option<Login>,
    appearance: Option<String>,
    /// Choose… was activated; the picker opens once the host is released.
    pick_folder: bool,
    /// A custom color well was activated; its picker opens likewise.
    pick_color: Option<usize>,
    /// Capture History… was activated; the loop opens it.
    open_history: bool,
    /// A `WM_APP_WAKE` was posted since the loop last checked `need`.
    woken: bool,
    /// The IME is associated (a text field has focus).
    ime: bool,
    /// The pointer is selecting text in the find input.
    dragging: bool,
    /// A `WM_CHAR` high surrogate waiting for its low half.
    high_surrogate: Option<u16>,
    /// The last double-click in a text field, for triple-click select all.
    double_click: Option<(std::time::Instant, f32, f32)>,
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
            let mut rotations = Vec::new();
            for (index, node) in scene.nodes.iter().enumerate() {
                let visual: IDCompositionVisual3 = dcomp.CreateVisual()?.cast()?;
                rotations.push(if matches!(node.kind, Kind::Chevron(_)) {
                    let rotation = dcomp.CreateRotateTransform()?;
                    visual.SetTransform(&rotation)?;
                    Some(rotation)
                } else {
                    None
                });
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
            // A disposable profile never writes the real Run key.
            let login = options
                .history_root
                .clone()
                .filter(|_| !options.profile)
                .map(|root| Login {
                    root,
                    settings_file: options.settings_file.clone(),
                    results,
                    sender,
                });
            appearance::title_bar(hwnd, &scene.t);
            // No IME until a text field has focus, like Chromium.
            textfield::ime_enable(hwnd, false);
            // Microphones enumerate (cpal/WASAPI) off the UI thread.
            let (found, microphones) = channel();
            let target_hwnd = hwnd.0 as isize;
            std::thread::spawn(move || {
                let devices = captures_recording_platform::microphone_devices()
                    .into_iter()
                    .map(|device| (device.id, device.name))
                    .collect();
                let _ = found.send(devices);
                let _ = PostMessageW(
                    Some(HWND(target_hwnd as *mut _)),
                    WM_APP_MICROPHONES,
                    WPARAM(0),
                    LPARAM(0),
                );
            });
            Ok(Self {
                hwnd,
                g,
                scene,
                need: true,
                _target: target,
                visuals,
                rotations,
                microphones,
                surfaces: (0..count).map(|_| None).collect(),
                offsets: vec![(0., 0.); count],
                layout: true,
                tracking: false,
                timer: false,
                pointer: None,
                login,
                appearance: options.appearance.clone(),
                pick_folder: false,
                pick_color: None,
                open_history: false,
                woken: false,
                ime: false,
                dragging: false,
                high_surrogate: None,
                double_click: None,
            })
        }
    }

    /// Queue a wake message when a frame is due and none is queued.
    fn wake(&mut self) {
        if self.need && !self.woken {
            self.woken = true;
            unsafe {
                let _ = PostMessageW(Some(self.hwnd), WM_APP_WAKE, WPARAM(0), LPARAM(0));
            }
        }
    }

    /// Draw and commit now, and wait until the compositor has the frame, so
    /// a resize never shows the old frame beside uncovered desktop.
    fn present(&mut self) {
        self.need = false;
        if let Err(error) = self.frame() {
            eprintln!("frame failed: {error:?}");
        }
        if let Some(dcomp) = &self.g.dcomp {
            unsafe {
                let _ = dcomp.WaitForCommitCompletion();
            }
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
        if std::mem::take(&mut self.scene.retheme) {
            // Custom colors changed; this frame lays out in the new palette.
            let need = self.need;
            self.retheme();
            self.need = need;
        }
        let text = self.scene.text_focused();
        if text != self.ime {
            textfield::ime_enable(self.hwnd, text);
            self.ime = text;
        }
        if self.layout || self.scene.relayout {
            self.layout = false;
            self.scene.relayout = false;
            let (w, h) = self.client_dips();
            self.scene.layout(&self.g, w, h)?;
        }
        self.scene.prepare(&self.g)?;
        let s = self.scene.scale;
        let count = self.scene.nodes.len();
        // Every node's rect snaps to device pixels by rounding its edges, not
        // its origin and size, so neighbours that share an edge in DIPs share
        // it in pixels too: no half-covered row at fractional scales (150%),
        // where the window has no redirection bitmap and the desktop would
        // show through. Visual origins are the snapped origin minus the bleed.
        let mut abs = vec![(0f32, 0f32); count];
        let mut origin = vec![(0f32, 0f32); count];
        let mut snapped = vec![(0f32, 0f32); count];
        for i in 0..count {
            let node = &self.scene.nodes[i];
            let base = node.parent.map_or((0., 0.), |p| abs[p]);
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
                    if let Some(rotation) = &self.rotations[i] {
                        let bleed = (node.bleed * s).round();
                        rotation.SetCenterX2(bleed + snapped[i].0 / 2.)?;
                        rotation.SetCenterY2(bleed + snapped[i].1 / 2.)?;
                        rotation.SetAngle2(node.angle)?;
                    }
                    if node.clip {
                        let bleed = (node.bleed * s).round();
                        visual.SetClip2(&D2D_RECT_F {
                            left: bleed,
                            top: bleed,
                            right: bleed + snapped[i].0,
                            bottom: bleed + snapped[i].1,
                        })?;
                    }
                }
                // Hidden nodes (find highlights, rings, the listbox) get a
                // surface when first shown; `dirty` waits until then.
                if !node.kind.has_content() || !node.visible {
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
            let node = &self.scene.nodes[i];
            let bleed = (node.bleed * s).round();
            // DIPs to pixels per axis, so the rect fills its snapped box.
            let axis = |px: f32, dips: f32| if dips > 0. { px / dips } else { s };
            let scale = (
                axis(snapped[i].0, node.rect.w),
                axis(snapped[i].1, node.rect.h),
            );
            let (scene, g) = (&self.scene, &self.g);
            gfx::draw_surface(surface, *size, scale, bleed, |dc| scene.draw(dc, g, i))?;
        }
        self.start_motions()?;
        if std::mem::take(&mut self.scene.saved_timer) {
            unsafe { SetTimer(Some(self.hwnd), TIMER_SAVED, 2000, None) };
        }
        if std::mem::take(&mut self.scene.caret_blink) {
            self.blink_caret()?;
        }
        unsafe { self.g.dcomp.as_ref().ok_or_else(Error::empty)?.Commit()? };
        Ok(())
    }

    /// Restart the caret's blink, solid first, as a repeating compositor
    /// animation at the system blink rate: no timer and no redraw while it
    /// blinks. The IME windows follow the caret.
    fn blink_caret(&mut self) -> Result<()> {
        let Some(node) = self.scene.caret_node() else {
            return Ok(());
        };
        let dcomp = self.g.dcomp.clone().ok_or_else(Error::empty)?;
        let visual = &self.visuals[node];
        unsafe {
            let blink = GetCaretBlinkTime();
            if blink == 0 || blink == INFINITE {
                visual.SetOpacity2(1.)?;
            } else {
                let on = f64::from(blink) / 1000.;
                let animation = dcomp.CreateAnimation()?;
                animation.AddCubic(0., 1., 0., 0., 0.)?;
                animation.AddCubic(on, 0., 0., 0., 0.)?;
                animation.AddRepeat(2. * on, 2. * on)?;
                visual.SetOpacity(&animation)?;
            }
        }
        self.position_ime();
        Ok(())
    }

    /// Put the IME composition and candidate windows at the caret.
    fn position_ime(&self) {
        let Some(node) = self.scene.caret_node() else {
            return;
        };
        let r = self.scene.absolute(node);
        let s = self.scene.scale;
        let px = |v: f32| (v * s).round() as i32;
        textfield::ime_position(self.hwnd, (px(r.x), px(r.y), px(r.h)));
    }

    /// Ctrl+C, Ctrl+X, Ctrl+V (and Ctrl+Insert, Shift+Delete, Shift+Insert)
    /// in the focused text field; true when consumed.
    fn clipboard_key(&mut self, vk: VIRTUAL_KEY, ctrl: bool, shift: bool) -> bool {
        let Some(id) = self.scene.focused_field() else {
            return false;
        };
        let selected = self.scene.field(id).editor.selected_text();
        match vk {
            VK_C | VK_INSERT if ctrl && !shift => {
                if !selected.is_empty() {
                    textfield::copy(self.hwnd, &selected);
                }
            }
            VK_X if ctrl && !shift => self.cut(),
            VK_DELETE if shift && !ctrl => self.cut(),
            VK_V if ctrl && !shift => self.paste(),
            VK_INSERT if shift && !ctrl => self.paste(),
            _ => return false,
        }
        true
    }

    fn cut(&mut self) {
        let hwnd = self.hwnd;
        self.scene.field_edit(&self.g, |editor| {
            match editor.cut() {
                Some(text) => {
                    textfield::copy(hwnd, &text);
                    // `cut` already removed it; report the change.
                    textfield::Change {
                        text: true,
                        caret: true,
                    }
                }
                None => textfield::Change::default(),
            }
        });
    }

    fn paste(&mut self) {
        if let Some(text) = textfield::paste(self.hwnd) {
            self.scene
                .field_edit(&self.g, |editor| editor.insert(&text));
        }
    }

    /// Shipping `preferencesFindCommand` for a key that reached the window
    /// (a focused select handled its own keys first); true when consumed.
    fn find_key(&mut self, vk: VIRTUAL_KEY, wp: WPARAM, lp: LPARAM) -> bool {
        let event = crate::keys::event(wp, lp);
        let exactly = |ctrl: bool, shift: bool| {
            event.ctrl_key == ctrl && event.shift_key == shift && !event.alt_key && !event.meta_key
        };
        if event.code == "KeyF" && exactly(true, false) {
            self.scene.find_reveal(&self.g);
            return true;
        }
        if !self.scene.find.open {
            return false;
        }
        let plain = !event.ctrl_key && !event.alt_key && !event.meta_key;
        let input = self.scene.focused_field() == Some(FieldId::Find);
        let step = match vk {
            VK_ESCAPE if plain => {
                self.scene.find_close();
                return true;
            }
            VK_F3 if plain => !event.shift_key,
            VK_RETURN if plain && input => !event.shift_key,
            _ if event.code == "KeyG" && exactly(true, false) => true,
            _ if event.code == "KeyG" && exactly(true, true) => false,
            _ => return false,
        };
        self.scene.find_step(step);
        true
    }

    /// Editing keys for the focused text field; true when consumed.
    fn text_key(&mut self, vk: VIRTUAL_KEY, ctrl: bool, shift: bool, alt: bool) -> bool {
        if alt {
            return false;
        }
        // A hex input: Enter commits by blurring, Escape restores the saved
        // value and blurs; neither reaches Find.
        if let Some(FieldId::Hex(i)) = self.scene.focused_field() {
            match vk {
                VK_RETURN => {
                    self.scene.set_focus(None, false);
                    return true;
                }
                VK_ESCAPE => {
                    self.scene.revert_hex(i);
                    self.scene.set_focus(None, false);
                    return true;
                }
                _ => {}
            }
        }
        let key = match vk {
            VK_LEFT => Some(Key::Left),
            VK_RIGHT => Some(Key::Right),
            VK_HOME => Some(Key::Home),
            VK_END => Some(Key::End),
            VK_BACK => Some(Key::Backspace),
            VK_DELETE if !shift => Some(Key::Delete),
            VK_A if ctrl && !shift => Some(Key::SelectAll),
            VK_Z if ctrl && !shift => Some(Key::Undo),
            VK_Z | VK_Y if ctrl => Some(Key::Redo),
            _ => None,
        };
        if let Some(key) = key {
            self.scene
                .field_edit(&self.g, |editor| editor.key(key, shift, ctrl));
            return true;
        }
        if self.clipboard_key(vk, ctrl, shift) {
            return true;
        }
        // Space types through `WM_CHAR`; a single-line input keeps Up and
        // Down from scrolling the page.
        matches!(vk, VK_SPACE | VK_UP | VK_DOWN)
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
            if motion.fresh {
                // A new motion starts with the frame that first shows it, so
                // a slow frame (a re-theme redraws every surface) does not
                // eat into it. Motions re-issued after a layout keep going.
                motion.fresh = false;
                motion.start = std::time::Instant::now();
            }
            // Offsets are static pixel offsets (at the target) plus the delta.
            let (base, mul) = match motion.prop {
                Prop::Opacity | Prop::Angle => (0., 1.),
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
                    Prop::Angle => {
                        if let Some(rotation) = &self.rotations[motion.node] {
                            rotation.SetAngle(&animation)?;
                        }
                    }
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
        match self.scene.activate(target) {
            Some(Action::Login(on)) => self.request_login(Some(on)),
            Some(Action::Retheme) => {
                // A choice made here replaces a `--appearance` override.
                self.appearance = None;
                self.retheme();
            }
            Some(Action::ChooseFolder) => self.pick_folder = true,
            Some(Action::PickColor(i)) => self.pick_color = Some(i),
            Some(Action::OpenHistory) => self.open_history = true,
            Some(Action::KeyboardSettings) => {
                let platform = crate::scene::PLATFORM;
                if let Err(error) = preferences::open_keyboard_settings(platform) {
                    eprintln!("{error}");
                }
            }
            None => {}
        }
        self.need = true;
    }

    /// Rebuild the tokens from the saved appearance and accent; every surface
    /// redraws in the new palette.
    fn retheme(&mut self) {
        let tokens = appearance::tokens(self.appearance.as_deref(), self.scene.settings());
        appearance::title_bar(self.hwnd, &tokens);
        self.scene.t = tokens;
        self.layout = true;
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

    /// Windows' app mode or animation setting changed.
    fn refresh_appearance(&mut self) {
        self.scene.reduced_motion = appearance::reduced_motion();
        let tokens = appearance::tokens(self.appearance.as_deref(), self.scene.settings());
        if tokens.appearance != self.scene.t.appearance {
            self.retheme();
        }
    }

    /// Show the system color picker for custom color `i` (Chromium's
    /// `input[type="color"]` on Windows); called with the host released.
    fn choose_color(hwnd: HWND, i: usize) {
        let Some(current) = with_host(|h| {
            h.pick_color = None;
            h.scene.custom_value(i).to_owned()
        }) else {
            return;
        };
        match crate::dialog::pick_color(hwnd, &current) {
            Ok(Some(hex)) => {
                with_host(|h| {
                    h.scene.set_custom_color(i, hex);
                    h.need = true;
                });
            }
            Ok(None) => {}
            Err(error) => eprintln!("color picker failed: {error}"),
        }
    }

    /// Show the folder picker; called with the host released.
    fn choose_folder(hwnd: HWND) {
        let Some(current) = with_host(|h| {
            h.pick_folder = false;
            h.scene.settings().output_directory.clone()
        }) else {
            return;
        };
        match crate::dialog::pick_folder(hwnd, crate::scene::FOLDER_DIALOG_TITLE, &current) {
            Ok(Some(path)) => {
                with_host(|h| {
                    if h.scene.set_output_directory(path) {
                        h.layout = true;
                        h.need = true;
                    }
                });
            }
            Ok(None) => {}
            Err(error) => eprintln!("folder picker failed: {error}"),
        }
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
            // No menu bar: suppress lone Alt/F10, not Alt+Space or other chords.
            WM_SYSCOMMAND if wp.0 & 0xfff0 == SC_KEYMENU as usize && lp.0 == 0 => {
                return Some(LRESULT(0));
            }
            WM_SIZE => {
                if lp.0 & 0xffff != 0 && (lp.0 >> 16) & 0xffff != 0 {
                    self.layout = true;
                    self.present();
                }
            }
            // Only the loop clears `woken`, after the drain: clearing it here
            // let `with_host` repost while `need` was still set, so the drain
            // never ended and no frame ran.
            WM_APP_WAKE => {}
            WM_TIMER if wp.0 == TIMER_SAVED => {
                unsafe {
                    let _ = KillTimer(Some(self.hwnd), TIMER_SAVED);
                }
                self.scene.clear_saved();
                self.need = true;
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
            WM_APP_MICROPHONES => {
                if let Some(devices) = self.microphones.try_iter().last() {
                    self.scene.set_microphones(devices);
                    self.need = true;
                }
            }
            // Like a DOM blur: recording stops and the listbox closes.
            WM_KILLFOCUS => {
                self.scene.blur();
                self.dragging = false;
                self.need = true;
                return None;
            }
            WM_SETFOCUS => {
                self.scene.window_focus(true);
                self.need = true;
                return None;
            }
            WM_IME_STARTCOMPOSITION if self.scene.text_focused() => {
                // The composition draws inline; no system composition window.
                self.position_ime();
            }
            WM_IME_COMPOSITION if self.scene.text_focused() => {
                let (hwnd, flags) = (self.hwnd, lp.0 as u32);
                self.scene.field_edit(&self.g, |editor| {
                    textfield::ime_composition(hwnd, flags, editor)
                });
                self.need = true;
            }
            WM_IME_ENDCOMPOSITION if self.scene.text_focused() => {
                self.scene
                    .field_edit(&self.g, |editor| editor.commit_composition(&[]));
                self.need = true;
            }
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
                if self.dragging {
                    let position = self.scene.field_hit(x);
                    self.scene
                        .field_edit(&self.g, |editor| editor.move_to(position, true));
                }
                self.set_pointer(Some((x, y)));
            }
            WM_MOUSELEAVE => {
                self.tracking = false;
                self.set_pointer(None);
            }
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
                unsafe { SetCapture(self.hwnd) };
                let hit = self.scene.hit(x, y);
                self.scene.pressed = hit;
                // Options keep focus on their trigger; a click anywhere else
                // closes the listbox (shipping's outside `pointerdown`).
                if !matches!(hit, Some(Target::Option(..))) {
                    if let Some((key, _)) = self.scene.open
                        && hit != Some(Target::Select(key))
                    {
                        self.scene.close_select();
                    }
                    // A click focuses the control without showing the ring.
                    let focus = hit.filter(|t| self.scene.takes_focus(*t));
                    self.scene.set_focus(focus, false);
                }
                if self.scene.field_of(hit).is_some() {
                    self.field_click(msg == WM_LBUTTONDBLCLK, x, y, shift);
                }
                self.need = true;
            }
            WM_LBUTTONUP => {
                unsafe {
                    let _ = ReleaseCapture();
                }
                self.dragging = false;
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
                if self.scene.list_contains(px, py) {
                    self.scene
                        .scroll_list(-delta * appearance::wheel_lines() * 100. / 3.);
                    self.set_pointer(Some((px, py)));
                } else if self.scene.viewport().contains(px, py) {
                    // Chromium's 100 px per notch at the default three lines.
                    self.scene
                        .scroll_by(-delta * appearance::wheel_lines() * 100. / 3.);
                    // An open listbox follows its trigger, like shipping's
                    // scroll listener.
                    self.scene.place_list();
                    self.set_pointer(Some((px, py)));
                }
            }
            WM_SETCURSOR => {
                if (lp.0 & 0xffff) as u32 != HTCLIENT {
                    return None;
                }
                let hit = self.pointer.and_then(|(x, y)| self.scene.hit(x, y));
                let hand = hit.is_some_and(|t| match t {
                    Target::Row(row) => self.scene.rows[row].enabled,
                    Target::Find(part) => self.scene.find_enabled(part),
                    _ => true,
                });
                let cursor = if self.scene.field_of(hit).is_some() {
                    IDC_IBEAM
                } else if hand {
                    IDC_HAND
                } else {
                    IDC_ARROW
                };
                unsafe {
                    SetCursor(LoadCursorW(None, cursor).ok());
                }
                return Some(LRESULT(1));
            }
            WM_KEYDOWN | WM_SYSKEYDOWN | WM_KEYUP | WM_SYSKEYUP
                if self.scene.recording.is_some() =>
            {
                let down = matches!(msg, WM_KEYDOWN | WM_SYSKEYDOWN);
                let alt_f4 = msg == WM_SYSKEYDOWN && VIRTUAL_KEY(wp.0 as u16) == VK_F4;
                if alt_f4 {
                    return None;
                }
                // Every key goes to the recorder (shipping `preventDefault`);
                // a modifier's release only refreshes the held keys.
                if down || crate::keys::is_modifier(wp) {
                    self.scene.record_key(&crate::keys::event(wp, lp));
                }
                self.need = true;
            }
            WM_CHAR if self.scene.text_focused() => self.text_char(wp.0 as u16),
            WM_CHAR if self.scene.open.is_some() => {
                if let Some(ch) = char::from_u32(wp.0 as u32).filter(|c| !c.is_control())
                    && self.scene.type_ahead(ch)
                {
                    self.need = true;
                } else {
                    return None;
                }
            }
            // Alt chords arrive as system keys. Alt+Down and Alt+Up open a
            // focused select, or close an open one keeping the current choice
            // (Windows combo boxes, ARIA combobox); Alt+F4, Alt+Space and the
            // rest stay with DefWindowProc.
            WM_SYSKEYDOWN
                if matches!(VIRTUAL_KEY(wp.0 as u16), VK_DOWN | VK_UP)
                    && matches!(self.scene.focus, Some(Target::Select(_))) =>
            {
                let key = if self.scene.open.is_some() {
                    select::Key::Escape
                } else if VIRTUAL_KEY(wp.0 as u16) == VK_DOWN {
                    select::Key::ArrowDown
                } else {
                    select::Key::ArrowUp
                };
                if !self.scene.select_key(key) {
                    return None;
                }
                self.need = true;
            }
            WM_KEYDOWN => {
                let vk = VIRTUAL_KEY(wp.0 as u16);
                let select_key = match vk {
                    VK_DOWN => Some(select::Key::ArrowDown),
                    VK_UP => Some(select::Key::ArrowUp),
                    VK_HOME => Some(select::Key::Home),
                    VK_END => Some(select::Key::End),
                    VK_RETURN => Some(select::Key::Enter),
                    VK_SPACE => Some(select::Key::Space),
                    VK_ESCAPE => Some(select::Key::Escape),
                    _ => None,
                };
                if let Some(key) = select_key
                    && self.scene.select_key(key)
                {
                    self.need = true;
                    return Some(LRESULT(0));
                }
                let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
                let alt = unsafe { GetKeyState(VK_MENU.0 as i32) } < 0;
                if (self.scene.text_focused() && self.text_key(vk, ctrl, shift, alt))
                    || self.find_key(vk, wp, lp)
                {
                    self.need = true;
                    return Some(LRESULT(0));
                }
                // Not in shipping: Delete or Backspace on a focused recorder
                // restores its default shortcut.
                if let (VK_DELETE | VK_BACK, Some(Target::Shortcut(index))) = (vk, self.scene.focus)
                {
                    self.scene.reset_shortcut(index);
                    self.need = true;
                    return Some(LRESULT(0));
                }
                let page = self.scene.viewport().h * 0.875;
                let now = self.scene.scroll_now();
                match vk {
                    VK_TAB => {
                        self.scene.focus_step(!shift);
                        // Tabbing into a text input selects its text.
                        if self.scene.text_focused() {
                            self.scene.field_edit(&self.g, |editor| editor.select_all());
                        }
                    }
                    VK_SPACE | VK_RETURN => {
                        if let Some(focus) = self.scene.focus {
                            self.scene.set_focus(Some(focus), true);
                            self.activate(focus);
                        }
                    }
                    VK_LEFT | VK_RIGHT | VK_UP | VK_DOWN
                        if let Some(next) =
                            self.scene.choice_step(matches!(vk, VK_RIGHT | VK_DOWN)) =>
                    {
                        // Radio groups: the arrow moves focus and selection.
                        self.scene.set_focus(Some(next), true);
                        self.activate(next);
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

    /// A press in a text field: place the caret (Shift extends), select
    /// a word on double-click and everything on a third click.
    fn field_click(&mut self, double: bool, x: f32, y: f32, shift: bool) {
        let Some(id) = self.scene.focused_field() else {
            return;
        };
        if self.scene.field(id).editor.composing() {
            textfield::ime_complete(self.hwnd);
        }
        let position = self.scene.field_hit(x);
        let near = |(at, px, py): (std::time::Instant, f32, f32)| {
            let time = std::time::Duration::from_millis(u64::from(unsafe { GetDoubleClickTime() }));
            at.elapsed() < time && (px - x).abs() < 4. && (py - y).abs() < 4.
        };
        if double {
            let (start, end) = self.scene.field(id).editor.word_at(position);
            self.scene
                .field_edit(&self.g, |editor| editor.select(start, end));
            self.double_click = Some((std::time::Instant::now(), x, y));
        } else if self.double_click.take().is_some_and(near) {
            self.scene.field_edit(&self.g, |editor| editor.select_all());
        } else {
            self.scene
                .field_edit(&self.g, |editor| editor.move_to(position, shift));
            self.dragging = true;
        }
    }

    /// A `WM_CHAR` for the find input; surrogate pairs arrive in halves and
    /// control characters (Ctrl+letters, Backspace, Enter) do not type.
    fn text_char(&mut self, unit: u16) {
        let units = match unit {
            0xD800..=0xDBFF => {
                self.high_surrogate = Some(unit);
                return;
            }
            0xDC00..=0xDFFF => match self.high_surrogate.take() {
                Some(high) => vec![high, unit],
                None => return,
            },
            _ => vec![unit],
        };
        let text = String::from_utf16_lossy(&units);
        if text.chars().any(char::is_control) {
            return;
        }
        self.scene
            .field_edit(&self.g, |editor| editor.insert(&text));
        self.need = true;
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
                Event::Folder => (event, false, self.scene.settings().output_directory.clone()),
                Event::Value(Target::Select(key)) | Event::Expanded(key, _) => {
                    (event, false, self.scene.select_value(key))
                }
                Event::Value(Target::Shortcut(index)) | Event::Recording(index, _) => {
                    (event, false, self.scene.shortcut_value(index))
                }
                Event::ShortcutError(index) => {
                    (event, false, self.scene.shortcut_error(index).to_owned())
                }
                Event::FieldText(id) => (event, false, self.scene.field_value(id)),
                Event::Value(Target::Well(i)) => {
                    (event, false, self.scene.custom_value(i).to_owned())
                }
                Event::SaveStatus => (event, false, self.scene.save_message()),
                Event::FindCount => (event, false, self.scene.find_count_label()),
                Event::Focus(_)
                | Event::Selected(_)
                | Event::Active(..)
                | Event::Value(_)
                | Event::FindOpen(_)
                | Event::FieldCaret(_) => (event, false, String::new()),
            })
            .collect()
    }
}

/// Outer window size for a client area of `w`×`h` DIPs at the window's DPI.
pub fn window_size(hwnd: HWND, w: f32, h: f32) -> (i32, i32) {
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
                HOST.with(|h| {
                    if let Ok(mut h) = h.try_borrow_mut() {
                        h.take();
                    }
                });
                // The last window quits.
                if !crate::history::exists()
                    && !crate::editor::exists()
                    && !crate::recording::exists()
                    && !crate::setup::exists()
                    && !crate::live::resident()
                {
                    PostQuitMessage(0);
                }
                return LRESULT(0);
            }
            WM_ERASEBKGND => return LRESULT(1),
            WM_IME_SETCONTEXT => {
                // Text fields draw the composition inline.
                let lp = LPARAM(lp.0 & !(ISC_SHOWUICOMPOSITIONWINDOW as isize));
                return DefWindowProcW(hwnd, msg, wp, lp);
            }
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

/// Open Preferences, or bring it forward when it is already open.
pub fn open(options: &Options) -> Result<()> {
    if let Some(hwnd) = with_host(|h| h.hwnd) {
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
            let _ = SetForegroundWindow(hwnd);
        }
        return Ok(());
    }
    unsafe {
        let instance = GetModuleHandleW(None)?;
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
            .filter(|_| options.scene == crate::Scene::Preferences)
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
        let host = Host::new(hwnd, options)?;
        HOST.with(|h| *h.borrow_mut() = Some(host));
        with_host(|h| h.request_login(None));
        let _ = ShowWindow(hwnd, SW_SHOW);
    }
    Ok(())
}

pub fn run(options: Options, resident: Option<captures_app::instance::Instance>) -> Result<()> {
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
        crate::history::register(instance.into())?;
        crate::editor::register(instance.into())?;
        crate::recording::register(instance.into())?;
        crate::setup::register(instance.into())?;
        if let Some(instance) = resident {
            crate::live::start(&options, instance);
        } else {
            match options.scene {
                crate::Scene::Preferences => open(&options)?,
                crate::Scene::History => crate::history::open(&options, options.size)?,
                crate::Scene::Editor => {
                    crate::editor::open(&options, options.artifact.as_deref().unwrap_or_default())?
                }
                // Parsing requires --screenshot, which never runs the host.
                crate::Scene::Capture => {}
                crate::Scene::Setup => crate::setup::open(&options)?,
                crate::Scene::Recording => crate::recording::open(
                    &options,
                    options.artifact.as_deref().unwrap_or_default(),
                )?,
            }
        }
        loop {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_QUIT {
                    HOST.with(|h| h.borrow_mut().take());
                    crate::live::stop();
                    // Closed editors may still be committing labels and
                    // writing drafts; returning would end the process.
                    crate::editor::wait_for_workers();
                    return Ok(());
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            let state = with_host(|h| {
                h.woken = false;
                if h.need {
                    h.need = false;
                    if let Err(error) = h.frame() {
                        eprintln!("frame failed: {error:?}");
                    }
                }
                (
                    h.hwnd,
                    h.need,
                    h.take_events(),
                    h.pick_folder,
                    h.pick_color,
                    std::mem::take(&mut h.open_history),
                )
            });
            crate::live::service();
            // History's frame, like Preferences': only the loop clears `woken`.
            let history = crate::history::with_history(|h| {
                h.woken = false;
                let (again, events, preferences, editor) = h.service();
                (
                    again,
                    crate::history_uia::describe(h, events),
                    preferences,
                    editor,
                )
            });
            if let Some(Some(entry)) = crate::history::with_history(|h| h.restore.take()) {
                crate::previews::restore(&options, &entry);
            }
            let mut again = false;
            if let Some((again_h, events, preferences, editor)) = history {
                again |= again_h;
                if !events.is_empty() {
                    crate::history_uia::raise(&events);
                }
                if preferences && let Err(error) = open(&options) {
                    eprintln!("couldn't open Preferences: {error}");
                }
                if let Some((id, kind)) = editor {
                    let result = if kind.is_recording() {
                        crate::recording::open(&options, &id)
                    } else {
                        crate::editor::open(&options, &id)
                    };
                    if let Err(error) = result {
                        eprintln!("couldn't open the editor for {id}: {error}");
                    }
                }
            }
            // Each editor's frame; again only the loop clears `woken`.
            for hwnd in crate::editor::windows() {
                let serviced = crate::editor::with_editor(hwnd, |e| {
                    e.woken = false;
                    let (again, events) = e.service();
                    (again, crate::editor_uia::describe(e, events))
                });
                if let Some((again_e, events)) = serviced {
                    again |= again_e;
                    if !events.is_empty() {
                        crate::editor_uia::raise(hwnd, &events);
                    }
                }
            }
            for hwnd in crate::recording::windows() {
                let serviced = crate::recording::with_recording(hwnd, |editor| {
                    editor.woken = false;
                    let (again, events) = editor.service();
                    (again, crate::recording_uia::describe(editor, events))
                });
                if let Some((again_r, events)) = serviced {
                    again |= again_r;
                    crate::recording_uia::raise(hwnd, &events);
                }
            }
            if let Some((hwnd, again_p, events, pick_folder, pick_color, open_history)) = state {
                again |= again_p;
                if !events.is_empty() {
                    crate::uia::raise(&events);
                }
                if open_history && let Err(error) = crate::history::open(&options, None) {
                    eprintln!("couldn't open Capture History: {error}");
                }
                if pick_folder {
                    Host::choose_folder(hwnd);
                    continue;
                }
                if let Some(i) = pick_color {
                    Host::choose_color(hwnd, i);
                    continue;
                }
            }
            if !again {
                // Returns for posted, input and sent messages alike; the
                // drain above dispatches them and `need` is checked again.
                let _ =
                    MsgWaitForMultipleObjectsEx(None, INFINITE, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
            }
        }
    }
}
