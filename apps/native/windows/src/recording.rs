//! Retained recording preview, timeline, output cards and save footer, backed by
//! the shared media session. Crop and comparison follow in separate slices; this is
//! an opt-in workbench, not the shipping editor.
use std::cell::RefCell;
use std::path::PathBuf;

use captures_app::recording_editor::RecordingSaveRequest;
use captures_app::recording_editor_ui as ui;
use captures_media::{CancelToken, CropDragHandle};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectComposition::*;
use windows::Win32::Graphics::Gdi::ValidateRect;
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;
use windows_numerics::Matrix3x2;

use crate::gfx::{self, Gfx, Style};
use crate::scene::Rect;
use crate::textfield::{self, Change, Key, TextField};
use crate::tokens::Tokens;
use crate::{Options, appearance};

mod crop;
mod menu;
mod output;
mod preview;
mod timeline;
mod worker;
use output::Output;
use preview::Preview;
use timeline::Timeline;
use worker::{Job, Outcome, Presented, Worker};

pub const SIZE: (f32, f32) = (1280., 840.);
const WM_OUTCOME: u32 = WM_APP + 31;
const WM_WAKE: u32 = WM_APP + 32;
const WM_FRAME: u32 = WM_APP + 33;
const WM_MOUSELEAVE: u32 = 0x02A3;
const OUTPUT_TIMER: usize = 42;

thread_local! {
    static WINDOWS: RefCell<Vec<Recording>> = const { RefCell::new(Vec::new()) };
}

pub fn windows() -> Vec<HWND> {
    WINDOWS.with(|w| w.borrow().iter().map(|w| w.hwnd).collect())
}

pub fn exists() -> bool {
    WINDOWS.with(|w| w.try_borrow().map_or(true, |w| !w.is_empty()))
}

pub fn with_recording<R>(hwnd: HWND, f: impl FnOnce(&mut Recording) -> R) -> Option<R> {
    WINDOWS.with(|w| {
        let mut windows = w.try_borrow_mut().ok()?;
        let window = windows.iter_mut().find(|w| w.hwnd == hwnd)?;
        let result = f(window);
        if (window.pick_folder || window.dirty.iter().any(|&dirty| dirty)) && !window.woken {
            window.woken = true;
            unsafe {
                let _ = PostMessageW(Some(hwnd), WM_WAKE, WPARAM(0), LPARAM(0));
            }
        }
        Some(result)
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Filename,
    Change,
    Copy,
    Cancel,
    Reveal,
    Save,
    Play,
    Loop,
    Fit,
    Actual,
    TrimStart,
    TrimEnd,
    Resolution,
    Width,
    Height,
    Fps,
    GifWidth,
    Quality,
    System,
    SystemVolume,
    Microphone,
    MicrophoneVolume,
    Mono,
    Crop,
    CropLock,
    CropX,
    CropY,
    CropWidth,
    CropHeight,
    CropHandle(CropDragHandle),
}

pub const TARGETS: [Target; 37] = [
    Target::Loop,
    Target::Fit,
    Target::Actual,
    Target::Play,
    Target::CropHandle(CropDragHandle::North),
    Target::CropHandle(CropDragHandle::NorthEast),
    Target::CropHandle(CropDragHandle::East),
    Target::CropHandle(CropDragHandle::SouthEast),
    Target::CropHandle(CropDragHandle::South),
    Target::CropHandle(CropDragHandle::SouthWest),
    Target::CropHandle(CropDragHandle::West),
    Target::CropHandle(CropDragHandle::NorthWest),
    Target::TrimStart,
    Target::TrimEnd,
    Target::Fps,
    Target::GifWidth,
    Target::Crop,
    Target::CropX,
    Target::CropY,
    Target::CropWidth,
    Target::CropHeight,
    Target::CropLock,
    Target::Resolution,
    Target::Width,
    Target::Height,
    Target::Quality,
    Target::System,
    Target::SystemVolume,
    Target::Microphone,
    Target::MicrophoneVolume,
    Target::Mono,
    Target::Change,
    Target::Filename,
    Target::Copy,
    Target::Cancel,
    Target::Reveal,
    Target::Save,
];

impl Target {
    pub fn label(self) -> &'static str {
        match self {
            Self::Filename => "Saved filename",
            Self::Change => "Change save location",
            Self::Copy => "Save as new file",
            Self::Cancel => "Cancel",
            Self::Reveal => "Show in Folder",
            Self::Save => "Save",
            Self::Play => "Play preview",
            Self::Loop => "Loop preview",
            Self::Fit => "Fit",
            Self::Actual => "100%",
            Self::TrimStart => "Trim start",
            Self::TrimEnd => "Trim end",
            Self::Resolution => "Output resolution",
            Self::Width => "Output width",
            Self::Height => "Output height",
            Self::Fps => "GIF frame rate",
            Self::GifWidth => "GIF maximum width",
            Self::Quality => "Compression quality",
            Self::System => "System audio",
            Self::SystemVolume => "System audio volume",
            Self::Microphone => "Microphone",
            Self::MicrophoneVolume => "Microphone volume",
            Self::Mono => "Convert to mono",
            Self::Crop => "Crop recording",
            Self::CropLock => "Lock aspect ratio",
            Self::CropX => "Crop X",
            Self::CropY => "Crop Y",
            Self::CropWidth => "Crop width",
            Self::CropHeight => "Crop height",
            Self::CropHandle(handle) => crop::handle_label(handle),
        }
    }

    fn is_preview(self) -> bool {
        matches!(self, Self::Play | Self::Loop | Self::Fit | Self::Actual)
    }

    fn scrolls(self) -> bool {
        self.is_preview()
            || matches!(self, Self::TrimStart | Self::TrimEnd | Self::CropHandle(_))
            || self.is_output()
    }
}

#[derive(Clone, Copy)]
pub enum Event {
    Focus(Target),
    Value,
    Copy,
    Status,
    Structure,
    Toggle(Target),
    Name(Target),
    Range(Target),
    OutputValue(Target),
    Expanded(Target),
}

// Each control and the caret has its own retained surface. A progress tick
// never redraws the window, filename or header; a caret blink never wakes it.
const BACK: usize = 0;
const FOOTER: usize = 1;
const STATUS: usize = 2;
const PROGRESS: usize = 3;
const CARET: usize = 4;
const PREVIEW: usize = 5;
const TIMELINE: usize = 6;
const OUTPUT: usize = 7;
const CONTROL_START: usize = 8;
const MENU: usize = CONTROL_START + TARGETS.len();
const COUNT: usize = MENU + 1;

struct Layer {
    visual: IDCompositionVisual3,
    surface: Option<(IDCompositionSurface, (u32, u32))>,
}

pub struct Recording {
    pub hwnd: HWND,
    id: String,
    g: Gfx,
    t: Tokens,
    options: Options,
    _target: Option<IDCompositionTarget>,
    layers: Vec<Layer>,
    dirty: [bool; COUNT],
    pub woken: bool,
    pub scale: f32,
    pub width: f32,
    pub height: f32,
    page_scroll: f32,
    rects: [Rect; COUNT],
    worker: Worker,
    presented: Option<Presented>,
    preview: Preview,
    timeline: Timeline,
    output: Output,
    crop_drag: Option<crop::Drag>,
    menu: Option<menu::Menu>,
    pub filename: TextField,
    pub directory: PathBuf,
    source_directory: PathBuf,
    source_stem: String,
    pub make_copy: bool,
    pub focus: Option<Target>,
    focus_visible: bool,
    hover: Option<Target>,
    pressed: Option<Target>,
    dragging: bool,
    tracking: bool,
    high_surrogate: Option<u16>,
    pub status: String,
    pub error: bool,
    loading: bool,
    pub saving: bool,
    progress: u16,
    saved: Option<PathBuf>,
    saved_fingerprint: Option<(PathBuf, bool)>,
    pick_folder: bool,
    closing: bool,
    close_ready: bool,
    events: Vec<Event>,
}

impl Recording {
    fn new(hwnd: HWND, options: &Options, id: &str, g: Gfx, t: Tokens) -> Result<Self> {
        // A corrupt profile must never silently redirect saves to the real
        // default Captures folder.
        let settings = captures_settings::load(&options.settings_file)
            .map_err(|error| Error::new(E_FAIL, format!("Couldn’t read settings: {error}")))?;
        let mut layers: Vec<Layer> = Vec::new();
        let target = if let Some(dcomp) = &g.dcomp {
            let target = unsafe { dcomp.CreateTargetForHwnd(hwnd, true)? };
            for index in 0..COUNT {
                let visual: IDCompositionVisual3 = unsafe { dcomp.CreateVisual()?.cast()? };
                unsafe {
                    if index == BACK {
                        target.SetRoot(&visual)?;
                    } else if index != CARET {
                        // With no reference, false inserts above all siblings:
                        // controls must paint over the opaque footer band.
                        layers[BACK].visual.AddVisual(
                            &visual,
                            false,
                            None::<&IDCompositionVisual>,
                        )?;
                    }
                }
                layers.push(Layer {
                    visual,
                    surface: None,
                });
            }
            // The caret sits above the opaque filename surface, not beneath it.
            unsafe {
                layers[BACK].visual.AddVisual(
                    &layers[CARET].visual,
                    false,
                    None::<&IDCompositionVisual>,
                )?;
            }
            Some(target)
        } else {
            None
        };
        let directory = PathBuf::from(&settings.output_directory);
        let worker = Worker::start(
            hwnd,
            options
                .history_root
                .clone()
                .unwrap_or_else(captures_app::default_history_root),
            id.into(),
            directory.clone(),
            settings.recording.clone(),
        )
        .map_err(|error| Error::new(E_FAIL, error.to_string()))?;
        Ok(Self {
            hwnd,
            id: id.into(),
            g,
            t,
            options: options.clone(),
            _target: target,
            layers,
            dirty: [true; COUNT],
            woken: false,
            scale: unsafe { GetDpiForWindow(hwnd) } as f32 / 96.,
            width: SIZE.0,
            height: SIZE.1,
            page_scroll: 0.,
            rects: [Rect::default(); COUNT],
            worker,
            presented: None,
            preview: Preview::default(),
            timeline: Timeline::default(),
            output: Output::new(&settings.recording),
            crop_drag: None,
            menu: None,
            filename: TextField::new(Style::Option, ""),
            directory,
            source_directory: PathBuf::new(),
            source_stem: String::new(),
            make_copy: false,
            focus: None,
            focus_visible: false,
            hover: None,
            pressed: None,
            dragging: false,
            tracking: false,
            high_surrogate: None,
            status: String::new(),
            error: false,
            loading: true,
            saving: false,
            progress: 0,
            saved: None,
            saved_fingerprint: None,
            pick_folder: false,
            closing: false,
            close_ready: false,
            events: Vec::new(),
        })
    }

    pub fn title(&self) -> &'static str {
        self.presented
            .as_ref()
            .map_or(ui::TITLE_RECORDING, |p| ui::title(&p.mime_type))
    }

    pub fn warning(&self) -> Option<String> {
        self.presented
            .as_ref()
            .and_then(|p| ui::dropped_frames_warning(p.dropped_frames))
    }

    pub fn title_rect(&self) -> Rect {
        let width = (self.width - 2. * self.t.n("s-8")).min(1220.);
        Rect::new(
            (self.width - width) / 2.,
            self.t.n("s-8") - self.page_scroll,
            width,
            34.,
        )
    }

    pub fn warning_rect(&self) -> Rect {
        let title = self.title_rect();
        let height = self
            .warning()
            .and_then(|warning| {
                self.g
                    .text(
                        &self.t,
                        Style::RowDescription,
                        &warning,
                        title.w - 2. * self.t.n("s-5"),
                    )
                    .ok()
            })
            .map_or(0., |text| text.height + 2. * self.t.n("s-4"));
        Rect::new(
            title.x,
            title.y + title.h + self.t.n("s-6"),
            title.w,
            height,
        )
    }

    pub fn text(&self) -> String {
        String::from_utf16_lossy(self.filename.editor.units())
    }

    fn extension(&self) -> &'static str {
        if self
            .presented
            .as_ref()
            .is_some_and(|p| p.mime_type == "image/gif")
        {
            "gif"
        } else if self
            .presented
            .as_ref()
            .is_some_and(|p| p.mime_type.starts_with("video/webm"))
        {
            "webm"
        } else {
            "mp4"
        }
    }

    fn destination(&self) -> PathBuf {
        self.directory
            .join(format!("{}.{}", self.text(), self.extension()))
    }

    pub fn shown(&self, target: Target) -> bool {
        if target.is_output() {
            return self.output_shown(target);
        }
        match target {
            Target::CropHandle(_) => self.crop_shown(),
            Target::Cancel => self.saving,
            Target::Reveal => self.saved.is_some() && !self.saving,
            Target::Play | Target::Loop => self
                .presented
                .as_ref()
                .is_some_and(|p| p.mime_type != "image/gif"),
            _ => true,
        }
    }

    pub fn enabled(&self, target: Target) -> bool {
        if self.closing || !self.shown(target) {
            return false;
        }
        if target == Target::Cancel {
            return self.saving;
        }
        if self.saving || self.loading || self.presented.is_none() {
            return false;
        }
        match target {
            Target::CropX | Target::CropY | Target::CropWidth | Target::CropHeight => {
                self.output.values.crop_on
            }
            Target::SystemVolume => !self.output.values.audio.mute_system_audio,
            Target::MicrophoneVolume => !self.output.values.audio.mute_microphone,
            Target::Play | Target::Save if self.timeline.pending => false,
            Target::Play => self.presented.as_ref().is_some_and(|p| p.duration_ms > 0),
            Target::TrimStart | Target::TrimEnd => {
                self.presented.as_ref().is_some_and(|p| p.duration_ms > 0)
            }
            Target::Save => {
                self.saved_fingerprint.as_ref() != Some(&(self.destination(), self.make_copy))
                    && self.presented.as_ref().is_some_and(|p| {
                        let (edit, export) = self.output.values.specs(
                            p.width,
                            p.height,
                            p.edit.clone(),
                            p.export.clone(),
                        );
                        edit == p.edit && export == p.export
                    })
            }
            Target::Copy => self
                .presented
                .as_ref()
                .is_some_and(|p| p.original.is_some()),
            _ => true,
        }
    }

    pub fn target_rect(&self, target: Target) -> Rect {
        self.rects[slot(target)]
    }

    pub fn hit(&self, x: f32, y: f32) -> Option<Target> {
        if let Some(handle) = self.crop_hit(x, y) {
            return (handle != CropDragHandle::Move).then_some(Target::CropHandle(handle));
        }
        if self.shown(Target::Play)
            && y < self.rects[FOOTER].y
            && self.preview_view().contains(x, y)
            && self.preview_media().contains(x, y)
        {
            return Some(Target::Play);
        }
        TARGETS.into_iter().find(|&t| {
            t != Target::Play
                && self.shown(t)
                && (!t.scrolls() || y < self.rects[FOOTER].y)
                && self.target_rect(t).contains(x, y)
        })
    }

    fn page_scroll_max(&self) -> f32 {
        let timeline = self.rects[OUTPUT];
        (timeline.y + timeline.h + self.page_scroll + self.t.n("s-6") - self.rects[FOOTER].y)
            .max(0.)
    }

    fn scroll_page(&mut self, delta: f32) {
        let next = (self.page_scroll + delta).clamp(0., self.page_scroll_max());
        if next != self.page_scroll {
            self.page_scroll = next;
            let _ = self.layout();
        }
    }

    /// Viewport clipping shared by live composition, screenshots and UIA.
    pub fn visible_target_rect(&self, target: Target) -> Rect {
        let r = if matches!(target, Target::Play | Target::CropHandle(_)) {
            self.preview_control_clip(target)
        } else {
            self.target_rect(target)
        };
        if !target.scrolls() {
            return r;
        }
        self.page_visible_rect(r)
    }

    pub fn page_visible_rect(&self, r: Rect) -> Rect {
        let y = r.y.max(0.);
        Rect::new(
            r.x,
            y,
            r.w,
            ((r.y + r.h).min(self.rects[FOOTER].y) - y).max(0.),
        )
    }

    fn content_clip(&self, index: usize) -> Option<Rect> {
        if index != PREVIEW
            && index != TIMELINE
            && index != OUTPUT
            && !(index == CARET && self.focus.is_some_and(Target::is_number))
            && !((CONTROL_START..MENU).contains(&index) && TARGETS[index - CONTROL_START].scrolls())
        {
            return None;
        }
        let r = if index == slot(Target::Play)
            || ((CONTROL_START..MENU).contains(&index)
                && matches!(TARGETS[index - CONTROL_START], Target::CropHandle(_)))
        {
            self.preview_view()
        } else {
            Rect::new(0., 0., self.width, self.rects[FOOTER].y)
        };
        Some(self.page_visible_rect(r))
    }

    fn layout(&mut self) -> Result<()> {
        let (w, h) = (self.width, self.height);
        let n = |name| self.t.n(name);
        let compact = w <= 820.;
        let footer_h = if compact { 116. } else { 76. };
        let y = h - footer_h;
        self.rects[BACK] = Rect::new(0., 0., w, h);
        self.rects[FOOTER] = Rect::new(0., y, w, footer_h);
        self.rects[PROGRESS] = Rect::new(0., y, w, 3.);
        let pad = if compact { n("s-6") } else { n("s-7") };
        let gap = if compact { n("s-5") } else { n("s-6") };
        let copy_w = 32.
            + n("s-3")
            + self
                .g
                .text(&self.t, Style::Control, Target::Copy.label(), 500.)?
                .width;
        let filename_w = if compact {
            (w - 2. * pad - gap - copy_w).max(120.)
        } else {
            (w - 2. * pad - 2. * gap - copy_w - 340.).clamp(280., 420.)
        };
        let total = filename_w + gap + copy_w + if compact { 0. } else { gap + 340. };
        let x = if compact { pad } else { (w - total) / 2. };
        let field_y = y + n("s-4") + 16. + n("s-2");
        self.rects[slot(Target::Filename)] = Rect::new(x, field_y, filename_w, n("h-md"));
        self.rects[slot(Target::Change)] =
            Rect::new(x + filename_w - 58., y + n("s-4") - 2., 58., 20.);
        self.rects[slot(Target::Copy)] =
            Rect::new(x + filename_w + gap, field_y, copy_w, n("h-md"));
        let action_x = if compact {
            w - pad - 340.
        } else {
            x + filename_w + gap + copy_w + gap
        };
        let action_y = if compact {
            h - n("s-4") - n("h-md")
        } else {
            field_y
        };
        self.rects[slot(Target::Cancel)] = Rect::new(action_x, action_y, 76., n("h-md"));
        self.rects[slot(Target::Reveal)] =
            Rect::new(action_x + 76. + n("s-4"), action_y, 140., n("h-md"));
        self.rects[slot(Target::Save)] = Rect::new(action_x + 240., action_y, 100., n("h-md"));
        self.rects[STATUS] = Rect::new(action_x, action_y - 19., 340., 16.);
        self.filename
            .relayout(&self.g, &self.t, filename_w - 76. - n("s-4"))?;
        self.place_caret();
        self.layout_preview();
        let scroll = self.page_scroll.min(self.page_scroll_max());
        if scroll != self.page_scroll {
            self.page_scroll = scroll;
            self.layout_preview();
        }
        self.dirty.fill(true);
        Ok(())
    }

    fn place_caret(&mut self) {
        if let Some(r) = self.output_caret() {
            self.rects[CARET] = r;
            self.dirty[CARET] = true;
            return;
        }
        let field = self.target_rect(Target::Filename);
        let (x, y, h) = self.filename.caret_box(self.t.n("s-4"), field.h);
        self.rects[CARET] = Rect::new(field.x + x, field.y + y, 1., h);
        self.dirty[CARET] = true;
    }

    pub fn set_focus(&mut self, target: Option<Target>, visible: bool) {
        self.output_focus(target, visible);
        if self.menu_target().is_some_and(|t| Some(t) != target) {
            self.close_output_menu();
        }
        if let Some(old) = self.focus {
            self.dirty[slot(old)] = true;
        }
        let changed = self.focus != target;
        self.focus = target;
        self.focus_visible = visible;
        if visible && target == Some(Target::Play) {
            self.reveal_play();
        }
        if visible && let Some(t) = target {
            self.reveal_crop_handle(t);
        }
        if visible
            && let Some(target) = target
            && target.scrolls()
        {
            let r = self.target_rect(target);
            let delta = if r.y < 4. {
                r.y - 4.
            } else {
                (r.y + r.h + 4. - self.rects[FOOTER].y).max(0.)
            };
            self.scroll_page(delta);
        }
        self.dirty[TIMELINE] = true;
        if let Some(target) = target {
            self.dirty[slot(target)] = true;
            if changed {
                self.events.push(Event::Focus(target));
            }
        }
        if changed && target == Some(Target::Filename) {
            self.filename.editor.select_all();
            self.filename.caret_moved();
        }
        textfield::ime_enable(self.hwnd, target == Some(Target::Filename) && !self.saving);
        self.place_caret();
    }

    pub fn set_value(&mut self, value: &str) {
        let change = self.filename.editor.set_text(value);
        self.edited(change);
    }

    fn edited(&mut self, change: Change) {
        if !change.any() {
            return;
        }
        let _ = self.filename.apply(&self.g, &self.t, change);
        self.dirty[slot(Target::Filename)] = true;
        self.place_caret();
        if change.text {
            self.notify("", false);
            self.dirty[slot(Target::Save)] = true;
            self.events.push(Event::Value);
        }
    }

    fn notify(&mut self, text: &str, error: bool) {
        self.status = text.into();
        self.error = error;
        self.dirty[STATUS] = true;
        self.events.push(Event::Status);
    }

    pub fn activate(&mut self, target: Target) {
        if !target.is_number() {
            self.commit_output_number();
        }
        if !self.enabled(target) {
            return;
        }
        if target.is_output() {
            self.activate_output(target);
            return;
        }
        match target {
            Target::TrimStart | Target::TrimEnd => {}
            Target::Play => {
                if self.preview.playing {
                    self.stop_preview();
                } else {
                    // Match Tauri's replay tolerance, including a full-end
                    // still parked at duration - 1 ms after a trim edit.
                    if self.preview.position_ms >= self.timeline.end.saturating_sub(10) {
                        self.preview.position_ms = self.timeline.start;
                        self.dirty[TIMELINE] = true;
                    }
                    self.start_preview();
                }
            }
            Target::Loop => {
                self.worker
                    .looping
                    .fetch_xor(true, std::sync::atomic::Ordering::Relaxed);
                self.dirty[slot(target)] = true;
                self.events.push(Event::Toggle(target));
            }
            Target::Fit | Target::Actual => {
                self.preview.actual = target == Target::Actual;
                self.preview.scroll = (0., 0.);
                self.layout_preview();
                self.events.push(Event::Toggle(Target::Fit));
                self.events.push(Event::Toggle(Target::Actual));
            }
            Target::Filename => self.set_focus(Some(target), true),
            Target::Change => self.pick_folder = true,
            Target::Copy => {
                self.make_copy = !self.make_copy;
                let edited = ui::edited_file_stem(&self.source_stem);
                if self.make_copy
                    && self.text() == self.source_stem
                    && self.directory == self.source_directory
                {
                    self.set_value(&edited);
                } else if !self.make_copy && self.text() == edited {
                    self.set_value(&self.source_stem.clone());
                }
                self.saved = None;
                self.saved_fingerprint = None;
                self.notify("", false);
                self.refresh_footer();
                self.events.push(Event::Copy);
            }
            Target::Cancel => {
                if let Some(cancel) = &self.worker.cancel {
                    cancel.cancel();
                }
            }
            Target::Reveal => {
                self.stop_preview();
                if let Some(path) = &self.saved {
                    let _ = self.worker.jobs.send(Job::Reveal(path.clone()));
                }
            }
            Target::Save => {
                if let Some(error) = ui::filename_error(&self.text()) {
                    self.notify(error, true);
                    return;
                }
                if self.extension() == "webm" {
                    self.notify(ui::WEBM_EXPORT_ERROR, true);
                    return;
                }
                let Some(presented) = &self.presented else {
                    return;
                };
                let cancel = CancelToken::default();
                let request = RecordingSaveRequest {
                    destination: self.destination(),
                    export: presented.export.clone(),
                };
                self.stop_preview();
                if self
                    .worker
                    .jobs
                    .send(Job::Save(request, self.make_copy, cancel.clone()))
                    .is_err()
                {
                    self.notify(
                        "The recording session closed. Reopen the editor to save.",
                        true,
                    );
                    return;
                }
                self.worker.cancel = Some(cancel);
                self.saving = true;
                self.progress = 0;
                self.notify("Preparing…", false);
                self.refresh_footer();
            }
            _ => {}
        }
    }

    fn refresh_footer(&mut self) {
        self.dirty[FOOTER..].fill(true);
        self.events.push(Event::Structure);
    }

    fn outcomes(&mut self) {
        while let Ok(outcome) = self.worker.outcomes.try_recv() {
            self.apply_outcome(outcome);
        }
    }

    fn apply_outcome(&mut self, outcome: Outcome) {
        match outcome {
            Outcome::Opened(result) => {
                self.loading = false;
                match result {
                    Ok(p) => {
                        self.directory = PathBuf::from(&p.defaults.directory);
                        self.source_directory = self.directory.clone();
                        self.source_stem = p.defaults.stem.clone();
                        self.make_copy = p.original.is_none();
                        self.set_value(&p.defaults.stem);
                        self.set_preview_frame(p.frame.clone());
                        let gif = p.mime_type == "image/gif";
                        self.timeline.start = p.edit.trim_start_ms;
                        self.timeline.end = p.edit.trim_end_ms.unwrap_or(p.duration_ms).max(1);
                        self.presented = Some(p);
                        self.reset_output();
                        self.layout_preview();
                        if !self.hwnd.is_invalid() {
                            self.load_timeline();
                        }
                        if gif && !self.hwnd.is_invalid() && !self.closing {
                            self.worker
                                .looping
                                .store(true, std::sync::atomic::Ordering::Relaxed);
                            self.start_preview();
                        }
                    }
                    Err(error) => self.notify(&error, true),
                }
                self.dirty.fill(true);
                self.events.push(Event::Structure);
            }
            Outcome::Preview(generation, p, error) => {
                if generation != self.timeline.generation || self.closing {
                    return;
                }
                let crop_was_shown = self.crop_shown();
                self.timeline.pending = false;
                self.timeline.start = p.edit.trim_start_ms;
                self.timeline.end = p.edit.trim_end_ms.unwrap_or(p.duration_ms).max(1);
                self.preview.position_ms = p.position_ms;
                self.set_preview_frame(p.frame.clone());
                self.presented = Some(*p);
                if let Some(error) = error {
                    self.reset_output();
                    self.timeline.resume = false;
                    self.notify(&error, true);
                } else if self.saved.is_none() {
                    self.notify("", false);
                }
                if crop_was_shown != self.crop_shown() {
                    self.events.push(Event::Structure);
                }
                let _ = self.layout();
                self.refresh_footer();
                self.events.push(Event::Range(Target::TrimStart));
                self.events.push(Event::Range(Target::TrimEnd));
                self.resume_timeline();
            }
            Outcome::Thumbnails(result) => {
                self.worker.thumbnail_cancel.take();
                match result {
                    Ok(strip) => match strip.bitmap(&self.g.dc) {
                        Ok(bitmap) => self.timeline.strip = Some(bitmap),
                        Err(error) => {
                            self.notify(&format!("Timeline preview unavailable: {error}"), true)
                        }
                    },
                    Err(error) if !self.closing => {
                        self.notify(&format!("Timeline preview unavailable: {error}"), true)
                    }
                    Err(_) => {}
                }
                self.dirty[TIMELINE] = true;
            }
            Outcome::Progress(progress) => {
                self.progress = progress.completed_per_mille.min(1000);
                self.notify(&worker::progress_text(&progress), false);
                self.dirty[PROGRESS] = true;
            }
            Outcome::Saved(result) => {
                self.worker.cancel.take();
                self.saving = false;
                match result {
                    Ok(saved) => {
                        self.saved_fingerprint = Some((saved.path.clone(), self.make_copy));
                        self.saved = Some(saved.path);
                        if let Some(p) = saved.replaced {
                            self.source_directory = PathBuf::from(&p.defaults.directory);
                            self.source_stem = p.defaults.stem.clone();
                            self.set_preview_frame(p.frame.clone());
                            self.preview.position_ms = 0;
                            self.timeline.start = p.edit.trim_start_ms;
                            self.timeline.end = p.edit.trim_end_ms.unwrap_or(p.duration_ms).max(1);
                            self.presented = Some(p);
                            self.reset_output();
                            self.load_timeline();
                            self.layout_preview();
                            self.dirty[BACK] = true;
                            // Replacement rebases the shared session to source-preserving
                            // defaults. Accept the displayed GIF defaults before another save.
                            if self.extension() == "gif" {
                                self.seek_timeline(0);
                            }
                        }
                        if let Some(warning) = saved.warning {
                            self.notify(
                                &format!(
                                    "Saved, but Capture History could not be updated: {warning}"
                                ),
                                true,
                            );
                        } else {
                            self.notify(
                                &ui::saved_message(self.extension() == "gif", saved.size),
                                false,
                            );
                        }
                    }
                    Err(error)
                        if error == captures_media::MediaToolError::Cancelled.to_string() =>
                    {
                        self.notify("Save cancelled.", false);
                    }
                    Err(error) => self.notify(&error, true),
                }
                self.refresh_footer();
                if self.focus == Some(Target::Cancel) && !self.closing {
                    let target = if self.enabled(Target::Save) {
                        Target::Save
                    } else {
                        Target::Reveal
                    };
                    self.set_focus(Some(target), self.focus_visible);
                }
                if self.extension() == "gif" && !self.closing && !self.hwnd.is_invalid() {
                    self.start_preview();
                }
            }
            Outcome::Revealed(result) => {
                if let Err(error) = result {
                    self.notify(&error, true);
                }
                if self.extension() == "gif" && !self.closing && !self.hwnd.is_invalid() {
                    self.start_preview();
                }
            }
            Outcome::PlaybackEnded(generation, result) => {
                self.take_preview_frame();
                if generation == self.preview.generation {
                    // Seeking into the final frame's hold interval can leave
                    // an empty playback tail. Retry a loop from trim start,
                    // but do not keep retrying a trim that is itself empty.
                    let resume = self
                        .worker
                        .looping
                        .load(std::sync::atomic::Ordering::Relaxed)
                        && self.preview.position_ms > self.timeline.start;
                    self.worker.playback_cancel.take();
                    self.preview.playing = false;
                    self.preview.position_ms = self.timeline.start;
                    self.dirty[TIMELINE] = true;
                    self.dirty[slot(Target::Play)] = true;
                    self.events.push(Event::Name(Target::Play));
                    if let Err(error) = result {
                        self.notify(&error, true);
                    } else if !self.closing {
                        self.timeline.resume = resume;
                        self.seek_timeline(self.timeline.start);
                    }
                }
            }
            Outcome::Closed => {
                self.close_ready = true;
            }
        }
    }

    fn request_close(&mut self) {
        if self.closing {
            return;
        }
        self.closing = true;
        self.worker.close();
        self.refresh_footer();
    }

    fn draw_text(
        &self,
        dc: &ID2D1DeviceContext,
        style: Style,
        text: &str,
        r: Rect,
        color: &str,
        right: bool,
    ) -> Result<()> {
        let text = self.g.text(&self.t, style, text, r.w.max(1.))?;
        self.g.ellipsis(&text, r.w)?;
        gfx::text(
            dc,
            &text,
            if right {
                r.x + r.w - text.width.min(r.w)
            } else {
                r.x
            },
            r.y + (r.h - text.height) / 2.,
            self.t.color(color),
        )
    }

    fn draw(&self, dc: &ID2D1DeviceContext, index: usize) -> Result<()> {
        let r = self.rects[index];
        let local = gfx::rect(0., 0., r.w, r.h);
        let t = &self.t;
        let text = |style, text: &str, area, color, right| {
            self.draw_text(dc, style, text, area, color, right)
        };
        match index {
            BACK => {
                gfx::fill(dc, local, t.color("surface-canvas"))?;
                let footer = self.rects[FOOTER];
                gfx::shadow(
                    dc,
                    gfx::rect(0., footer.y, footer.w, footer.y + footer.h),
                    0.,
                    t.shadow("shadow-lg"),
                )?;
                let title = self.title_rect();
                text(Style::RecordingTitle, self.title(), title, "text", false)?;
                if let Some(warning) = self.warning() {
                    let r = self.warning_rect();
                    gfx::fill_round(
                        dc,
                        gfx::rect(r.x, r.y, r.x + r.w, r.y + r.h),
                        t.n("r-md"),
                        t.color("caution-surface"),
                    )?;
                    text(
                        Style::RowDescription,
                        &warning,
                        Rect::new(r.x + t.n("s-5"), r.y, r.w - 2. * t.n("s-5"), r.h),
                        "caution-text",
                        false,
                    )?;
                }
                if self.loading {
                    text(
                        Style::Body,
                        "Loading recording…",
                        Rect::new(title.x, 120., title.w, 24.),
                        "text-muted",
                        false,
                    )?;
                }
            }
            FOOTER => {
                gfx::fill(dc, local, t.color("surface-raised"))?;
                gfx::fill(dc, gfx::rect(0., 0., r.w, 1.), t.color("border-subtle"))?;
                let field = self.target_rect(Target::Filename);
                let change = self.target_rect(Target::Change);
                text(
                    Style::Status,
                    "Filename",
                    Rect::new(field.x, t.n("s-4"), 60., 16.),
                    "text-subtle",
                    false,
                )?;
                let folder = format!("Saving to {}", self.directory.display());
                text(
                    Style::MonoTiny,
                    &folder,
                    Rect::new(
                        field.x + 65.,
                        t.n("s-4"),
                        (change.x - field.x - 69.).max(1.),
                        16.,
                    ),
                    "text-subtle",
                    true,
                )?;
            }
            STATUS => text(
                Style::Status,
                &self.status,
                Rect::new(0., 0., r.w, r.h),
                if self.error {
                    "danger-text"
                } else if self.saved.is_some() && !self.saving {
                    "positive-text"
                } else {
                    "text-subtle"
                },
                true,
            )?,
            PROGRESS => {
                if self.saving {
                    gfx::fill(dc, local, t.color("surface-sunken"))?;
                    gfx::fill(
                        dc,
                        gfx::rect(0., 0., r.w * f32::from(self.progress) / 1000., 3.),
                        t.color("info-text"),
                    )?;
                }
            }
            CARET => gfx::fill(dc, local, t.color("text"))?,
            PREVIEW => self.draw_preview(dc)?,
            TIMELINE => self.draw_timeline(dc)?,
            OUTPUT => self.draw_output(dc)?,
            MENU => self.draw_output_menu(dc)?,
            _ => {
                let target = TARGETS[index - CONTROL_START];
                if matches!(target, Target::CropHandle(_)) {
                    return self.draw_crop_handle(dc, target);
                }
                if target.is_output() {
                    return self.draw_output_control(dc, target);
                }
                if matches!(target, Target::TrimStart | Target::TrimEnd) {
                    return self.draw_trim_handle(dc, target);
                }
                if target.is_preview() {
                    return self.draw_preview_control(dc, target);
                }
                let active = self.hover == Some(target) && self.enabled(target);
                let focused = self.focus == Some(target) && self.focus_visible;
                if target == Target::Filename {
                    gfx::fill_round(dc, local, t.n("r-md"), t.color("surface-field"))?;
                    gfx::border(
                        dc,
                        local,
                        t.n("r-md"),
                        1.,
                        t.color(if self.focus == Some(target) {
                            "theme-accent"
                        } else {
                            "control-border"
                        }),
                    )?;
                    self.filename.draw(
                        dc,
                        t,
                        (t.n("s-4"), r.w - 76. - t.n("s-4"), r.h),
                        self.focus == Some(target),
                    )?;
                    gfx::fill(
                        dc,
                        gfx::rect(r.w - 76., 1., r.w - 75., r.h - 1.),
                        t.color("control-border"),
                    )?;
                    text(
                        Style::Option,
                        &format!(".{}", self.extension()),
                        Rect::new(r.w - 64., 0., 60., r.h),
                        "text-subtle",
                        false,
                    )?;
                } else if target == Target::Copy {
                    let y = (r.h - 19.) / 2.;
                    gfx::fill_round(
                        dc,
                        gfx::rect(0., y, 32., y + 19.),
                        9.5,
                        t.color(if self.make_copy {
                            "theme-accent"
                        } else {
                            "control-border"
                        }),
                    )?;
                    let x = if self.make_copy { 16. } else { 3. };
                    gfx::fill_round(
                        dc,
                        gfx::rect(x, y + 3., x + 13., y + 16.),
                        6.5,
                        t.color("surface-raised"),
                    )?;
                    text(
                        Style::Control,
                        target.label(),
                        Rect::new(32. + t.n("s-3"), 0., r.w - 32. - t.n("s-3"), r.h),
                        "text-muted",
                        false,
                    )?;
                } else {
                    let primary = target == Target::Save;
                    gfx::fill_round(
                        dc,
                        local,
                        t.n("r-md"),
                        t.color(if primary {
                            "theme-accent"
                        } else if active {
                            "surface-hover"
                        } else {
                            "surface-raised"
                        }),
                    )?;
                    if !primary {
                        gfx::border(dc, local, t.n("r-md"), 1., t.color("control-border"))?;
                    }
                    let label = if target == Target::Change {
                        "Change…"
                    } else {
                        target.label()
                    };
                    let style = if target == Target::Change {
                        Style::Tiny
                    } else {
                        Style::Control
                    };
                    let measured = self.g.text(t, style, label, r.w)?;
                    let icon = match target {
                        Target::Save => Some("save"),
                        Target::Reveal => Some("folder"),
                        _ => None,
                    };
                    let total = measured.width + if icon.is_some() { 21. } else { 0. };
                    let x = (r.w - total) / 2.;
                    let color = if primary {
                        "theme-accent-ink"
                    } else {
                        "text-muted"
                    };
                    if let Some(icon) = icon {
                        gfx::icon(
                            dc,
                            &self.g.factory,
                            icon,
                            &[],
                            (x, (r.h - 15.) / 2., 15.),
                            1.8,
                            t.color(color),
                        )?;
                    }
                    gfx::text(
                        dc,
                        &measured,
                        x + if icon.is_some() { 21. } else { 0. },
                        (r.h - measured.height) / 2.,
                        t.color(color),
                    )?;
                }
                if focused {
                    gfx::ring(dc, local, t.n("r-md"), 2., 4., t.color("theme-accent"))?;
                }
            }
        }
        Ok(())
    }

    fn bleed(&self, index: usize) -> f32 {
        let shadow = if index == slot(Target::Play) || index == MENU {
            Some("shadow-lg")
        } else if index == PREVIEW || index == TIMELINE || index == OUTPUT {
            Some("shadow-sm")
        } else {
            None
        };
        if let Some(shadow) = shadow {
            // gfx::shadow paints out to half the blur plus spread and offset.
            // Keep that ink (and its antialiasing) inside the retained surface.
            self.t
                .shadow(shadow)
                .iter()
                .map(|s| s.x.abs().max(s.y.abs()) + s.spread + s.blur / 2. + 1.)
                .fold(4., f32::max)
                .ceil()
        } else if index >= CONTROL_START {
            4.
        } else {
            0.
        }
    }

    fn opacity(&self, index: usize) -> f32 {
        if index == MENU {
            return f32::from(self.menu.is_some());
        }
        if index == CARET {
            f32::from(
                self.output_caret_visible()
                    || (self.focus == Some(Target::Filename)
                        && self.enabled(Target::Filename)
                        && self.filename.editor.selection().0
                            == self.filename.editor.selection().1),
            )
        } else if index >= CONTROL_START {
            let target = TARGETS[index - CONTROL_START];
            if !self.shown(target)
                || (target == Target::Play
                    && self.preview.playing
                    && !self.preview.hovered
                    && !(self.focus == Some(target) && self.focus_visible))
            {
                0.
            } else if !self.enabled(target) {
                0.5
            } else {
                1.
            }
        } else {
            1.
        }
    }

    /// The same control drawing and opacity as the retained surfaces, for
    /// the workbench's existing Direct2D screenshot path.
    pub fn paint(&self, dc: &ID2D1DeviceContext) -> Result<()> {
        for index in (0..COUNT).filter(|&i| i != CARET).chain([CARET]) {
            let opacity = self.opacity(index);
            if opacity == 0. {
                continue;
            }
            let r = self.rects[index];
            let bleed = self.bleed(index);
            let mut clip = Rect::new(r.x - bleed, r.y - bleed, r.w + 2. * bleed, r.h + 2. * bleed);
            if let Some(view) = self.content_clip(index) {
                // Match both the retained surface's bounds and its viewport
                // clip; otherwise offscreen renders hide surface-bleed bugs.
                let (x, y) = (clip.x.max(view.x), clip.y.max(view.y));
                clip = Rect::new(
                    x,
                    y,
                    ((clip.x + clip.w).min(view.x + view.w) - x).max(0.),
                    ((clip.y + clip.h).min(view.y + view.h) - y).max(0.),
                );
            }
            unsafe {
                dc.SetTransform(&Matrix3x2::identity());
                dc.PushLayer(
                    &D2D1_LAYER_PARAMETERS1 {
                        contentBounds: gfx::rect(clip.x, clip.y, clip.x + clip.w, clip.y + clip.h),
                        opacity,
                        maskTransform: Matrix3x2::identity(),
                        ..Default::default()
                    },
                    None::<&ID2D1Layer>,
                );
                dc.PushAxisAlignedClip(
                    &gfx::rect(clip.x, clip.y, clip.x + clip.w, clip.y + clip.h),
                    D2D1_ANTIALIAS_MODE_ALIASED,
                );
                dc.SetTransform(&Matrix3x2 {
                    M31: r.x,
                    M32: r.y,
                    ..Matrix3x2::identity()
                });
                let result = self.draw(dc, index);
                dc.SetTransform(&Matrix3x2::identity());
                dc.PopAxisAlignedClip();
                dc.PopLayer();
                result?;
            }
        }
        Ok(())
    }

    fn frame(&mut self) -> Result<()> {
        for index in 0..COUNT {
            if !std::mem::take(&mut self.dirty[index]) {
                continue;
            }
            if self.opacity(index) == 0. {
                unsafe {
                    self.layers[index].visual.SetOpacity2(0.)?;
                }
                continue;
            }
            let r = self.rects[index];
            let s = self.scale;
            let bleed = (self.bleed(index) * s).ceil();
            let inner = ((r.w * s).round().max(1.), (r.h * s).round().max(1.));
            let size = ((inner.0 + 2. * bleed) as u32, (inner.1 + 2. * bleed) as u32);
            if self.layers[index]
                .surface
                .as_ref()
                .is_none_or(|(_, old)| *old != size)
            {
                let surface = self.g.surface(size.0, size.1)?;
                unsafe {
                    self.layers[index].visual.SetContent(&surface)?;
                }
                self.layers[index].surface = Some((surface, size));
            }
            let surface = &self.layers[index]
                .surface
                .as_ref()
                .expect("created surface")
                .0;
            gfx::draw_surface(
                surface,
                size,
                (inner.0 / r.w.max(1.), inner.1 / r.h.max(1.)),
                bleed,
                |dc| self.draw(dc, index),
            )?;
            let visual = &self.layers[index].visual;
            unsafe {
                visual.SetOffsetX2((r.x * s).round() - bleed)?;
                visual.SetOffsetY2((r.y * s).round() - bleed)?;
                if let Some(clip) = self.content_clip(index) {
                    visual.SetClip2(&gfx::rect(
                        (clip.x - r.x) * s + bleed,
                        (clip.y - r.y) * s + bleed,
                        (clip.x + clip.w - r.x) * s + bleed,
                        (clip.y + clip.h - r.y) * s + bleed,
                    ))?;
                } else {
                    visual.SetClip2(&gfx::rect(0., 0., size.0 as f32, size.1 as f32))?;
                }
                let opacity = self.opacity(index);
                visual.SetOpacity2(opacity)?;
                if index == CARET && opacity > 0. {
                    let blink = GetCaretBlinkTime();
                    if blink != 0 && blink != u32::MAX {
                        let animation = self
                            .g
                            .dcomp
                            .as_ref()
                            .expect("composition")
                            .CreateAnimation()?;
                        let on = f64::from(blink) / 1000.;
                        animation.AddCubic(0., 1., 0., 0., 0.)?;
                        animation.AddCubic(on, 0., 0., 0., 0.)?;
                        animation.AddRepeat(2. * on, 2. * on)?;
                        visual.SetOpacity(&animation)?;
                    }
                    textfield::ime_position(
                        self.hwnd,
                        ((r.x * s) as i32, (r.y * s) as i32, (r.h * s) as i32),
                    );
                }
            }
        }
        unsafe { self.g.dcomp.as_ref().expect("composition").Commit() }
    }

    pub fn service(&mut self) -> (bool, Vec<Event>) {
        if self.dirty.iter().any(|&d| d)
            && let Err(error) = self.frame()
        {
            eprintln!("recording frame: {error}");
        }
        (
            self.dirty.iter().any(|&d| d),
            std::mem::take(&mut self.events),
        )
    }

    fn key(&mut self, key: VIRTUAL_KEY) {
        let down = |key: VIRTUAL_KEY| unsafe { GetKeyState(i32::from(key.0)) < 0 };
        let (ctrl, shift) = (down(VK_CONTROL), down(VK_SHIFT));
        if self.crop_key(key, shift) {
            return;
        }
        if self.output_key(key, ctrl, shift) {
            return;
        }
        if key == VK_ESCAPE && (self.timeline.drag.is_some() || self.timeline.scrubbing) {
            self.end_timeline_drag();
            unsafe {
                let _ = ReleaseCapture();
            }
            return;
        }
        if let Some(target) = self.focus
            && let Some((value, _, _)) = self.trim_range(target)
        {
            let duration = self.presented.as_ref().map_or(1, |p| p.duration_ms);
            let step = if duration < 60_000 { 1. } else { 10. };
            let delta = match key {
                VK_LEFT | VK_DOWN => Some(-step),
                VK_RIGHT | VK_UP => Some(step),
                VK_PRIOR => Some(1000.),
                VK_NEXT => Some(-1000.),
                _ => None,
            };
            if let Some(delta) = delta {
                self.set_trim(target, value as f64 + delta);
                return;
            }
        }
        if key == VK_TAB {
            let order: Vec<_> = TARGETS.into_iter().filter(|&t| self.enabled(t)).collect();
            if order.is_empty() {
                return;
            }
            let i = self.focus.and_then(|t| order.iter().position(|&x| x == t));
            let next = i.map_or(if shift { order.len() - 1 } else { 0 }, |i| {
                if shift {
                    (i + order.len() - 1) % order.len()
                } else {
                    (i + 1) % order.len()
                }
            });
            self.set_focus(Some(order[next]), true);
            return;
        }
        if self.focus != Some(Target::Filename) || !self.enabled(Target::Filename) {
            if (key == VK_RETURN || key == VK_SPACE)
                && let Some(target) = self.focus
            {
                self.activate(target);
            }
            return;
        }
        let edit = match key {
            VK_LEFT => Some(Key::Left),
            VK_RIGHT => Some(Key::Right),
            VK_HOME => Some(Key::Home),
            VK_END => Some(Key::End),
            VK_BACK => Some(Key::Backspace),
            VK_DELETE => Some(Key::Delete),
            VK_A if ctrl => Some(Key::SelectAll),
            VK_Z if ctrl => Some(if shift { Key::Redo } else { Key::Undo }),
            VK_Y if ctrl => Some(Key::Redo),
            _ => None,
        };
        if let Some(key) = edit {
            let change = self.filename.editor.key(key, shift, ctrl);
            self.edited(change);
        } else if ctrl && key == VK_C {
            textfield::copy(self.hwnd, &self.filename.editor.selected_text());
        } else if ctrl && key == VK_X {
            if let Some(text) = self.filename.editor.cut() {
                textfield::copy(self.hwnd, &text);
                self.edited(Change {
                    text: true,
                    caret: true,
                });
            }
        } else if ctrl
            && key == VK_V
            && let Some(text) = textfield::paste(self.hwnd)
        {
            let change = self.filename.editor.insert(&text);
            self.edited(change);
        }
    }

    fn handle(&mut self, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
        match msg {
            // No menu bar: suppress lone Alt/F10, not Alt+Space or other chords.
            WM_SYSCOMMAND if wp.0 & 0xfff0 == SC_KEYMENU as usize && lp.0 == 0 => {
                return Some(LRESULT(0));
            }
            WM_OUTCOME => self.outcomes(),
            WM_FRAME => self.take_preview_frame(),
            WM_TIMER if wp.0 == OUTPUT_TIMER => self.apply_output_due(),
            WM_WAKE => {}
            WM_SIZE if wp.0 != SIZE_MINIMIZED as usize => {
                self.width = (lp.0 as u32 & 0xffff) as f32 / self.scale;
                self.height = ((lp.0 as u32 >> 16) & 0xffff) as f32 / self.scale;
                let _ = self.layout();
            }
            WM_SETTINGCHANGE | WM_THEMECHANGED => {
                let settings =
                    captures_settings::load(&self.options.settings_file).unwrap_or_default();
                self.t = appearance::tokens(self.options.appearance.as_deref(), &settings);
                appearance::title_bar(self.hwnd, &self.t);
                let _ = self.layout();
            }
            WM_GETMINMAXINFO => unsafe {
                let (x, y) = crate::host::window_size(self.hwnd, 640., 480.);
                (*(lp.0 as *mut MINMAXINFO)).ptMinTrackSize = POINT { x, y };
            },
            WM_MOUSEMOVE | WM_LBUTTONDOWN | WM_LBUTTONDBLCLK | WM_LBUTTONUP => {
                let (x, y) = (
                    (lp.0 as u16 as i16) as f32 / self.scale,
                    ((lp.0 >> 16) as u16 as i16) as f32 / self.scale,
                );
                if self.output_pointer(msg, x, y)
                    || self.crop_pointer(msg, x, y)
                    || self.timeline_pointer(msg, x, y)
                {
                    return Some(LRESULT(0));
                }
                let hit = self.hit(x, y);
                let over_media = y < self.rects[FOOTER].y
                    && self.preview_view().contains(x, y)
                    && self.preview_media().contains(x, y);
                if over_media != self.preview.hovered {
                    self.preview.hovered = over_media;
                    self.dirty[slot(Target::Play)] = true;
                }
                if self.hover != hit {
                    self.dirty[TIMELINE] = true;
                    if let Some(t) = self.hover {
                        self.dirty[slot(t)] = true;
                    }
                    if let Some(t) = hit {
                        self.dirty[slot(t)] = true;
                    }
                    self.hover = hit;
                }
                if msg == WM_MOUSEMOVE {
                    if !self.tracking {
                        unsafe {
                            let _ = TrackMouseEvent(&mut TRACKMOUSEEVENT {
                                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                                dwFlags: TME_LEAVE,
                                hwndTrack: self.hwnd,
                                ..Default::default()
                            });
                        }
                        self.tracking = true;
                    }
                    if self.dragging {
                        let at = self
                            .filename
                            .hit(x - self.target_rect(Target::Filename).x - self.t.n("s-4"));
                        let c = self.filename.editor.move_to(at, true);
                        self.edited(c);
                    }
                } else if msg == WM_LBUTTONDOWN || msg == WM_LBUTTONDBLCLK {
                    unsafe {
                        let _ = SetFocus(Some(self.hwnd));
                        SetCapture(self.hwnd);
                    }
                    self.pressed = hit.filter(|&t| self.enabled(t));
                    let was = self.focus;
                    self.set_focus(self.pressed, false);
                    if self.pressed == Some(Target::Filename) {
                        let at = self
                            .filename
                            .hit(x - self.target_rect(Target::Filename).x - self.t.n("s-4"));
                        if msg == WM_LBUTTONDBLCLK {
                            let (a, b) = self.filename.editor.word_at(at);
                            let c = self.filename.editor.select(a, b);
                            self.edited(c);
                        } else if was == Some(Target::Filename) {
                            let c = self.filename.editor.move_to(at, false);
                            self.edited(c);
                            self.dragging = true;
                        }
                    }
                } else {
                    let pressed = self.pressed.take();
                    self.dragging = false;
                    unsafe {
                        let _ = ReleaseCapture();
                    }
                    if pressed == hit
                        && let Some(t) = pressed
                        && t != Target::Filename
                    {
                        self.activate(t);
                    }
                }
            }
            WM_CAPTURECHANGED => {
                self.output.slider = None;
                self.end_crop_drag();
                self.end_timeline_drag();
                self.dragging = false;
                self.pressed = None;
            }
            WM_MOUSELEAVE => {
                self.tracking = false;
                self.dirty[TIMELINE] = true;
                self.preview.hovered = false;
                self.dirty[slot(Target::Play)] = true;
                if let Some(t) = self.hover.take() {
                    self.dirty[slot(t)] = true;
                }
            }
            WM_SETCURSOR if (lp.0 & 0xffff) as u32 == HTCLIENT => unsafe {
                let mut point = POINT::default();
                let _ = GetCursorPos(&mut point);
                let _ = windows::Win32::Graphics::Gdi::ScreenToClient(self.hwnd, &mut point);
                let (x, y) = (point.x as f32 / self.scale, point.y as f32 / self.scale);
                let over_timeline =
                    y < self.rects[FOOTER].y && self.timeline_track().contains(x, y);
                let cursor = if let Some(handle) = self.crop_hit(x, y) {
                    match handle {
                        CropDragHandle::North | CropDragHandle::South => IDC_SIZENS,
                        CropDragHandle::East | CropDragHandle::West => IDC_SIZEWE,
                        CropDragHandle::NorthEast | CropDragHandle::SouthWest => IDC_SIZENESW,
                        CropDragHandle::NorthWest | CropDragHandle::SouthEast => IDC_SIZENWSE,
                        CropDragHandle::Move => IDC_SIZEALL,
                    }
                } else if self.timeline.drag.is_some()
                    || self.timeline.scrubbing
                    || self.enabled(Target::TrimStart)
                        && (over_timeline
                            || matches!(self.hit(x, y), Some(Target::TrimStart | Target::TrimEnd)))
                {
                    IDC_SIZEWE
                } else {
                    IDC_ARROW
                };
                SetCursor(LoadCursorW(None, cursor).ok());
                return Some(LRESULT(1));
            },
            WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
                let mut point = POINT {
                    x: lp.0 as u16 as i16 as i32,
                    y: (lp.0 >> 16) as u16 as i16 as i32,
                };
                unsafe {
                    let _ = windows::Win32::Graphics::Gdi::ScreenToClient(self.hwnd, &mut point);
                }
                let delta =
                    (wp.0 >> 16) as u16 as i16 as f32 / 120. * appearance::wheel_lines() * 100.
                        / 3.;
                if self.output_menu_wheel(delta) {
                    return Some(LRESULT(0));
                }
                let old_scroll = self.preview.scroll;
                let in_page = point.y as f32 / self.scale < self.rects[FOOTER].y;
                if in_page
                    && self
                        .preview_view()
                        .contains(point.x as f32 / self.scale, point.y as f32 / self.scale)
                {
                    let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
                    if msg == WM_MOUSEHWHEEL {
                        self.preview.scroll.0 += delta;
                    } else if shift {
                        self.preview.scroll.0 -= delta;
                    } else {
                        self.preview.scroll.1 -= delta;
                    }
                    self.clamp_preview_scroll();
                    self.place_play();
                    self.dirty[PREVIEW] = true;
                }
                if in_page
                    && msg == WM_MOUSEWHEEL
                    && old_scroll == self.preview.scroll
                    && unsafe { GetKeyState(VK_SHIFT.0 as i32) } >= 0
                {
                    self.scroll_page(-delta);
                }
            }
            WM_KEYDOWN => self.key(VIRTUAL_KEY(wp.0 as u16)),
            WM_CHAR if self.output_char(wp.0 as u16) => {}
            WM_CHAR if self.focus == Some(Target::Filename) && self.enabled(Target::Filename) => {
                let unit = wp.0 as u16;
                if (0xd800..=0xdbff).contains(&unit) {
                    self.high_surrogate = Some(unit);
                } else if unit >= 32 {
                    let mut units = Vec::new();
                    if let Some(high) = self.high_surrogate.take() {
                        units.push(high);
                    }
                    units.push(unit);
                    let c = self
                        .filename
                        .editor
                        .insert(&String::from_utf16_lossy(&units));
                    self.edited(c);
                }
            }
            WM_IME_COMPOSITION if self.focus == Some(Target::Filename) => {
                let c =
                    textfield::ime_composition(self.hwnd, lp.0 as u32, &mut self.filename.editor);
                self.edited(c);
            }
            WM_IME_ENDCOMPOSITION => {
                let c = self.filename.editor.set_composition(&[], 0);
                self.edited(c);
            }
            WM_KILLFOCUS => {
                self.set_focus(None, false);
            }
            _ => return None,
        }
        Some(LRESULT(0))
    }
}

fn slot(target: Target) -> usize {
    CONTROL_START + TARGETS.iter().position(|&t| t == target).expect("target")
}

pub fn register(instance: HINSTANCE) -> Result<()> {
    unsafe {
        RegisterClassW(&WNDCLASSW {
            style: CS_DBLCLKS,
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: w!("CapturesRecordingEditor"),
            ..Default::default()
        });
    }
    Ok(())
}

pub fn open(options: &Options, id: &str) -> Result<()> {
    let existing = WINDOWS.with(|w| w.borrow().iter().find(|w| w.id == id).map(|w| w.hwnd));
    if let Some(hwnd) = existing {
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
            let _ = SetForegroundWindow(hwnd);
        }
        return Ok(());
    }
    let settings = captures_settings::load(&options.settings_file)
        .map_err(|error| Error::new(E_FAIL, format!("Couldn’t read settings: {error}")))?;
    let tokens = appearance::tokens(options.appearance.as_deref(), &settings);
    let g = Gfx::new(&tokens, true)?;
    unsafe {
        let instance = windows::Win32::System::LibraryLoader::GetModuleHandleW(None)?;
        let hwnd = CreateWindowExW(
            WS_EX_NOREDIRECTIONBITMAP,
            w!("CapturesRecordingEditor"),
            &HSTRING::from(captures_app::app_windows::RECORDING_EDITOR_TITLE),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            SIZE.0 as i32,
            SIZE.1 as i32,
            None,
            None,
            Some(instance.into()),
            None,
        )?;
        let mut editor = match Recording::new(hwnd, options, id, g, tokens) {
            Ok(editor) => editor,
            Err(error) => {
                let _ = DestroyWindow(hwnd);
                return Err(error);
            }
        };
        let (w, h) = options
            .size
            .filter(|_| options.scene == crate::Scene::Recording)
            .map_or(SIZE, |(w, h)| (w as f32, h as f32));
        editor.width = w;
        editor.height = h;
        if let Err(error) = editor.layout() {
            drop(editor);
            let _ = DestroyWindow(hwnd);
            return Err(error);
        }
        appearance::title_bar(hwnd, &editor.t);
        textfield::ime_enable(hwnd, false);
        WINDOWS.with(|windows| windows.borrow_mut().push(editor));
        let (w, h) = crate::host::window_size(hwnd, w, h);
        let _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            w,
            h,
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
        let _ = ShowWindow(hwnd, SW_SHOW);
    }
    Ok(())
}

pub fn render_offscreen(
    options: &Options,
    g: Gfx,
    tokens: Tokens,
    size: (f32, f32),
) -> Result<Recording> {
    let id = options
        .artifact
        .as_deref()
        .ok_or_else(|| Error::new(E_FAIL, "--scene recording needs --artifact ID"))?;
    let mut window = Recording::new(HWND::default(), options, id, g, tokens)?;
    window.scale = 1.;
    (window.width, window.height) = size;
    // Opening probes the actual fixture through the same session worker.
    // Offscreen rendering has no message loop and never exports or saves.
    let opened = window
        .worker
        .outcomes
        .recv()
        .map_err(|error| Error::new(E_FAIL, error.to_string()))?;
    window.apply_outcome(opened);
    if window.presented.is_some() {
        window.load_timeline();
        let thumbnails = window
            .worker
            .outcomes
            .recv()
            .map_err(|error| Error::new(E_FAIL, error.to_string()))?;
        window.apply_outcome(thumbnails);
    }
    window.layout()?;
    Ok(window)
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        // Commit composition before actions move focus or save. Completion
        // sends WM_IME_COMPOSITION synchronously, so release the window first.
        if (matches!(msg, WM_LBUTTONDOWN | WM_CLOSE | WM_KILLFOCUS)
            || msg == WM_KEYDOWN && wp.0 == VK_TAB.0 as usize)
            && with_recording(hwnd, |w| w.filename.editor.composing()) == Some(true)
        {
            textfield::ime_complete(hwnd);
        }
        match msg {
            WM_GETOBJECT if lp.0 as i32 == UiaRootObjectId => {
                return UiaReturnRawElementProvider(
                    hwnd,
                    wp,
                    lp,
                    &crate::recording_uia::node(hwnd, crate::recording_uia::Id::Root),
                );
            }
            WM_CLOSE => {
                with_recording(hwnd, |w| w.request_close());
                return LRESULT(0);
            }
            WM_DESTROY => {
                let _ = UiaReturnRawElementProvider(hwnd, WPARAM(0), LPARAM(0), None);
                WINDOWS.with(|windows| {
                    if let Ok(mut windows) = windows.try_borrow_mut()
                        && let Some(i) = windows.iter().position(|w| w.hwnd == hwnd)
                    {
                        windows.remove(i);
                    }
                });
                if !exists()
                    && !crate::host::exists()
                    && !crate::history::exists()
                    && !crate::editor::exists()
                {
                    PostQuitMessage(0);
                }
                return LRESULT(0);
            }
            WM_PAINT => {
                let _ = ValidateRect(Some(hwnd), None);
                return LRESULT(0);
            }
            WM_ERASEBKGND => return LRESULT(1),
            WM_IME_SETCONTEXT => {
                return DefWindowProcW(
                    hwnd,
                    msg,
                    wp,
                    LPARAM(
                        lp.0 & !(windows::Win32::UI::Input::Ime::ISC_SHOWUICOMPOSITIONWINDOW
                            as isize),
                    ),
                );
            }
            WM_DPICHANGED => {
                with_recording(hwnd, |w| {
                    w.scale = (wp.0 & 0xffff) as f32 / 96.;
                    let _ = w.layout();
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
        let handled = with_recording(hwnd, |w| w.handle(msg, wp, lp)).flatten();
        if with_recording(hwnd, |w| std::mem::take(&mut w.close_ready)) == Some(true) {
            let _ = DestroyWindow(hwnd);
            return LRESULT(0);
        }
        // UIA activation also sets this flag and posts WM_WAKE. Take it before
        // entering the dialog's nested loop, with the window borrow released.
        let folder = with_recording(hwnd, |w| {
            std::mem::take(&mut w.pick_folder).then(|| w.directory.to_string_lossy().into_owned())
        })
        .flatten();
        if let Some(folder) = folder {
            let result = crate::dialog::pick_folder(hwnd, "Choose save location", &folder);
            with_recording(hwnd, |w| match result {
                Ok(Some(path)) => {
                    w.directory = path.into();
                    w.notify("", false);
                    w.refresh_footer();
                }
                Err(error) => w.notify(&error.to_string(), true),
                _ => {}
            });
        }
        handled.unwrap_or_else(|| DefWindowProcW(hwnd, msg, wp, lp))
    }
}
