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
//! (`.recording-countdown`) on the selected display; Esc cancels it. Record
//! shows shipping's `recording-options-row` (FPS, Max resolution, Show
//! cursor, Show clicks, Desktop audio and Microphone, kept for this capture
//! only) and hands the flow, display, target and those choices to
//! `recorder`. A drawn region moves by its frame and resizes by its corner
//! handles under a W × H label; Shift squares it and the Aspect select locks
//! its ratio (the shared `selection` geometry). Full screen's Display select
//! prepares another display and reopens the bar there. The guidance chip,
//! options-row arrive, ready ping and list pop-in run on a frame timer only
//! while they move. Screenshot Region and Window open the same overlay as
//! shipping's bar-less selector (`Menu::direct`). A disabled Window target
//! and unavailable Record switches show their reason in the shared glass
//! tooltip. Tab walks the bar with shipping's focus rings, and the bar
//! exposes UI Automation through `panel_uia`.

use std::cell::RefCell;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::{Duration, Instant};

use captures_app::capture_error;
use captures_app::capture_flow::CaptureFlow;
use captures_app::capture_menu::{
    self, GuidanceChip, GuidanceTarget, MenuMode, PreferenceTarget, PrimaryState, RecordingToggle,
};
use captures_app::controls::select;
use captures_app::motion::{Motion, Pose};
use captures_app::selection::{self, Bounds, DragMode, DragOptions, Point};
use captures_app::window::{self as window_target, WindowSession};
use captures_capture::{DisplayDescriptor, LogicalRect};
use captures_recording::{CaptureRect, MaxResolution, RecordingTarget};
use captures_recording_platform::RecordingCapabilities;
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
use windows_numerics::Matrix3x2;

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
    /// A `.recording-options-row` switch.
    Toggle(RecordingToggle),
    /// A `.recording-options-row` select's trigger.
    Select(Field),
    /// A row in the open listbox.
    Option(usize),
    /// A `.recording-window-target` button, activated by keyboard or UI
    /// Automation (pointer clicks hit-test the surface instead).
    Pick(usize),
    /// A `.capture-selector-preferences-link` in the note.
    Preferences(PreferenceTarget),
}

/// The bar's selects, in their UI Automation key order.
const FIELDS: [Field; 5] = [
    Field::Fps,
    Field::Resolution,
    Field::Microphone,
    Field::Aspect,
    Field::Display,
];

/// UI Automation keys for the parts that are not controls.
const UIA_TYPE_GROUP: u32 = 50;
const UIA_TARGET_GROUP: u32 = 51;
const UIA_LISTBOX: u32 = 60;
const UIA_NOTE: u32 = 70;
const UIA_GUIDANCE: u32 = 71;

/// A control's UI Automation key (stable while the bar is open).
fn uia_key(hit: Hit) -> u32 {
    match hit {
        Hit::Close => 1,
        Hit::Screenshot => 2,
        Hit::Record => 3,
        Hit::Target(Target::Region) => 4,
        Hit::Target(Target::Window) => 5,
        Hit::Target(Target::Display) => 6,
        Hit::Primary => 7,
        Hit::Preferences(PreferenceTarget::IncludeRecordingControlsInCaptures) => 8,
        Hit::Preferences(PreferenceTarget::AutoStartOnSelection) => 9,
        Hit::Toggle(toggle) => {
            10 + RecordingToggle::ALL
                .iter()
                .position(|t| *t == toggle)
                .unwrap_or(0) as u32
        }
        Hit::Select(field) => 20 + FIELDS.iter().position(|f| *f == field).unwrap_or(0) as u32,
        Hit::Option(index) => 100 + index as u32,
        Hit::Pick(index) => 1000 + index as u32,
    }
}

fn uia_hit(key: u32) -> Option<Hit> {
    Some(match key {
        1 => Hit::Close,
        2 => Hit::Screenshot,
        3 => Hit::Record,
        4 => Hit::Target(Target::Region),
        5 => Hit::Target(Target::Window),
        6 => Hit::Target(Target::Display),
        7 => Hit::Primary,
        8 => Hit::Preferences(PreferenceTarget::IncludeRecordingControlsInCaptures),
        9 => Hit::Preferences(PreferenceTarget::AutoStartOnSelection),
        10..=12 => Hit::Toggle(RecordingToggle::ALL[(key - 10) as usize]),
        20..=24 => Hit::Select(FIELDS[(key - 20) as usize]),
        100..=999 => Hit::Option((key - 100) as usize),
        1000.. => Hit::Pick((key - 1000) as usize),
        _ => return None,
    })
}

/// A select trigger's `aria-label`.
fn field_label(field: Field) -> &'static str {
    match field {
        Field::Fps => "Frames per second",
        Field::Resolution => "Maximum resolution",
        Field::Microphone => "Microphone",
        Field::Aspect => "Region aspect ratio",
        Field::Display => "Display",
    }
}

/// The bar's selects: the Record options row's, and Region's Aspect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Fps,
    Resolution,
    Microphone,
    /// `.recording-region-aspect-picker`.
    Aspect,
    /// `.recording-display-picker`.
    Display,
}

/// The Display select's width (`.recording-display-picker`).
const DISPLAY_W: f32 = 190.;

/// Shipping's Display select options as `(id, label, description)`: the OS
/// name or `Display N`, then `W × H` and ` · Primary`.
fn display_options(displays: &[DisplayDescriptor]) -> Vec<(String, String, String)> {
    displays
        .iter()
        .enumerate()
        .map(|(index, d)| {
            let name = d.name.trim();
            let label = if name.is_empty() {
                format!("Display {}", index + 1)
            } else {
                name.to_owned()
            };
            let primary = if d.is_primary { " · Primary" } else { "" };
            let detail = format!("{} × {}{primary}", d.width, d.height);
            (d.id.clone(), label, detail)
        })
        .collect()
}

/// Shipping `REGION_ASPECT_PRESETS` (`lib/selection.ts`).
const ASPECT_PRESETS: [(&str, Option<f64>); 6] = [
    ("Free", None),
    ("1 : 1", Some(1.)),
    ("4 : 3", Some(4. / 3.)),
    ("3 : 2", Some(3. / 2.)),
    ("16 : 9", Some(16. / 9.)),
    ("9 : 16", Some(9. / 16.)),
];
/// `dragSelectionRect`'s default smallest resize.
const MINIMUM_SIZE: f64 = 16.;
/// Grab distance around a 10 px `.handle`'s center.
const HANDLE_REACH: f32 = 8.;
/// The mouse messages' Shift flag (`MK_SHIFT`).
const MK_SHIFT: usize = 0x0004;

/// A region drag in progress: shipping `RecordingRegionDrag`.
#[derive(Clone, Copy, Debug)]
struct RegionDrag {
    mode: DragMode,
    origin: (f32, f32),
    initial: D2D_RECT_F,
}

/// `capture_menu::RESOLUTION_OPTIONS` order.
const RESOLUTIONS: [MaxResolution; 3] = [
    MaxResolution::Original,
    MaxResolution::P1080,
    MaxResolution::P720,
];

/// Shipping `.recording-options-row` columns: `76px 132px 92px 92px 118px
/// minmax(180px, 240px)` with `--s-4` gaps inside `--s-5` padding.
const OPTION_COLUMNS: [f32; 6] = [76., 132., 92., 92., 118., 240.];
/// `.recording-field > span`: a `--text-2xs` caption's line box.
const CAPTION_H: f32 = 11.;

/// The Record options for this capture. Like shipping they start from the
/// saved settings, gated by the platform's capabilities, and only apply to
/// the take they start; Preferences keeps the saved defaults.
pub struct RecordOptions {
    pub fps: u16,
    pub resolution: MaxResolution,
    pub show_cursor: bool,
    pub highlight_clicks: bool,
    pub system_audio: bool,
    pub microphone: Option<String>,
    pub capabilities: RecordingCapabilities,
    /// Enumerated microphones as `(id, name)`; empty until loaded.
    pub microphones: Vec<(String, String)>,
    pub microphones_loading: bool,
    microphones_requested: bool,
}

impl RecordOptions {
    pub fn new(settings: &captures_settings::AppSettings) -> Self {
        let capabilities =
            RecordingCapabilities::current(settings.include_recording_controls_in_captures);
        let r = &settings.recording;
        Self {
            fps: r.video_fps,
            resolution: r.video_max_resolution,
            show_cursor: capabilities.cursor_control && r.show_cursor,
            highlight_clicks: capabilities.click_highlights && r.highlight_clicks,
            system_audio: capabilities.system_audio && r.capture_system_audio,
            microphone: capabilities
                .microphone
                .then(|| r.microphone_device_id.clone())
                .flatten(),
            capabilities,
            microphones: Vec::new(),
            microphones_loading: false,
            microphones_requested: false,
        }
    }

    /// Whether to enumerate microphones now: once per menu, when it first
    /// shows Record (shipping `loadAudioDevices`).
    fn take_microphone_request(&mut self) -> bool {
        let request = self.capabilities.microphone && !self.microphones_requested;
        if request {
            self.microphones_requested = true;
            self.microphones_loading = true;
        }
        request
    }

    pub fn set_microphones(&mut self, devices: Vec<(String, String)>) {
        self.microphones = devices;
        self.microphones_loading = false;
    }

    fn microphone_entries(&self) -> Vec<capture_menu::MicrophoneEntry> {
        let devices: Vec<_> = self
            .microphones
            .iter()
            .map(|(id, name)| (id.as_str(), name.as_str()))
            .collect();
        capture_menu::microphone_entries(
            self.capabilities.microphone,
            self.microphones_loading,
            self.microphone.as_deref(),
            &devices,
        )
    }

    /// A select's option labels, which are disabled, and the selected index
    /// (the first option when the value is not listed, like shipping).
    fn entries(&self, field: Field) -> (Vec<String>, Vec<bool>, usize) {
        match field {
            Field::Fps => {
                let selected = capture_menu::FPS_OPTIONS
                    .iter()
                    .position(|&f| f == self.fps);
                (
                    capture_menu::FPS_OPTIONS.map(|f| f.to_string()).to_vec(),
                    vec![false; 3],
                    selected.unwrap_or(0),
                )
            }
            Field::Resolution => {
                let selected = RESOLUTIONS.iter().position(|&r| r == self.resolution);
                (
                    capture_menu::RESOLUTION_OPTIONS
                        .map(|(_, label)| label.to_string())
                        .to_vec(),
                    vec![false; 3],
                    selected.unwrap_or(0),
                )
            }
            Field::Microphone => {
                let entries = self.microphone_entries();
                let selected = entries
                    .iter()
                    .position(|e| e.id == self.microphone)
                    .unwrap_or(0);
                (
                    entries.iter().map(|e| e.label.clone()).collect(),
                    entries.iter().map(|e| !e.enabled).collect(),
                    selected,
                )
            }
            // The menu's own selects (`Menu::entries`).
            Field::Aspect | Field::Display => (Vec::new(), Vec::new(), 0),
        }
    }

    /// The closed trigger's label.
    fn value(&self, field: Field) -> String {
        match field {
            Field::Microphone => {
                let devices: Vec<_> = self
                    .microphones
                    .iter()
                    .map(|(id, name)| (id.as_str(), name.as_str()))
                    .collect();
                capture_menu::microphone_selected_label(
                    self.capabilities.microphone,
                    self.microphones_loading,
                    self.microphone.as_deref(),
                    &devices,
                )
            }
            _ => {
                let (labels, _, selected) = self.entries(field);
                labels.into_iter().nth(selected).unwrap_or_default()
            }
        }
    }

    /// Shipping disables the microphone select while devices load.
    fn enabled(&self, field: Field) -> bool {
        field != Field::Microphone || (self.capabilities.microphone && !self.microphones_loading)
    }

    fn choose(&mut self, field: Field, index: usize) {
        match field {
            Field::Fps => {
                if let Some(&fps) = capture_menu::FPS_OPTIONS.get(index) {
                    self.fps = fps;
                }
            }
            Field::Resolution => {
                if let Some(&resolution) = RESOLUTIONS.get(index) {
                    self.resolution = resolution;
                }
            }
            Field::Microphone => {
                if let Some(entry) = self.microphone_entries().into_iter().nth(index)
                    && entry.enabled
                {
                    self.microphone = entry.id;
                }
            }
            Field::Aspect | Field::Display => {}
        }
    }

    fn available(&self, toggle: RecordingToggle) -> bool {
        let c = &self.capabilities;
        match toggle {
            RecordingToggle::ShowCursor => c.cursor_control,
            RecordingToggle::ShowClicks => c.click_highlights,
            RecordingToggle::DesktopAudio => c.system_audio,
        }
    }

    fn on(&self, toggle: RecordingToggle) -> bool {
        match toggle {
            RecordingToggle::ShowCursor => self.show_cursor,
            RecordingToggle::ShowClicks => self.highlight_clicks,
            RecordingToggle::DesktopAudio => self.system_audio,
        }
    }

    fn toggle(&mut self, toggle: RecordingToggle) {
        if !self.available(toggle) {
            return;
        }
        match toggle {
            RecordingToggle::ShowCursor => self.show_cursor = !self.show_cursor,
            RecordingToggle::ShowClicks => self.highlight_clicks = !self.highlight_clicks,
            RecordingToggle::DesktopAudio => self.system_audio = !self.system_audio,
        }
        (self.show_cursor, self.highlight_clicks) =
            capture_menu::couple_pointer_options(toggle, self.show_cursor, self.highlight_clicks);
    }

    /// The take's settings: shipping's `start_recording` options.
    pub fn apply(&self, recording: &mut captures_settings::RecordingSettings) {
        let c = &self.capabilities;
        recording.video_fps = self.fps;
        recording.video_max_resolution = self.resolution;
        recording.show_cursor = c.cursor_control && self.show_cursor;
        recording.highlight_clicks = c.click_highlights && self.highlight_clicks;
        recording.capture_system_audio = c.system_audio && self.system_audio;
        recording.microphone_device_id = if c.microphone {
            self.microphone.clone()
        } else {
            None
        };
    }
}

type Rect = D2D_RECT_F;

fn contains(r: &Rect, x: f32, y: f32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

/// A D2D rect as the shared selection geometry's rect.
fn logical(r: Rect) -> selection::Rect {
    selection::Rect {
        x: r.left.into(),
        y: r.top.into(),
        width: (r.right - r.left).into(),
        height: (r.bottom - r.top).into(),
    }
}

fn d2d(r: selection::Rect) -> Rect {
    rect(
        r.x as f32,
        r.y as f32,
        (r.x + r.width) as f32,
        (r.y + r.height) as f32,
    )
}

fn alpha(mut color: D2D1_COLOR_F, a: f32) -> D2D1_COLOR_F {
    color.a *= a;
    color
}

/// Shade everything in `size` but `r` (a closed evenodd cutout in shipping).
fn shade_around(
    dc: &ID2D1DeviceContext,
    (w, h): (f32, f32),
    r: Rect,
    color: D2D1_COLOR_F,
) -> Result<()> {
    let r = rect(
        r.left.max(0.),
        r.top.max(0.),
        r.right.min(w),
        r.bottom.min(h),
    );
    for band in [
        rect(0., 0., w, r.top),
        rect(0., r.bottom, w, h),
        rect(0., r.top, r.left, r.bottom),
        rect(r.right, r.top, w, r.bottom),
    ] {
        if band.right > band.left && band.bottom > band.top {
            gfx::fill(dc, band, color)?;
        }
    }
    Ok(())
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

/// One `.recording-field`: its uppercase caption over a select or switch.
struct FieldBox {
    caption: gfx::Text,
    x: f32,
    caption_y: f32,
    /// The trigger, or the switch's whole `.recording-toggle` label.
    control: Rect,
    /// The trigger's or switch's text.
    value: gfx::Text,
    hit: Hit,
    enabled: bool,
}

/// The open `.custom-select-listbox`: rows with their label and whether each
/// is disabled or selected, plus the selected row's check.
struct Listbox {
    rect: Rect,
    rows: Vec<(Rect, gfx::Text, bool, bool)>,
    /// Each row's `.custom-select-option-copy small` line (the Display
    /// select's size), if any.
    details: Vec<Option<gfx::Text>>,
    check: gfx::Text,
}

struct Layout {
    panel: Rect,
    /// The options row's top border and its fields, in Record.
    options: Option<(f32, Vec<FieldBox>)>,
    listbox: Option<Listbox>,
    close: Rect,
    switches: [Rect; 2],
    divider: Rect,
    segments: Vec<Segment>,
    /// The first row's select (Aspect), its caption to its left.
    pickers: Vec<FieldBox>,
    primary: Option<(Rect, gfx::Text, bool)>,
    /// The note's runs: text, x, bold, and the link it belongs to.
    note: Vec<(gfx::Text, f32, bool, Option<Hit>)>,
    note_y: f32,
    /// `.capture-selector-preferences-link` buttons: their box, where the
    /// external icon sits, and their action.
    links: Vec<(Rect, f32, Hit)>,
    /// `.capture-guidance` at rest, 16% down the display.
    guidance: Option<GuidanceBox>,
}

impl Layout {
    /// Every select and switch on the bar.
    fn fields(&self) -> impl Iterator<Item = &FieldBox> {
        self.pickers
            .iter()
            .chain(self.options.iter().flat_map(|(_, fields)| fields))
    }
}

/// The menu's state and drawing, shared by the live window and the
/// offscreen `--screenshot` renderer. Coordinates are DIPs from the
/// display's top-left corner.
pub struct Menu {
    pub target: Target,
    pub mode: MenuMode,
    /// Recording controls stay out of recordings (Preferences).
    pub controls_excluded: bool,
    pub size: (f32, f32),
    pub selection: Option<Rect>,
    drag: Option<RegionDrag>,
    /// Shift held: a square region (shipping `forceSquare`).
    shift: bool,
    /// The last pointer position, for re-running a drag when Shift or the
    /// aspect changes mid-gesture.
    pointer: (f32, f32),
    /// The Aspect select's `ASPECT_PRESETS` index.
    pub aspect: usize,
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
    pub record: RecordOptions,
    /// A recording is running, so Record stays disabled (the recording
    /// controls' Screenshot opens the bar on its take).
    pub recording_locked: bool,
    /// The open listbox and its highlighted row.
    pub open: Option<(Field, select::State)>,
    /// The Display select's options as `(id, label, description)` and the
    /// shown display's index.
    pub displays: Vec<(String, String, String)>,
    pub display_index: usize,
    /// Another display is being prepared (shipping `switchingDisplay`).
    pub switching: bool,
    /// A display chosen in the select, for `Capture` to switch to.
    switch_to: Option<usize>,
    /// A repeated shortcut is freezing the bar into a new snapshot
    /// (shipping's `starting` while it recaptures).
    pub starting: bool,
    /// The motion clock (`tooltip::now_ms`) and Windows' reduced motion, set
    /// before each paint. Offscreen renders keep reduced motion, so every
    /// motion draws at rest.
    pub clock: f64,
    pub reduced: bool,
    /// `CaptureGuidance`: the Region chip (true) or the Window one, and its
    /// shared motion.
    guidance: Option<(bool, GuidanceChip)>,
    /// The pointer is over the chip, so it fades out of the way.
    guidance_over: bool,
    /// When Record's options row (and the primary's ready ping) mounted.
    record_since: Option<f64>,
    /// The open listbox's field and when it popped in.
    open_since: Option<(Field, f64)>,
    /// Shipping's bar-less Screenshot Region and Window selectors
    /// (`CaptureOverlay`): no bar, a region commits on release and a window
    /// on click.
    pub direct: bool,
    /// `showSelectionFeedback`: the empty click's attempt (it re-keys the
    /// chip), when its feedback ends, and a click waiting for the clock.
    feedback: (u32, Option<f64>),
    feedback_pending: bool,
    /// The glass tooltip under the pointer and its fade (shared `tooltip`).
    tooltip: crate::tooltip::Fade<Hit>,
    tooltip_target: Option<Hit>,
    /// The focused control (shipping's DOM focus), and whether the keyboard
    /// moved it there so its `:focus-visible` ring draws.
    focus: Option<Hit>,
    focus_visible: bool,
    /// The bar's top-left once dragged by its blank glass (shipping
    /// `panelPosition`), and the pointer's offset into it while dragging.
    panel_at: Option<(f32, f32)>,
    panel_drag: Option<(f32, f32)>,
}

/// The chip's frame with its title and hint lines.
struct GuidanceBox {
    rect: Rect,
    title: gfx::Text,
    hint: gfx::Text,
}

/// What a capture shortcut did to the open bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ShortcutEffect {
    Ignored,
    /// Switched mode and target in place.
    Switched,
    /// Freeze the screen with the capture UI in it and open, on that
    /// snapshot, the bar (or a direct selector) on this target and mode.
    Recapture {
        target: Target,
        mode: MenuMode,
        direct: bool,
    },
}

fn menu_target(target: capture_error::Target) -> Target {
    match target {
        capture_error::Target::Region => Target::Region,
        capture_error::Target::Window => Target::Window,
        capture_error::Target::Display => Target::Display,
    }
}

fn route_target(target: Target) -> capture_error::Target {
    match target {
        Target::Region => capture_error::Target::Region,
        Target::Window => capture_error::Target::Window,
        Target::Display => capture_error::Target::Display,
    }
}

impl Menu {
    pub fn new(
        target: Target,
        size: (f32, f32),
        display: (String, u32, u32),
        auto_start: bool,
    ) -> Self {
        let only = (
            String::new(),
            display.0.clone(),
            format!("{} × {}", display.1, display.2),
        );
        Self {
            target,
            mode: MenuMode::Screenshot,
            controls_excluded: true,
            size,
            selection: None,
            drag: None,
            shift: false,
            pointer: (0., 0.),
            aspect: 0,
            hover: None,
            pressed: None,
            display,
            auto_start,
            layout: None,
            windows: Vec::new(),
            hovered: None,
            picked: None,
            record: RecordOptions::new(&captures_settings::AppSettings::default()),
            recording_locked: false,
            open: None,
            displays: vec![only],
            display_index: 0,
            switching: false,
            switch_to: None,
            starting: false,
            clock: 0.,
            reduced: true,
            guidance: None,
            guidance_over: false,
            record_since: None,
            open_since: None,
            direct: false,
            feedback: (0, None),
            feedback_pending: false,
            tooltip: crate::tooltip::Fade::default(),
            tooltip_target: None,
            focus: None,
            focus_visible: false,
            panel_at: None,
            panel_drag: None,
        }
    }

    /// Shipping's Tab order through the bar: the window targets, Close, the
    /// enabled segments, the first row's select, the primary and the Record
    /// options. Disabled controls are skipped, like disabled buttons.
    fn focus_order(&self) -> Vec<Hit> {
        let Some(layout) = self.layout.as_ref().filter(|_| !self.direct) else {
            return Vec::new();
        };
        let mut order = Vec::new();
        if self.target == Target::Window {
            order.extend((0..self.windows.len()).map(Hit::Pick));
        }
        order.push(Hit::Close);
        order.extend(layout.segments.iter().filter(|s| s.enabled).map(|s| s.hit));
        order.extend(layout.pickers.iter().filter(|f| f.enabled).map(|f| f.hit));
        if let Some((_, _, true)) = layout.primary {
            order.push(Hit::Primary);
        }
        if let Some((_, fields)) = &layout.options {
            order.extend(fields.iter().filter(|f| f.enabled).map(|f| f.hit));
        }
        order.extend(layout.links.iter().map(|(.., hit)| *hit));
        order
    }

    /// Focus `hit`; a focused window target lights like a hovered one
    /// (`.recording-window-target:focus-visible`).
    fn set_focus(&mut self, hit: Hit, visible: bool) {
        self.focus = Some(hit);
        self.focus_visible = visible;
        if let Hit::Pick(index) = hit {
            self.hovered = Some(Some(index));
        }
    }

    /// Tab or Shift+Tab: the next or previous control, wrapping. An open
    /// listbox closes, like `CustomSelect` on Tab.
    fn focus_step(&mut self, back: bool) -> bool {
        let order = self.focus_order();
        if order.is_empty() {
            return false;
        }
        self.open = None;
        let n = order.len();
        let next = match self.focus.and_then(|f| order.iter().position(|h| *h == f)) {
            Some(i) if back => (i + n - 1) % n,
            Some(i) => (i + 1) % n,
            None if back => n - 1,
            None => 0,
        };
        self.set_focus(order[next], true);
        true
    }

    /// Enter and Space on the focused control (after the select's keys).
    /// Shipping's capture-phase Enter confirms from the primary, the
    /// segments or nothing focused, and leaves Close and the window targets
    /// to native activation; Space activates the focused button or switch.
    fn focus_key(&mut self, enter: bool) -> Option<Hit> {
        if self.focus.is_some() {
            self.focus_visible = true;
        }
        match (self.focus, enter) {
            (Some(hit @ (Hit::Close | Hit::Pick(_) | Hit::Preferences(_))), _) => Some(hit),
            (Some(Hit::Toggle(_)), true) => None,
            (_, true) => Some(Hit::Primary),
            (Some(hit), false) if self.focus_order().contains(&hit) => Some(hit),
            (_, false) => None,
        }
    }

    /// The UI Automation root's name: the bar, or a direct selector's
    /// overlay (the AppKit names, as shipping's `<main>` is unlabelled).
    pub fn uia_name(&self) -> &'static str {
        match (self.direct, self.target) {
            (true, Target::Window) => capture_menu::WINDOW_SELECTOR_LABEL,
            (true, _) => capture_menu::REGION_SELECTOR_LABEL,
            (false, _) => "New Capture",
        }
    }

    /// The bar's parts in shipping's DOM order with its roles and names:
    /// the guidance chip (`role="status"`), the window target buttons, Close,
    /// the `aria-pressed` segments in their labelled groups, the
    /// `CustomSelect` comboboxes and open listbox, the primary, the Record
    /// options' checkboxes, and the note.
    pub fn uia_items(&self) -> Vec<crate::panel_uia::Item> {
        use crate::panel_uia::{Item, Role};
        let mut items = Vec::new();
        let Some(layout) = self.layout.as_ref() else {
            return items;
        };
        // The faded chip is `aria-hidden`.
        if let (Some(guide), Some(target), false) =
            (&layout.guidance, self.guidance_target(), self.guidance_over)
        {
            let copy = capture_menu::guidance(target, self.feedback.0 > 0);
            let mut chip = Item::new(UIA_GUIDANCE, Role::Status, copy.title, guide.rect);
            chip.help = copy.hint.into();
            items.push(chip);
        }
        if self.direct {
            return items;
        }
        // While a listbox is open its active option has focus
        // (`aria-activedescendant`).
        let control = |hit: Hit, role, name: &str, rect, enabled| {
            let mut item = Item::new(uia_key(hit), role, name, rect);
            item.enabled = enabled;
            item.focused = self.open.is_none() && self.focus == Some(hit);
            item
        };
        if self.target == Target::Window {
            for (index, (r, title)) in self.windows.iter().enumerate() {
                let title = if title.is_empty() { "window" } else { title };
                let name = format!("Select {title}");
                items.push(control(Hit::Pick(index), Role::Button, &name, *r, true));
            }
        }
        items.push(control(
            Hit::Close,
            Role::Button,
            "Close capture controls",
            layout.close,
            true,
        ));
        for (key, name, r) in [
            (UIA_TYPE_GROUP, "Capture type", layout.switches[0]),
            (UIA_TARGET_GROUP, "Capture target", layout.switches[1]),
        ] {
            items.push(Item::new(key, Role::Group, name, r));
        }
        for s in &layout.segments {
            let (group, name) = match s.hit {
                Hit::Screenshot => (UIA_TYPE_GROUP, "Screenshot"),
                Hit::Record => (UIA_TYPE_GROUP, "Record"),
                Hit::Target(Target::Region) => (UIA_TARGET_GROUP, "Region"),
                Hit::Target(Target::Window) => (UIA_TARGET_GROUP, "Window"),
                _ => (UIA_TARGET_GROUP, "Full screen"),
            };
            let mut item = control(s.hit, Role::Toggle, name, s.rect, s.enabled);
            item.parent = Some(group);
            item.selected = s.active;
            if let Some((_, help, _)) = self
                .tooltip_at((s.rect.left + s.rect.right) / 2., s.rect.top + 1.)
                .filter(|(h, ..)| *h == s.hit)
            {
                item.help = help.into();
            }
            items.push(item);
        }
        let field = |f: &FieldBox| match f.hit {
            Hit::Select(field) => {
                let mut item = control(
                    f.hit,
                    Role::ComboBox,
                    field_label(field),
                    f.control,
                    f.enabled,
                );
                item.selected = self.open.is_some_and(|(o, _)| o == field);
                item.value = Some(self.value(field));
                item
            }
            Hit::Toggle(toggle) => {
                let name = toggle.accessibility_label();
                let mut item = control(f.hit, Role::Check, name, f.control, f.enabled);
                item.selected = f.enabled && self.record.on(toggle);
                if !f.enabled {
                    item.help = toggle.unavailable_reason().into();
                }
                item
            }
            _ => control(f.hit, Role::Button, "", f.control, f.enabled),
        };
        items.extend(layout.pickers.iter().map(field));
        if let Some((r, _, enabled)) = &layout.primary {
            let state = PrimaryState {
                starting: self.starting,
                switching_display: self.switching,
                ..PrimaryState::default()
            };
            let primary = capture_menu::primary_action(self.mode, self.auto_start, state);
            let name = primary.accessibility_label;
            items.push(control(Hit::Primary, Role::Button, name, *r, *enabled));
        }
        if let Some((_, fields)) = &layout.options {
            items.extend(fields.iter().map(field));
        }
        // The note: its Preferences links are buttons named by their text;
        // the `·` is `aria-hidden`.
        let excluded = self.mode == MenuMode::Screenshot || self.controls_excluded;
        let visibility = capture_menu::visibility_note(self.mode, true, excluded).text();
        let confirm = capture_menu::confirm_note(self.auto_start);
        let link = |hit| {
            layout
                .links
                .iter()
                .find(|(.., h)| *h == hit)
                .map(|(r, ..)| *r)
        };
        for (text, target) in [
            (
                visibility,
                Some(PreferenceTarget::IncludeRecordingControlsInCaptures),
            ),
            (confirm.text.to_string(), confirm.target),
        ] {
            let hit = target.map(Hit::Preferences);
            if let Some((hit, r)) = hit.and_then(|hit| Some((hit, link(hit)?))) {
                items.push(control(hit, Role::Button, &text, r, true));
            } else if let Some((run, x, ..)) = layout.note.last() {
                // "Press Enter to confirm", the note's last run.
                let y = layout.note_y;
                let r = rect(*x, y, x + run.width, y + run.height);
                items.push(Item::new(UIA_NOTE, Role::Text, text, r));
            }
        }
        if let (Some((field, state)), Some(listbox)) = (self.open, &layout.listbox) {
            items.push(Item::new(
                UIA_LISTBOX,
                Role::List,
                field_label(field),
                listbox.rect,
            ));
            let (labels, disabled, selected) = self.entries(field);
            let details = self.details(field);
            for (index, (r, ..)) in listbox.rows.iter().enumerate() {
                let label = labels.get(index).map_or("", String::as_str);
                let name = match details.get(index).cloned().flatten() {
                    Some(detail) => format!("{label} {detail}"),
                    None => label.to_string(),
                };
                let mut option = Item::new(uia_key(Hit::Option(index)), Role::Option, name, *r);
                option.parent = Some(UIA_LISTBOX);
                option.enabled = !disabled.get(index).copied().unwrap_or(true);
                option.selected = index == selected;
                option.focused = index == state.active;
                items.push(option);
            }
        }
        items
    }

    /// Offscreen fixtures: draw `hit` with its keyboard focus ring.
    pub fn show_focus(&mut self, which: &str) {
        let hit = match which {
            "close" => Hit::Close,
            "primary" => Hit::Primary,
            "select" => Hit::Select(Field::Aspect),
            "toggle" => Hit::Toggle(RecordingToggle::ShowCursor),
            "window" => Hit::Pick(0),
            "link" => Hit::Preferences(PreferenceTarget::IncludeRecordingControlsInCaptures),
            _ => Hit::Target(self.target),
        };
        self.set_focus(hit, true);
    }

    /// Shipping disables Window when the listed display has no windows.
    fn window_available(&self) -> bool {
        !self.windows.is_empty()
    }

    /// Offscreen fixtures: no windows listed, so Window is disabled, with
    /// its tooltip at rest.
    pub fn without_windows_tooltip(&mut self) {
        let hit = Some(Hit::Target(Target::Window));
        self.windows.clear();
        self.tooltip = crate::tooltip::Fade::settled(hit);
        self.tooltip_target = hit;
    }

    /// The tooltip under the pointer: shipping's `title` on a disabled
    /// Window target or an unavailable Record switch.
    fn tooltip_at(&self, x: f32, y: f32) -> Option<(Hit, &'static str, Rect)> {
        let layout = self.layout.as_ref().filter(|_| !self.direct)?;
        if layout
            .listbox
            .as_ref()
            .is_some_and(|b| contains(&b.rect, x, y))
        {
            return None;
        }
        let segment = layout.segments.iter().find_map(|s| match s.hit {
            Hit::Target(Target::Window) if !s.enabled && contains(&s.rect, x, y) => Some((
                s.hit,
                "Window capture is not available in this desktop session",
                s.rect,
            )),
            _ => None,
        });
        segment.or_else(|| {
            layout.fields().find_map(|f| match f.hit {
                Hit::Toggle(toggle) if !f.enabled && contains(&f.control, x, y) => {
                    Some((f.hit, toggle.unavailable_reason(), f.control))
                }
                _ => None,
            })
        })
    }

    /// A tooltip key's copy and anchor in the current layout.
    fn tooltip_anchor(&self, hit: Hit) -> Option<(&'static str, Rect)> {
        let layout = self.layout.as_ref()?;
        let rect = match hit {
            Hit::Target(_) => layout.segments.iter().find(|s| s.hit == hit)?.rect,
            _ => layout.fields().find(|f| f.hit == hit)?.control,
        };
        let (cx, cy) = ((rect.left + rect.right) / 2., (rect.top + rect.bottom) / 2.);
        let (_, label, _) = self.tooltip_at(cx, cy).filter(|(h, ..)| *h == hit)?;
        Some((label, rect))
    }

    /// Which guidance chip shows, if any: Region's, or Window's until a
    /// window is picked (over the desktop it offers the display).
    fn guidance_target(&self) -> Option<GuidanceTarget> {
        match self.target {
            Target::Region => Some(GuidanceTarget::Region),
            Target::Window if self.picked.is_none() => Some(if self.hovered == Some(None) {
                GuidanceTarget::Display
            } else {
                GuidanceTarget::Window
            }),
            _ => None,
        }
    }

    /// Follow the bar's state with its motions at `self.clock`: the chip
    /// mounts with its target (Region and Window are separate chips, like
    /// shipping's two elements) and fades out while a region is drawn or the
    /// pointer covers it; the options row and listbox replay their entrance
    /// whenever they mount.
    fn sync_motion(&mut self, t: &Tokens) {
        let now = self.clock;
        self.tooltip.set(self.tooltip_target, t, now);
        match (self.mode, self.record_since) {
            (MenuMode::Recording, None) => self.record_since = Some(now),
            (MenuMode::Screenshot, Some(_)) => self.record_since = None,
            _ => {}
        }
        let open = self.open.map(|(field, _)| field);
        if open != self.open_since.map(|(field, _)| field) {
            self.open_since = open.map(|field| (field, now));
        }
        let creating = self
            .drag
            .is_some_and(|drag| matches!(drag.mode, DragMode::Create));
        // An empty click's feedback lasts `GUIDANCE_FEEDBACK`; each attempt
        // and its end re-key the chip, replaying its entrance.
        let (mut attempt, mut until) = self.feedback;
        if std::mem::take(&mut self.feedback_pending) {
            attempt += 1;
            until = Some(now + capture_menu::GUIDANCE_FEEDBACK.as_secs_f64() * 1000.);
        } else if until.is_some_and(|until| now >= until) {
            (attempt, until) = (0, None);
        }
        let rekeyed = (attempt, until) != self.feedback;
        self.feedback = (attempt, until);
        let Some(target) = self.guidance_target() else {
            self.guidance = None;
            return;
        };
        let region = target == GuidanceTarget::Region;
        let shown = !(self.guidance_over || region && creating);
        match &mut self.guidance {
            Some((r, chip)) if *r == region && !rekeyed => chip.set_shown(shown, now),
            _ => {
                let feedback = region && attempt > 0;
                self.guidance = GuidanceChip::new(t, self.reduced, now, shown, feedback)
                    .map(|chip| (region, chip));
            }
        }
    }

    /// Shipping `showSelectionFeedback` after a region click without a drag.
    pub fn selection_feedback(&mut self) {
        self.feedback_pending = true;
    }

    /// Shipping `clearSelectionFeedback` when a new drag starts.
    fn clear_selection_feedback(&mut self) {
        self.feedback_pending = false;
        if self.feedback != (0, None) {
            // Re-key at the next sync, like the attempt dropping to 0.
            self.feedback.1 = Some(f64::NEG_INFINITY);
        }
    }

    /// The bar's poses at `self.clock`: the options row's arrive and the open
    /// listbox's pop-in (`ui-pop-in`).
    fn options_pose(&self, t: &Tokens) -> Pose {
        // `pose_at` holds the first frame through the 40 ms delay even under
        // reduced motion, where the row must simply be there.
        let motion = Motion::CaptureMenuOptionsArrive.resolve(t);
        match (self.record_since, motion) {
            (Some(since), Some(motion)) if !self.reduced => {
                motion.pose_at(self.clock - since, false)
            }
            _ => Pose::REST,
        }
    }

    fn listbox_pose(&self, t: &Tokens) -> Pose {
        match (self.open_since, Motion::PopoverIn.resolve(t)) {
            (Some((_, since)), Some(motion)) => motion.pose_at(self.clock - since, self.reduced),
            _ => Pose::REST,
        }
    }

    /// The enabled Start recording dot's `recording-ready-ping`, repeating
    /// from when Record mounted; at rest (hidden) under reduced motion.
    fn ping_pose(&self, t: &Tokens) -> Option<Pose> {
        let since = self.record_since?;
        let motion = Motion::CaptureRecordReadyPing.resolve(t)?;
        Some(motion.pose_repeating(self.clock - since, self.reduced))
    }

    /// True while a motion runs and the window must keep painting.
    fn animating(&self, t: &Tokens) -> bool {
        let now = self.clock;
        let running = |since: Option<f64>, motion: Motion| {
            since.is_some_and(|since| {
                motion
                    .resolve(t)
                    .is_some_and(|m| m.running(now - since, self.reduced))
            })
        };
        let primary =
            capture_menu::primary_action(self.mode, self.auto_start, PrimaryState::default());
        let pinging = self.mode == MenuMode::Recording
            && !self.reduced
            && !primary.hidden
            && self.can_start();
        self.guidance
            .as_ref()
            .is_some_and(|(_, chip)| chip.animating(now))
            || running(self.record_since, Motion::CaptureMenuOptionsArrive)
            || running(self.open_since.map(|(_, at)| at), Motion::PopoverIn)
            || pinging
            // The feedback's end re-keys the chip.
            || self.feedback.1.is_some()
            || self.feedback_pending
            || self.tooltip.animating(t, now)
    }

    /// A capture shortcut with the bar up (shared `busy_route`, shipping's
    /// in-bar shortcut handling): another mode or target switches in place,
    /// clearing the hover and, off Window, the picked window; the same
    /// screenshot target again recaptures. Like Tauri a18afc62, shortcuts
    /// honour the bar's disabled Record button.
    fn shortcut(&mut self, action: capture_error::Action) -> ShortcutEffect {
        if self.starting || self.switching {
            return ShortcutEffect::Ignored;
        }
        let screenshot_target =
            (self.mode == MenuMode::Screenshot).then_some(route_target(self.target));
        let activity = if self.direct {
            capture_error::Activity::Selector
        } else {
            capture_error::Activity::Menu { screenshot_target }
        };
        let mode = |record| {
            if record {
                MenuMode::Recording
            } else {
                MenuMode::Screenshot
            }
        };
        match capture_error::busy_route(action, activity, None, false) {
            capture_error::BusyRoute::SwitchMenu { record, target } => {
                let window = target == capture_error::Target::Window;
                if (record && self.recording_locked) || (window && !self.window_available()) {
                    return ShortcutEffect::Ignored;
                }
                self.mode = mode(record);
                self.target = menu_target(target);
                self.hovered = None;
                self.hover = None;
                self.open = None;
                self.drag = None;
                if self.target != Target::Window {
                    self.picked = None;
                }
                ShortcutEffect::Switched
            }
            capture_error::BusyRoute::RecaptureMenu { record, target } => {
                ShortcutEffect::Recapture {
                    target: menu_target(target),
                    mode: mode(record),
                    direct: false,
                }
            }
            capture_error::BusyRoute::RecaptureSelector(target) => ShortcutEffect::Recapture {
                target: menu_target(target),
                mode: MenuMode::Screenshot,
                direct: true,
            },
            _ => ShortcutEffect::Ignored,
        }
    }

    /// The window target under the pointer; true when it changed.
    fn hover_window(&mut self, hit: Option<Option<usize>>) -> bool {
        std::mem::replace(&mut self.hovered, hit) != hit
    }

    /// A drawn region of at least two DIPs each way (shipping
    /// `isCapturableSelection`), a picked window, or Full screen.
    fn can_start(&self) -> bool {
        if self.switching || self.starting {
            return false;
        }
        match self.target {
            Target::Display => true,
            Target::Region => selection::capturable(self.selection.map(logical)),
            Target::Window => self.picked.is_some(),
        }
    }

    /// A select's options, which are disabled, and the selected index.
    fn entries(&self, field: Field) -> (Vec<String>, Vec<bool>, usize) {
        match field {
            Field::Aspect => (
                ASPECT_PRESETS.map(|(label, _)| label.to_string()).to_vec(),
                vec![false; ASPECT_PRESETS.len()],
                self.aspect,
            ),
            Field::Display => (
                self.displays.iter().map(|d| d.1.clone()).collect(),
                vec![false; self.displays.len()],
                self.display_index,
            ),
            _ => self.record.entries(field),
        }
    }

    /// Each option's description line (the Display select's size).
    fn details(&self, field: Field) -> Vec<Option<String>> {
        match field {
            Field::Display => self.displays.iter().map(|d| Some(d.2.clone())).collect(),
            _ => Vec::new(),
        }
    }

    /// The closed trigger's label.
    fn value(&self, field: Field) -> String {
        match field {
            Field::Aspect | Field::Display => {
                let (labels, _, selected) = self.entries(field);
                labels.into_iter().nth(selected).unwrap_or_default()
            }
            _ => self.record.value(field),
        }
    }

    fn field_enabled(&self, field: Field) -> bool {
        match field {
            Field::Aspect => true,
            // Shipping disables it with one display and while switching.
            Field::Display => self.displays.len() > 1 && !self.switching && !self.starting,
            _ => self.record.enabled(field),
        }
    }

    fn choose(&mut self, field: Field, index: usize) {
        match field {
            Field::Aspect => self.set_aspect(index),
            Field::Display => {
                if index != self.display_index && !self.switching {
                    self.switch_to = Some(index);
                }
            }
            _ => self.record.choose(field, index),
        }
    }

    fn aspect_ratio(&self) -> Option<f64> {
        ASPECT_PRESETS
            .get(self.aspect)
            .and_then(|(_, ratio)| *ratio)
    }

    fn bounds(&self) -> Bounds {
        Bounds {
            width: self.size.0.into(),
            height: self.size.1.into(),
        }
    }

    /// Shipping `changeRegionAspect`: re-run a drag in progress, else snap a
    /// settled region to the new ratio around its center.
    fn set_aspect(&mut self, index: usize) {
        self.aspect = index.min(ASPECT_PRESETS.len() - 1);
        if self.drag.is_some() {
            return self.apply_drag();
        }
        let Some(ratio) = self.aspect_ratio() else {
            return;
        };
        if let Some(r) = self
            .selection
            .map(logical)
            .filter(|r| selection::capturable(Some(*r)))
        {
            let fitted = selection::constrain(r, Some(ratio), Some(self.bounds()), MINIMUM_SIZE);
            self.selection = Some(d2d(fitted));
        }
    }

    /// What a press at `(x, y)` does to the region: a corner handle resizes,
    /// the frame moves, anywhere else draws a new one.
    fn region_mode(&self, x: f32, y: f32) -> DragMode {
        let Some(r) = self.selection.filter(|_| self.can_start()) else {
            return DragMode::Create;
        };
        let near = |cx: f32, cy: f32| (x - cx).hypot(y - cy) <= HANDLE_REACH;
        if near(r.left, r.top) {
            DragMode::Nw
        } else if near(r.right, r.top) {
            DragMode::Ne
        } else if near(r.left, r.bottom) {
            DragMode::Sw
        } else if near(r.right, r.bottom) {
            DragMode::Se
        } else if contains(&r, x, y) {
            DragMode::Move
        } else {
            DragMode::Create
        }
    }

    /// Shipping `applyRegionDrag` at the last pointer position.
    fn apply_drag(&mut self) {
        let Some(drag) = self.drag else { return };
        let (w, h) = self.size;
        let point = |(x, y): (f32, f32)| Point {
            x: f64::from(x.clamp(0., w)),
            y: f64::from(y.clamp(0., h)),
        };
        let r = selection::drag(
            drag.mode,
            point(drag.origin),
            point(self.pointer),
            logical(drag.initial),
            self.bounds(),
            DragOptions {
                minimum_size: MINIMUM_SIZE,
                aspect_ratio: self.aspect_ratio(),
                force_square: self.shift,
            },
        );
        self.selection = Some(d2d(r));
    }

    /// Shift pressed or released: a drag snaps between square and its
    /// aspect at once (shipping re-runs it on `keydown`/`keyup`).
    fn set_shift(&mut self, shift: bool) -> bool {
        if std::mem::replace(&mut self.shift, shift) == shift || self.drag.is_none() {
            return false;
        }
        self.apply_drag();
        true
    }

    /// The pointer cursor's resize or move shape over the region.
    fn region_cursor(&self, x: f32, y: f32) -> PCWSTR {
        let mode = self
            .drag
            .map_or_else(|| self.region_mode(x, y), |drag| drag.mode);
        match mode {
            DragMode::Nw | DragMode::Se => IDC_SIZENWSE,
            DragMode::Ne | DragMode::Sw => IDC_SIZENESW,
            DragMode::Move => IDC_SIZEALL,
            DragMode::Create => IDC_CROSS,
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
                self.mode == MenuMode::Screenshot,
            ),
            (
                label("Record")?,
                None,
                Hit::Record,
                !self.recording_locked,
                self.mode == MenuMode::Recording,
            ),
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
                target != Target::Window || self.window_available(),
                target == self.target,
            ));
        }
        let box_width = |items: &[(gfx::Text, Option<&str>, Hit, bool, bool)]| {
            8. + items.iter().map(|i| width(&i.0)).sum::<f32>()
        };
        let (aw, tw) = (box_width(&action), box_width(&targets));
        // `.recording-region-aspect-picker`: 150 px, "Aspect" then the select.
        // `.recording-display-picker`: a 190 px select with no caption.
        const ASPECT_W: f32 = 150.;
        let picker = match self.target {
            Target::Region => Some((Field::Aspect, "Aspect", ASPECT_W)),
            Target::Display => Some((Field::Display, "", DISPLAY_W)),
            Target::Window => None,
        };
        let picker = match picker {
            Some((field, caption, width)) => {
                let caption = g.text_sized(t, caption, t.n("text-xs"), "weight-medium", None)?;
                let value = g.text_sized(
                    t,
                    &self.value(field),
                    t.n("text-sm"),
                    "weight-regular",
                    None,
                )?;
                Some((field, caption, value, width))
            }
            None => None,
        };
        let state = PrimaryState {
            starting: self.starting,
            switching_display: self.switching,
            ..PrimaryState::default()
        };
        let primary = capture_menu::primary_action(self.mode, self.auto_start, state);
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
            + picker.as_ref().map_or(0., |p| GAP + p.3)
            + primary.as_ref().map_or(0., |(w, _)| GAP + 4. + w)
            + PAD;
        // `These controls **won’t** show in screenshots · Press Enter…`: the
        // menu closes before every capture, so it never shows in one. In
        // Record the note follows the recording-controls preference.
        let excluded = self.mode == MenuMode::Screenshot || self.controls_excluded;
        let visibility = capture_menu::visibility_note(self.mode, true, excluded);
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
        // The note's flex items, `--s-2` apart; a note with a Preferences
        // target is a link (`padding: 2px 5px`, then a 5 px gap and the 10 px
        // external icon).
        let groups = [
            (
                vec![
                    (visibility.lead.trim_end(), false, space),
                    (visibility.emphasis, true, 0.),
                    (visibility.trail.as_str(), false, 0.),
                ],
                visibility.target.map(Hit::Preferences),
            ),
            (vec![(capture_menu::NOTE_SEPARATOR, false, 0.)], None),
            (
                vec![(confirm.text, false, 0.)],
                confirm.target.map(Hit::Preferences),
            ),
        ];
        let mut note = Vec::new();
        let mut note_links = Vec::new();
        let mut x = 0.;
        for (index, (runs, link)) in groups.into_iter().enumerate() {
            if index > 0 {
                x += 4.;
            }
            let start = x;
            if link.is_some() {
                x += 5.;
            }
            for (text, strong, after) in runs {
                let laid = note_text(text, strong)?;
                let w = laid.width;
                note.push((laid, x, strong, link));
                x += w + after;
            }
            if let Some(hit) = link {
                let icon = x + 5.;
                x = icon + 10. + 5.;
                note_links.push((start, x, icon, hit));
            }
        }
        let note_w = x;
        let note_h = note.first().map_or(0., |(text, ..)| text.height);
        let record = self.mode == MenuMode::Recording;
        // `.recording-options-row`: 74 px min height, `--s-5` padding, top border.
        let options_h = if record { 78. } else { 0. };
        let mut pw = row.max(note_w + 24.) + 2.;
        if record {
            let columns = OPTION_COLUMNS.iter().sum::<f32>() + 8. * 5.;
            pw = pw.max(1. + 12. + columns + 12. + 1.);
        }
        let ph = 1. + 56. + options_h + 28. + 1.;
        let (w, h) = self.size;
        let (left, top) = self
            .panel_at
            .unwrap_or((((w - pw) / 2.).round(), (h - 26. - ph).round()));
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
        let mut pickers = Vec::new();
        if let Some((field, caption, value, box_w)) = picker {
            let left = x + GAP;
            let (trigger, width) = if field == Field::Display {
                // The trigger fills the picker; a long name ellipsizes before
                // the chevron.
                g.ellipsis(&value, (box_w - 2. - 16. - 12. - 14.).max(1.))?;
                (left, box_w)
            } else {
                // The flex item sizes to its trigger: `--s-4` padding, the
                // label, the `--s-5` gap, the 14 px chevron and the border.
                let trigger = left + caption.width + 6.;
                let w = (1. + 8. + value.width + 12. + 14. + 8. + 1.).min(left + box_w - trigger);
                (trigger, w)
            };
            pickers.push(FieldBox {
                caption,
                x: left,
                caption_y: cy - CAPTION_H / 2.,
                control: rect(trigger, cy - 18., trigger + width, cy + 18.),
                value,
                hit: Hit::Select(field),
                enabled: self.field_enabled(field),
            });
            x = left + box_w;
        }
        let primary = primary.map(|(w, text)| {
            let px = x + GAP + 4.;
            (rect(px, cy - 20., px + w, cy + 20.), text, self.can_start())
        });
        let options = if record {
            Some(self.options_layout(g, t, left, top + 1. + 56.)?)
        } else {
            None
        };
        let trigger = self.open.and_then(|(field, _)| {
            pickers
                .iter()
                .chain(options.iter().flat_map(|(_, fields)| fields))
                .find(|f| f.hit == Hit::Select(field))
                .map(|f| (field, f.control))
        });
        let listbox = match trigger {
            Some((field, control)) => Some(self.listbox_layout(g, t, field, control)?),
            None => None,
        };
        // `.capture-guidance`: `--s-4 --s-6` padding, a `--text-md` semibold
        // title over a `--text-xs` medium hint, 2 px apart.
        let guidance = match self.guidance_target() {
            Some(target) => {
                let copy = capture_menu::guidance(target, self.feedback.0 > 0);
                let title = g.text_sized(
                    t,
                    copy.title,
                    t.n("text-md"),
                    "weight-semibold",
                    Some("tracking-tight"),
                )?;
                let hint = g.text_sized(t, copy.hint, t.n("text-xs"), "weight-medium", None)?;
                let (px, py) = (t.n("s-6"), t.n("s-4"));
                let gw = (title.width.max(hint.width) + 2. * px + 2.).ceil();
                let gh = (title.height + 2. + hint.height + 2. * py + 2.).ceil();
                let gl = ((w - gw) / 2.).round();
                let gt = capture_menu::guidance_top(f64::from(h)).round() as f32;
                Some(GuidanceBox {
                    rect: rect(gl, gt, gl + gw, gt + gh),
                    title,
                    hint,
                })
            }
            None => None,
        };
        let note_y = top + 1. + 56. + options_h + 10.;
        Ok(Layout {
            guidance,
            panel,
            options,
            listbox,
            close,
            switches: [action_box, target_box],
            divider,
            segments,
            pickers,
            primary,
            note: note
                .into_iter()
                .map(|(text, nx, strong, link)| {
                    (text, left + (pw - note_w) / 2. + nx, strong, link)
                })
                .collect(),
            note_y,
            links: note_links
                .into_iter()
                .map(|(from, to, icon, hit)| {
                    let x = left + (pw - note_w) / 2.;
                    let r = rect(x + from, note_y - 2., x + to, note_y + note_h + 2.);
                    (r, x + icon, hit)
                })
                .collect(),
        })
    }

    /// The `.recording-options-row` fields below the bar's first row.
    fn options_layout(
        &self,
        g: &Gfx,
        t: &Tokens,
        left: f32,
        row_top: f32,
    ) -> Result<(f32, Vec<FieldBox>)> {
        let caption_y = row_top + 1. + 12.;
        let control_y = caption_y + CAPTION_H + 6.;
        let items = [
            (capture_menu::FIELD_FPS, Hit::Select(Field::Fps)),
            (
                capture_menu::FIELD_MAX_RESOLUTION,
                Hit::Select(Field::Resolution),
            ),
            (
                RecordingToggle::ShowCursor.label(),
                Hit::Toggle(RecordingToggle::ShowCursor),
            ),
            (
                RecordingToggle::ShowClicks.label(),
                Hit::Toggle(RecordingToggle::ShowClicks),
            ),
            (
                RecordingToggle::DesktopAudio.label(),
                Hit::Toggle(RecordingToggle::DesktopAudio),
            ),
            (
                capture_menu::FIELD_MICROPHONE,
                Hit::Select(Field::Microphone),
            ),
        ];
        let mut x = left + 1. + 12.;
        let mut fields = Vec::new();
        for ((label, hit), width) in items.into_iter().zip(OPTION_COLUMNS) {
            // `.recording-field > span`: uppercase `--text-2xs` semibold, wide.
            let caption = g.text_sized(
                t,
                &label.to_uppercase(),
                t.n("text-2xs"),
                "weight-semibold",
                Some("tracking-wide"),
            )?;
            let control = rect(x, control_y, x + width, control_y + 36.);
            let (value, enabled) = match hit {
                // The trigger inherits the field's weight and tracking at `--text-sm`.
                Hit::Select(field) => {
                    let value = g.text_sized(
                        t,
                        &self.record.value(field),
                        t.n("text-sm"),
                        "weight-semibold",
                        Some("tracking-wide"),
                    )?;
                    // `--s-4` padding, `--s-5` gap and the 14 px chevron.
                    g.ellipsis(&value, (width - 16. - 12. - 14.).max(1.))?;
                    (value, self.record.enabled(field))
                }
                Hit::Toggle(toggle) => {
                    let available = self.record.available(toggle);
                    let status = capture_menu::toggle_status(available, self.record.on(toggle));
                    let value = g.text_sized(t, status, t.n("text-xs"), "weight-medium", None)?;
                    (value, available)
                }
                _ => continue,
            };
            fields.push(FieldBox {
                caption,
                x,
                caption_y,
                control,
                value,
                hit,
                enabled,
            });
            x += width + 8.;
        }
        Ok((row_top, fields))
    }

    /// The open glass `.custom-select-listbox`, placed like shipping.
    fn listbox_layout(&self, g: &Gfx, t: &Tokens, field: Field, trigger: Rect) -> Result<Listbox> {
        let (labels, disabled, selected) = self.entries(field);
        let check = g.text_sized(t, "✓", t.n("text-sm"), "weight-regular", None)?;
        let mut texts = Vec::new();
        for (index, label) in labels.iter().enumerate() {
            // `[aria-selected="true"]` is `--weight-medium`.
            let weight = if index == selected {
                "weight-medium"
            } else {
                "weight-regular"
            };
            texts.push(g.text_sized(t, label, t.n("text-sm"), weight, None)?);
        }
        let mut details = Vec::new();
        for detail in self.details(field) {
            details.push(match detail {
                Some(detail) => {
                    Some(g.text_sized(t, &detail, t.n("text-xs"), "weight-regular", None)?)
                }
                None => None,
            });
        }
        details.resize_with(texts.len(), || None);
        // `.custom-select-option-copy`: the label's 1 px block padding, then
        // the `small` line 2 px below it.
        let row_h = texts
            .iter()
            .zip(&details)
            .map(|(text, detail)| {
                text.height + 2. + detail.as_ref().map_or(0., |d| 2. + d.height) + 12.
            })
            .fold(28_f32, f32::max);
        let label_w = texts
            .iter()
            .zip(&details)
            .map(|(text, detail)| text.width.max(detail.as_ref().map_or(0., |d| d.width)))
            .fold(0., f32::max);
        let content_w = (label_w + 16. + check.width + 16. + 8. + 2.).ceil();
        let content_h = texts.len() as f32 * row_h + 8.;
        let (w, h) = self.size;
        let layout = select::layout(
            select::Rect {
                left: f64::from(trigger.left),
                top: f64::from(trigger.top),
                width: f64::from(trigger.right - trigger.left),
                height: f64::from(trigger.bottom - trigger.top),
            },
            f64::from(content_w),
            f64::from(content_h + 2.),
            f64::from(w),
            f64::from(h),
            texts.len(),
        );
        let (left, top) = (layout.left as f32, layout.top as f32);
        let height = (layout.max_height as f32).min(content_h + 2.);
        let list = rect(left, top, left + layout.width as f32, top + height);
        let rows = texts
            .into_iter()
            .enumerate()
            .map(|(index, text)| {
                let y = top + 1. + 4. + index as f32 * row_h;
                let row = rect(list.left + 5., y, list.right - 5., y + row_h);
                (row, text, disabled[index], index == selected)
            })
            .collect();
        Ok(Listbox {
            rect: list,
            rows,
            details,
            check,
        })
    }

    fn hit(&self, x: f32, y: f32) -> Option<Hit> {
        if self.direct {
            return None;
        }
        let layout = self.layout.as_ref()?;
        if let Some(listbox) = &layout.listbox
            && contains(&listbox.rect, x, y)
        {
            return listbox
                .rows
                .iter()
                .position(|(r, ..)| contains(r, x, y))
                .map(Hit::Option);
        }
        if let Some(field) = layout
            .fields()
            .find(|f| f.enabled && contains(&f.control, x, y))
        {
            return Some(field.hit);
        }
        if contains(&layout.close, x, y) {
            return Some(Hit::Close);
        }
        if let Some((.., hit)) = layout.links.iter().find(|(r, ..)| contains(r, x, y)) {
            return Some(*hit);
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

    /// Over the bar or its open listbox.
    fn over_panel(&self, x: f32, y: f32) -> bool {
        !self.direct
            && self.layout.as_ref().is_some_and(|l| {
                contains(&l.panel, x, y)
                    || l.listbox.as_ref().is_some_and(|b| contains(&b.rect, x, y))
            })
    }

    pub fn paint(
        &mut self,
        dc: &ID2D1DeviceContext,
        g: &Gfx,
        t: &Tokens,
        frozen: Option<&ID2D1Bitmap1>,
    ) -> Result<()> {
        self.sync_motion(t);
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
            // `CaptureDim`: the shade, cut out around a drawn region, then
            // `.recording-selection-frame` with its size and handles.
            Target::Region => match self
                .selection
                .filter(|r| r.right > r.left && r.bottom > r.top)
            {
                None => gfx::fill(dc, full, shade(0.38))?,
                Some(r) => {
                    shade_around(dc, self.size, r, shade(0.38))?;
                    self.paint_frame(dc, g, t, r)?;
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
                    shade_around(dc, self.size, r, shade(0.3))?;
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
        if let (Some(guide), Some((_, chip))) = (&layout.guidance, &self.guidance) {
            self.paint_guidance(dc, t, guide, chip.pose(self.clock))?;
        }
        // The direct selectors have no bar.
        if self.direct {
            self.layout = Some(layout);
            return Ok(());
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
            let pill = (r.bottom - r.top) / 2.;
            let shadow = [Shadow {
                x: 0.,
                y: 6.,
                blur: 18.,
                spread: 0.,
                color: [0., 0., 0., 0.34 * opacity],
            }];
            gfx::shadow(dc, r, pill, &shadow)?;
            let fill = if *enabled && hovered(Hit::Primary) {
                "theme-accent-hover"
            } else {
                "theme-accent"
            };
            gfx::fill_round(dc, r, pill, t.color_alpha(fill, opacity))?;
            gfx::top_highlight(dc, r, pill, t.color_alpha("primary-highlight", opacity))?;
            gfx::border(
                dc,
                r,
                pill,
                1.,
                D2D1_COLOR_F {
                    r: 0.,
                    g: 0.,
                    b: 0.,
                    a: 0.16 * opacity,
                },
            )?;
            let ink = t.color_alpha("theme-accent-ink", opacity);
            let content = 16. + 6. + label.width;
            let x = (r.left + r.right - content) / 2.;
            let cy = (r.top + r.bottom) / 2.;
            if self.mode == MenuMode::Recording {
                // `.recording-start .capture-record-dot` on the accent.
                let dot = rect(x + 2., cy - 6., x + 14., cy + 6.);
                gfx::fill_round(dc, dot, 6., t.color_alpha("theme-signal", opacity))?;
                gfx::border(dc, dot, 6., 1.5, alpha(ink, 0.6))?;
                // `recording-ready-ping`: a 0.7 copy of the dot swells to 2×
                // and fades while Start recording is enabled.
                if let Some(ping) = self.ping_pose(t).filter(|_| *enabled) {
                    let center = (x + 8., cy);
                    with_pose(dc, &ping, center, || {
                        let ping = rect(x + 3., cy - 5., x + 13., cy + 5.);
                        gfx::fill_round(dc, ping, 5., t.color_alpha("theme-signal", 0.7))
                    })?;
                }
            } else {
                gfx::icon(dc, &g.factory, "capture", &[1], (x, cy - 8., 16.), 1.8, ink)?;
            }
            gfx::text(dc, label, x + 22., cy - label.height / 2., ink)?;
        }
        self.paint_fields(dc, g, t, &layout.pickers)?;
        if let Some((row_top, fields)) = &layout.options {
            // `recording-options-arrive`: fades in from 5 px up, 40 ms late.
            let pose = self.options_pose(t);
            with_pose(dc, &pose, (0., 0.), || {
                self.paint_options(dc, g, t, &layout.panel, *row_top, fields)
            })?;
        }
        self.paint_focus(dc, t, &layout)?;
        // `.capture-selector-note`; a hovered or focused Preferences link
        // lights `--glass-hover` with `--theme-accent-text-strong` ink and a
        // fully opaque icon.
        let lit =
            |hit: Hit| self.hover == Some(hit) || (self.focus_visible && self.focus == Some(hit));
        let link_ink = t.color("theme-accent-text-strong");
        for (r, icon_x, hit) in &layout.links {
            let icon = if lit(*hit) {
                gfx::fill_round(dc, *r, t.n("r-sm"), t.color("glass-hover"))?;
                link_ink
            } else {
                alpha(t.color("glass-text-subtle"), 0.72)
            };
            // The 16-unit shipping icon drawn 10 DIPs wide, 1.6 units thick.
            let y = (r.top + r.bottom) / 2. - 5.;
            gfx::icon(
                dc,
                &g.factory,
                "external-link",
                &[],
                (*icon_x, y, 15.),
                1.6,
                icon,
            )?;
        }
        for (text, x, strong, link) in &layout.note {
            let color = if link.is_some_and(lit) {
                link_ink
            } else if *strong {
                glass_text
            } else {
                t.color("glass-text-subtle")
            };
            gfx::text(dc, text, *x, layout.note_y, color)?;
        }
        if let Some(listbox) = &layout.listbox {
            // `ui-pop-in` about the listbox's center.
            let r = listbox.rect;
            let center = ((r.left + r.right) / 2., (r.top + r.bottom) / 2.);
            with_pose(dc, &self.listbox_pose(t), center, || {
                self.paint_listbox(dc, t, listbox)
            })?;
        }
        self.layout = Some(layout);
        for (hit, progress) in self.tooltip.frames(t, self.clock) {
            // The anchor is laid out from this paint; a key whose control
            // left the bar draws nothing.
            let anchor = self.tooltip_anchor(hit);
            if let Some((label, anchor)) = anchor {
                use crate::tooltip::{self, Align};
                let gap = tooltip::GAP;
                tooltip::paint(dc, g, t, label, anchor, Align::Center, gap, full, progress)?;
            }
        }
        Ok(())
    }

    fn paint_options(
        &self,
        dc: &ID2D1DeviceContext,
        g: &Gfx,
        t: &Tokens,
        panel: &Rect,
        row_top: f32,
        fields: &[FieldBox],
    ) -> Result<()> {
        let border = rect(panel.left + 1., row_top, panel.right - 1., row_top + 1.);
        gfx::fill(dc, border, t.color("glass-border"))?;
        self.paint_fields(dc, g, t, fields)
    }

    /// The focused control's `:focus-visible` ring: Close and the primary's
    /// `outline: 2px solid accent; outline-offset: 2px`, the segments' outline
    /// over `--focus-ring-tight`, a select's `--focus-ring-tight` (its accent
    /// border is drawn with the trigger) and a switch's `--focus-ring`. A
    /// focused window target lights like a hovered one instead.
    fn paint_focus(&self, dc: &ID2D1DeviceContext, t: &Tokens, layout: &Layout) -> Result<()> {
        let Some(hit) = self.focus.filter(|_| self.focus_visible) else {
            return Ok(());
        };
        let accent = t.color("theme-accent");
        let tight = t.color_alpha("theme-accent", 0.55);
        match hit {
            Hit::Close => gfx::ring(dc, layout.close, 8., 2., 4., accent),
            Hit::Primary => match &layout.primary {
                Some((r, _, true)) => gfx::ring(dc, *r, 10., 2., 4., accent),
                _ => Ok(()),
            },
            Hit::Screenshot | Hit::Record | Hit::Target(_) => {
                let Some(s) = layout.segments.iter().find(|s| s.hit == hit && s.enabled) else {
                    return Ok(());
                };
                gfx::ring(dc, s.rect, 6., 0., 2., tight)?;
                gfx::ring(dc, s.rect, 6., 2., 4., accent)
            }
            Hit::Select(_) | Hit::Toggle(_) => {
                let Some(f) = layout.fields().find(|f| f.hit == hit && f.enabled) else {
                    return Ok(());
                };
                let c = f.control;
                if matches!(hit, Hit::Select(_)) {
                    return gfx::ring(dc, c, 8., 0., 2., tight);
                }
                let cy = (c.top + c.bottom) / 2.;
                let switch = rect(c.left, cy - 9., c.left + 30., cy + 9.);
                gfx::ring(dc, switch, 9., 0., 2., t.color("surface-canvas"))?;
                gfx::ring(dc, switch, 9., 2., 4., tight)
            }
            Hit::Preferences(_) => match layout.links.iter().find(|(.., h)| *h == hit) {
                Some((r, ..)) => gfx::ring(dc, *r, t.n("r-sm"), 2., 4., accent),
                None => Ok(()),
            },
            Hit::Option(_) | Hit::Pick(_) => Ok(()),
        }
    }

    /// Selects and switches with their captions.
    fn paint_fields(
        &self,
        dc: &ID2D1DeviceContext,
        g: &Gfx,
        t: &Tokens,
        fields: &[FieldBox],
    ) -> Result<()> {
        let accent = t.color("theme-accent");
        for f in fields {
            let cy_caption = f.caption_y + (CAPTION_H - f.caption.height) / 2.;
            gfx::text(
                dc,
                &f.caption,
                f.x,
                cy_caption,
                t.color("glass-text-subtle"),
            )?;
            let c = f.control;
            let cy = (c.top + c.bottom) / 2.;
            // Disabled controls are at 50% (`:disabled`, `:has(input:disabled)`).
            let fade = if f.enabled { 1. } else { 0.5 };
            let hovered = f.enabled && self.hover == Some(f.hit);
            match f.hit {
                // `.recording-selector-panel .custom-select-trigger`
                Hit::Select(field) => {
                    let open = self.open.is_some_and(|(o, _)| o == field);
                    gfx::fill_round(dc, c, 8., shade(0.3 * fade))?;
                    let focused = self.focus_visible && self.focus == Some(f.hit);
                    let edge = if open || focused {
                        accent
                    } else if hovered {
                        t.color("glass-border-strong")
                    } else {
                        t.color("glass-border")
                    };
                    gfx::border(dc, c, 8., 1., alpha(edge, fade))?;
                    let ink = alpha(t.color("glass-text"), fade);
                    gfx::text(dc, &f.value, c.left + 8., cy - f.value.height / 2., ink)?;
                    // The chevron turns over while open.
                    let icon = if open { "chevron-up" } else { "chevron-down" };
                    let subtle = alpha(t.color("glass-text-subtle"), fade);
                    gfx::icon(
                        dc,
                        &g.factory,
                        icon,
                        &[],
                        (c.right - 8. - 14., cy - 7., 14.),
                        1.7,
                        subtle,
                    )?;
                }
                // `.recording-toggle`: the 30×18 `.recording-switch` and its status.
                Hit::Toggle(toggle) => {
                    let on = f.enabled && self.record.on(toggle);
                    let switch = rect(c.left, cy - 9., c.left + 30., cy + 9.);
                    let knob_x = c.left + 3. + if on { 12. } else { 0. };
                    let knob = rect(knob_x, cy - 6., knob_x + 12., cy + 6.);
                    if on {
                        gfx::fill_round(dc, switch, 9., alpha(accent, fade))?;
                        let ink = t.color_alpha("theme-accent-ink", fade);
                        gfx::fill_round(dc, knob, 6., ink)?;
                    } else {
                        gfx::fill_round(dc, switch, 9., shade(0.35 * fade))?;
                        let edge = t.color_alpha("glass-border-strong", fade);
                        gfx::border(dc, switch, 9., 1., edge)?;
                        let knob_color = t.color_alpha("glass-text-subtle", fade);
                        gfx::fill_round(dc, knob, 6., knob_color)?;
                    }
                    let status = if hovered {
                        "glass-text"
                    } else {
                        "glass-text-muted"
                    };
                    let y = cy - f.value.height / 2.;
                    gfx::text(dc, &f.value, c.left + 36., y, t.color_alpha(status, fade))?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// `.custom-select-listbox-glass`: raised glass, hovered row lit, the
    /// selected row's check in the accent.
    fn paint_listbox(&self, dc: &ID2D1DeviceContext, t: &Tokens, listbox: &Listbox) -> Result<()> {
        let r = listbox.rect;
        gfx::shadow(dc, r, 10., t.shadow("shadow-lg"))?;
        gfx::fill_round(dc, r, 10., t.color("glass-raised"))?;
        gfx::border(dc, r, 10., 1., t.color("glass-border"))?;
        let active = self.open.map(|(_, state)| state.active);
        unsafe { dc.PushAxisAlignedClip(&r, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE) };
        let mut result = Ok(());
        for (index, (row, label, disabled, selected)) in listbox.rows.iter().enumerate() {
            let lit = !disabled && active == Some(index);
            let color = if *disabled {
                t.color("glass-text-subtle")
            } else if lit || *selected {
                t.color("glass-text")
            } else {
                t.color("glass-text-muted")
            };
            let cy = (row.top + row.bottom) / 2.;
            result = (|| {
                if lit {
                    gfx::fill_round(dc, *row, 6., t.color("glass-hover"))?;
                }
                match listbox.details.get(index).and_then(Option::as_ref) {
                    Some(detail) => {
                        let top = cy - (label.height + 2. + detail.height) / 2.;
                        gfx::text(dc, label, row.left + 8., top, color)?;
                        let y = top + label.height + 2.;
                        let subtle = t.color("glass-text-subtle");
                        gfx::text(dc, detail, row.left + 8., y, subtle)?;
                    }
                    None => gfx::text(dc, label, row.left + 8., cy - label.height / 2., color)?,
                }
                if *selected {
                    let check = &listbox.check;
                    let x = row.right - 8. - check.width;
                    gfx::text(
                        dc,
                        check,
                        x,
                        cy - check.height / 2.,
                        t.color("theme-accent"),
                    )?;
                }
                Ok(())
            })();
            if result.is_err() {
                break;
            }
        }
        unsafe { dc.PopAxisAlignedClip() };
        result
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

    /// `.recording-selection-frame.movable`: the 1.5 px accent border inside
    /// its hairlines, the centered `.selection-dimensions` label above it
    /// (inside once the region reaches the top 30 px) and the four corner
    /// `.handle`s.
    fn paint_frame(&self, dc: &ID2D1DeviceContext, g: &Gfx, t: &Tokens, r: Rect) -> Result<()> {
        let accent = t.color("theme-accent");
        gfx::ring(dc, r, 2., 0., 1., t.color("selection-hairline-outer"))?;
        gfx::border(dc, r, 2., 1.5, accent)?;
        let inner = rect(r.left + 1.5, r.top + 1.5, r.right - 1.5, r.bottom - 1.5);
        if inner.right > inner.left && inner.bottom > inner.top {
            gfx::border(dc, inner, 0.5, 1., t.color("selection-hairline-inner"))?;
        }
        let size = format!(
            "{} × {}",
            (r.right - r.left).round() as i64,
            (r.bottom - r.top).round() as i64
        );
        let text = g.text_sized(t, &size, t.n("text-xs"), "weight-semibold", None)?;
        let (w, h) = (text.width + 16., text.height + 8.);
        let x = ((r.left + r.right - w) / 2.).round();
        let y = if r.top < 30. {
            r.top + 1.5 + 6.
        } else {
            r.top + 1.5 - 30.
        };
        let label = rect(x, y, x + w, y + h);
        gfx::shadow(dc, label, 6., t.shadow("shadow-sm"))?;
        gfx::fill_round(dc, label, 6., accent)?;
        gfx::text(dc, &text, x + 8., y + 4., t.color("theme-accent-ink"))?;
        // The direct selector's `.selection-box` has no handles.
        if self.direct {
            return Ok(());
        }
        let handle_shadow = [Shadow {
            x: 0.,
            y: 1.,
            blur: 4.,
            spread: 0.,
            color: [0., 0., 0., 0.4],
        }];
        for (cx, cy) in [
            (r.left, r.top),
            (r.right, r.top),
            (r.left, r.bottom),
            (r.right, r.bottom),
        ] {
            let dot = rect(cx - 5., cy - 5., cx + 5., cy + 5.);
            gfx::shadow(dc, dot, 5., &handle_shadow)?;
            gfx::fill_round(dc, dot, 5., t.color("theme-accent-ink"))?;
            gfx::fill_round(dc, rect(cx - 3., cy - 3., cx + 3., cy + 3.), 3., accent)?;
        }
        Ok(())
    }

    /// `.capture-guidance`: a strong-glass card, its title over the muted
    /// hint, at the chip's pose (fade, entrance slide, feedback nudge).
    fn paint_guidance(
        &self,
        dc: &ID2D1DeviceContext,
        t: &Tokens,
        guide: &GuidanceBox,
        chip: capture_menu::GuidancePose,
    ) -> Result<()> {
        let pose = Pose {
            opacity: chip.opacity,
            translate_x: chip.translate_x,
            translate_y: chip.translate_y,
            ..Pose::REST
        };
        let feedback = chip.accent;
        with_pose(dc, &pose, (0., 0.), || {
            let r = guide.rect;
            let radius = t.n("r-xl");
            gfx::shadow(dc, r, radius, t.shadow("glass-shadow"))?;
            gfx::fill_round(dc, r, radius, t.color("glass-strong"))?;
            let edge = if feedback {
                let alpha = capture_menu::GUIDANCE_FEEDBACK_BORDER_ALPHA as f32;
                t.color_alpha("theme-accent", alpha)
            } else {
                t.color("glass-border-strong")
            };
            gfx::border(dc, r, radius, 1., edge)?;
            let cx = (r.left + r.right) / 2.;
            let top = r.top + 1. + t.n("s-4");
            let title = &guide.title;
            gfx::text(dc, title, cx - title.width / 2., top, t.color("glass-text"))?;
            let hint = &guide.hint;
            let y = top + title.height + capture_menu::GUIDANCE_ROW_GAP as f32;
            gfx::text(
                dc,
                hint,
                cx - hint.width / 2.,
                y,
                t.color("glass-text-muted"),
            )
        })
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
        self.pointer = (x, y);
        // Shipping `movePanel`: kept 8 px inside the display.
        if let (Some((dx, dy)), Some(layout)) = (self.panel_drag, self.layout.as_ref()) {
            let p = layout.panel;
            let (w, h) = self.size;
            let max = (
                (w - (p.right - p.left) - 8.).max(8.),
                (h - (p.bottom - p.top) - 8.).max(8.),
            );
            self.panel_at = Some(((x - dx).clamp(8., max.0), (y - dy).clamp(8., max.1)));
            return true;
        }
        if self.drag.is_some() {
            self.apply_drag();
            return true;
        }
        let hover = self.hit(x, y);
        let tip = self.tooltip_at(x, y).map(|(hit, ..)| hit);
        let mut changed = std::mem::replace(&mut self.tooltip_target, tip) != tip;
        // The chip fades away from a pointer near it (shipping
        // `isPointerOverCaptureGuidance`, with hysteresis).
        if let Some(r) = self
            .layout
            .as_ref()
            .and_then(|l| l.guidance.as_ref())
            .map(|g| g.rect)
        {
            let over = capture_menu::pointer_over_guidance(
                x.into(),
                y.into(),
                r.left.into(),
                r.top.into(),
                r.right.into(),
                r.bottom.into(),
                self.guidance_over,
            );
            changed |= std::mem::replace(&mut self.guidance_over, over) != over;
        }
        // Pointing at an enabled row highlights it (`onPointerEnter`).
        if let (Some(Hit::Option(index)), Some((field, _))) = (hover, self.open)
            && !self.entries(field).1[index]
            && let Some((_, state)) = self.open.as_mut()
            && state.active != index
        {
            state.active = index;
            changed = true;
        }
        std::mem::replace(&mut self.hover, hover) != hover || changed
    }

    fn pressed(&mut self, x: f32, y: f32) -> bool {
        self.pressed = self.hit(x, y);
        // A click focuses the control without its ring, or blurs it; a
        // listbox row keeps focus on its trigger.
        if !matches!(self.pressed, Some(Hit::Option(_))) {
            self.focus = self.pressed;
            self.focus_visible = false;
        }
        // A press anywhere else closes the listbox and still acts there.
        let mut closed = false;
        if let Some((field, _)) = self.open
            && self.pressed != Some(Hit::Select(field))
            && !self
                .layout
                .as_ref()
                .and_then(|l| l.listbox.as_ref())
                .is_some_and(|b| contains(&b.rect, x, y))
        {
            self.open = None;
            closed = true;
        }
        // Shipping `beginPanelDrag`: the bar's blank glass drags it.
        if let Some(layout) = self.layout.as_ref().filter(|_| !self.direct)
            && self.pressed.is_none()
            && contains(&layout.panel, x, y)
            && !layout
                .listbox
                .as_ref()
                .is_some_and(|b| contains(&b.rect, x, y))
        {
            let panel = layout.panel;
            self.panel_drag = Some((x - panel.left, y - panel.top));
            self.panel_at = Some((panel.left, panel.top));
            return true;
        }
        if closed && self.pressed.is_none() && self.over_panel(x, y) {
            return true;
        }
        if self.pressed.is_none() && self.target == Target::Region && !self.over_panel(x, y) {
            // Shipping `onPointerDown`: a handle resizes, the frame moves and
            // anywhere else draws a new region from this point.
            let mode = self.region_mode(x, y);
            if self.direct {
                self.clear_selection_feedback();
            }
            let origin = rect(x, y, x, y);
            self.pointer = (x, y);
            self.drag = Some(RegionDrag {
                mode,
                origin: (x, y),
                initial: match mode {
                    DragMode::Create => origin,
                    _ => self.selection.unwrap_or(origin),
                },
            });
            if matches!(mode, DragMode::Create) {
                self.selection = Some(origin);
            }
            return true;
        }
        closed
    }

    /// Run a click on an options-row control or listbox row.
    fn option_action(&mut self, hit: Hit) {
        match hit {
            Hit::Toggle(toggle) => self.record.toggle(toggle),
            Hit::Select(field) if self.open.is_some_and(|(o, _)| o == field) => self.open = None,
            Hit::Select(field) if self.field_enabled(field) => {
                let (_, disabled, selected) = self.entries(field);
                self.open = Some((field, select::open(&disabled, selected)));
            }
            // A disabled row leaves the listbox open, like shipping.
            Hit::Option(index) => {
                if let Some((field, _)) = self.open
                    && !self.entries(field).1.get(index).copied().unwrap_or(true)
                {
                    self.choose(field, index);
                    self.open = None;
                }
            }
            _ => {}
        }
    }

    /// Keys for the open listbox, or a focused trigger's (arrows, Enter and
    /// Space open it); true when consumed.
    fn listbox_key(&mut self, key: select::Key) -> bool {
        let (field, state) = match (self.open, self.focus) {
            (Some(open), _) => open,
            (None, Some(Hit::Select(field))) if self.field_enabled(field) => {
                (field, select::State::default())
            }
            _ => return false,
        };
        let (_, disabled, selected) = self.entries(field);
        let outcome = select::key(state, &disabled, selected, key);
        self.focus_visible |= outcome.handled && self.focus == Some(Hit::Select(field));
        if let Some(index) = outcome.chosen {
            self.choose(field, index);
        }
        self.open = outcome.state.open.then_some((field, outcome.state));
        outcome.handled
    }

    /// A click outside the bar in Window mode picks the hovered target;
    /// auto-start then confirms it.
    fn pick(&mut self) -> Option<Hit> {
        let hovered = self.hovered?;
        self.picked = Some(hovered);
        // The direct selector commits on click.
        (self.auto_start || self.direct).then_some(Hit::Primary)
    }

    /// Release; returns the action to run.
    fn released(&mut self, x: f32, y: f32) -> Option<Hit> {
        if self.panel_drag.take().is_some() {
            return None;
        }
        if let Some(drag) = self.drag {
            self.pointer = (x, y);
            self.apply_drag();
            self.drag = None;
            if !self.can_start() {
                self.selection = None;
                // The direct selector commits on release; a click without a
                // drag nudges the guidance instead.
                if self.direct {
                    self.selection_feedback();
                }
            }
            if self.direct {
                return self.can_start().then_some(Hit::Primary);
            }
            // Only a finished create auto-starts; move and resize adjust.
            let created = matches!(drag.mode, DragMode::Create);
            return (created && self.auto_start && self.can_start()).then_some(Hit::Primary);
        }
        let hit = self.hit(x, y);
        (self.pressed.take() == hit).then_some(hit).flatten()
    }
}

/// Draw `f` at a motion pose: its opacity through a layer, and its
/// translation and scale about `origin` composed onto the current transform.
fn with_pose(
    dc: &ID2D1DeviceContext,
    pose: &Pose,
    origin: (f32, f32),
    f: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let opacity = pose.opacity.clamp(0., 1.) as f32;
    if opacity <= 0. {
        return Ok(());
    }
    let (s, (ox, oy)) = (pose.scale as f32, origin);
    let mut base = Matrix3x2::default();
    unsafe { dc.GetTransform(&mut base) };
    let local = Matrix3x2 {
        M11: s,
        M12: 0.,
        M21: 0.,
        M22: s,
        M31: ox - ox * s + pose.translate_x as f32,
        M32: oy - oy * s + pose.translate_y as f32,
    };
    let layer = D2D1_LAYER_PARAMETERS1 {
        contentBounds: rect(-1e6, -1e6, 1e6, 1e6),
        maskTransform: Matrix3x2::identity(),
        opacity,
        ..Default::default()
    };
    unsafe {
        dc.SetTransform(&(local * base));
        dc.PushLayer(&layer, None::<&ID2D1Layer>);
    }
    let result = f();
    unsafe {
        dc.PopLayer();
        dc.SetTransform(&base);
    }
    result
}

/// What confirmation captures.
enum Choice {
    Session(window_target::Target),
    Display,
}

pub type CaptureResult = std::result::Result<Option<captures_app::Artifact>, String>;

/// What a finished New Capture leaves for the host.
pub enum Outcome {
    Captured(Box<CaptureResult>),
    /// Record was confirmed: the recorder takes over the flow and its Escape.
    Record(Box<Handoff>),
    /// A note link closed the bar to open Preferences at this row.
    Preferences(PreferenceTarget),
}

pub struct Handoff {
    pub flow: CaptureFlow,
    pub display: DisplayDescriptor,
    pub target: RecordingTarget,
    pub settings: captures_settings::AppSettings,
    pub tokens: Tokens,
}

/// A display's prepared session and every display for the Display select;
/// `None` when the flow was cancelled meanwhile.
type Prepared = std::result::Result<Option<(WindowSession, Vec<DisplayDescriptor>)>, String>;

enum Phase {
    Preparing(Receiver<Prepared>),
    Selecting(Box<Overlay>, Box<WindowSession>),
    Counting(Box<Counting>),
    Capturing(Receiver<CaptureResult>),
    Record(Box<(DisplayDescriptor, RecordingTarget)>),
    Done(Box<CaptureResult>),
}

struct Capture {
    flow: CaptureFlow,
    target: Target,
    mode: MenuMode,
    settings: captures_settings::AppSettings,
    tokens: Tokens,
    root: std::path::PathBuf,
    wake: std::sync::Arc<dyn Fn() + Send + Sync>,
    phase: Phase,
    /// Open as a bar-less direct selector (Screenshot Region and Window).
    direct: bool,
    /// The display the Display select is switching to, or a recapture of the
    /// open UI's own display with where it lands, preparing while the open
    /// UI stays up.
    switching: Option<(Receiver<Prepared>, Option<Landing>)>,
    /// The bar shows a snapshot of the capture UI itself: Full screen saves
    /// that snapshot, and no countdown runs (shipping
    /// `screenshot_countdown_seconds_for_capture_ui`).
    recaptured: bool,
    /// Where a finished recapture lands, for `show`.
    arrive_as: Option<Landing>,
    /// A note link cancelled the bar to open Preferences at this row.
    preferences: Option<PreferenceTarget>,
}

/// Where a recapture opens: the target, the mode, and whether it is a
/// bar-less direct selector.
type Landing = (Target, MenuMode, bool);

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
    /// Microphones enumerating off the UI thread.
    microphones: Option<Receiver<Vec<(String, String)>>>,
}

/// Enumerate microphones once the menu first shows Record (shipping
/// `loadAudioDevices`); the select reads "Loading microphones…" meanwhile.
fn load_microphones(overlay: &mut Overlay, wake: &std::sync::Arc<dyn Fn() + Send + Sync>) {
    if overlay.menu.mode != MenuMode::Recording || !overlay.menu.record.take_microphone_request() {
        return;
    }
    let (sender, receiver) = channel();
    let wake = wake.clone();
    std::thread::spawn(move || {
        let devices = captures_recording_platform::microphone_devices()
            .into_iter()
            .map(|device| (device.id, device.name))
            .collect::<Vec<_>>();
        let _ = sender.send(devices);
        wake();
    });
    overlay.microphones = Some(receiver);
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

/// The snapshot is being taken (before the bar shows).
pub fn preparing() -> bool {
    CAPTURE.with(|c| {
        c.try_borrow().is_ok_and(|c| {
            c.as_ref()
                .is_some_and(|c| matches!(c.phase, Phase::Preparing(_)))
        })
    })
}

/// Open New Capture on `target` over the display under the pointer.
/// `from_menu` is true when a tray menu click opened it, so its fade must
/// leave the screen first. `wake` runs on worker threads when the host loop
/// has work.
pub fn open(
    options: &crate::Options,
    target: Target,
    mode: MenuMode,
    direct: bool,
    from_menu: bool,
    wake: impl Fn() + Send + Sync + Clone + 'static,
) -> std::result::Result<(), String> {
    if active() {
        // `live` routes requests to an open bar (`shortcut`) first; anything
        // left is refused like shipping's `CaptureInProgress`. Bring the open
        // controls forward so Enter and Esc keep reaching them.
        focus_overlay();
        return Ok(());
    }
    let settings = captures_settings::load(&options.settings_file).unwrap_or_default();
    let flow = crate::recorder::begin_screenshot_flow().unwrap_or_else(|| CaptureFlow::begin(0))?;
    let wake: std::sync::Arc<dyn Fn() + Send + Sync> = std::sync::Arc::new(wake);
    let prepared = prepare(flow.generation(), &settings, None, from_menu, &wake);
    let tokens = crate::appearance::tokens(options.appearance.as_deref(), &settings);
    let root = options
        .history_root
        .clone()
        .unwrap_or_else(captures_app::default_history_root);
    CAPTURE.with(|c| {
        *c.borrow_mut() = Some(Capture {
            flow,
            target,
            mode,
            settings,
            tokens,
            root,
            wake,
            phase: Phase::Preparing(prepared),
            direct,
            switching: None,
            recaptured: false,
            arrive_as: None,
            preferences: None,
        })
    });
    Ok(())
}

/// Prepare `display` (else the one under the pointer) and list every display
/// on a worker, waking the host loop when done.
fn prepare(
    generation: u64,
    settings: &captures_settings::AppSettings,
    display: Option<String>,
    from_menu: bool,
    wake: &std::sync::Arc<dyn Fn() + Send + Sync>,
) -> Receiver<Prepared> {
    let (freeze, include_cursor) = (settings.freeze_screen, settings.show_cursor_in_screenshots);
    let (sender, prepared) = channel();
    let wake = wake.clone();
    std::thread::spawn(move || {
        wait_for_shell_menus(from_menu);
        let result = display
            .map_or_else(pointer_display, Ok)
            .and_then(|display| {
                match WindowSession::prepare(&display, generation, freeze, include_cursor, 0.) {
                    Ok(session) => {
                        let displays = captures_capture::XcapBackend
                            .displays()
                            .unwrap_or_else(|_| vec![session.display().clone()]);
                        Ok(Some((session, displays)))
                    }
                    Err(captures_app::Error::Cancelled) => Ok(None),
                    Err(error) => Err(error.to_string()),
                }
            });
        let _ = sender.send(result);
        wake();
    });
    prepared
}

/// The open bar's flow generation, for routing capture shortcuts to it
/// (`CaptureShortcuts::set_selector_generation`). `None` while it prepares,
/// switches, counts down, captures, or sits on a running take (that take's
/// routes keep New Capture restoring its controls).
pub fn selector_generation() -> Option<u64> {
    CAPTURE.with(|c| {
        let c = c.try_borrow().ok()?;
        let capture = c.as_ref()?;
        match &capture.phase {
            Phase::Selecting(overlay, _)
                if capture.switching.is_none() && !overlay.menu.recording_locked =>
            {
                Some(capture.flow.generation())
            }
            _ => None,
        }
    })
}

/// A capture shortcut or tray item while New Capture is open: the bar
/// switches or recaptures (`capture_error::busy_route`). False when no bar
/// is open, so the caller opens one.
pub fn shortcut(action: capture_error::Action) -> bool {
    CAPTURE.with(|c| {
        let Ok(mut c) = c.try_borrow_mut() else {
            return false;
        };
        let Some(capture) = c.as_mut() else {
            return false;
        };
        if capture.switching.is_none()
            && let Phase::Selecting(overlay, _) = &capture.phase
            && overlay.menu.recording_locked
        {
            return false;
        }
        capture.shortcut(action);
        true
    })
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
    let started = Instant::now();
    let open = shell_menus_open();
    while shell_menus_open() && started.elapsed() < SHELL_MENUS_WAIT {
        std::thread::sleep(Duration::from_millis(16));
    }
    // Menu selection fade (`SPI_GETSELECTIONFADE`) and flyout close animations
    // outlive the window's visibility.
    if from_menu || open {
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// How long to wait for the tray menu and overflow flyout to close.
pub const SHELL_MENUS_WAIT: Duration = Duration::from_millis(750);

/// A popup menu (the tray menu) or Windows 11's notification-area overflow
/// flyout is visible.
pub fn shell_menus_open() -> bool {
    const CLASSES: [PCWSTR; 3] = [
        w!("#32768"),
        w!("TopLevelWindowForOverflowXamlIsland"),
        w!("NotifyIconOverflowWindow"),
    ];
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
pub fn service() -> Option<Outcome> {
    CAPTURE.with(|cell| {
        let mut slot = cell.try_borrow_mut().ok()?;
        if matches!(slot.as_ref()?.phase, Phase::Record(_)) {
            let capture = slot.take()?;
            let Phase::Record(record) = capture.phase else {
                return None;
            };
            let (display, target) = *record;
            return Some(Outcome::Record(Box::new(Handoff {
                flow: capture.flow,
                display,
                target,
                settings: capture.settings,
                tokens: capture.tokens,
            })));
        }
        let capture = slot.as_mut()?;
        if let Err(error) = capture.finish_switch() {
            capture.flow.cancel();
            *slot = None;
            let result = Err(error.message().to_string());
            return Some(Outcome::Captured(Box::new(result)));
        }
        let finished = match &mut capture.phase {
            Phase::Preparing(prepared) => match prepared.try_recv() {
                Ok(Ok(Some((session, displays)))) => {
                    if let Err(error) = capture.show(session, displays) {
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
            Phase::Selecting(overlay, _) => {
                if let Some(receiver) = &overlay.microphones
                    && let Ok(devices) = receiver.try_recv()
                {
                    overlay.microphones = None;
                    overlay.menu.record.set_microphones(devices);
                    let _ = overlay.render(&capture.tokens);
                }
                (!capture.flow.is_current()).then_some(Ok(None))
            }
            // The countdown window's timer advances it.
            Phase::Counting(..) => None,
            Phase::Capturing(results) => match results.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some(Err("The capture worker stopped".into())),
            },
            Phase::Done(result) => Some(std::mem::replace(&mut **result, Ok(None))),
            Phase::Record(_) => None,
        };
        let preferences = capture.preferences;
        if finished.is_some() {
            *slot = None;
        }
        if let (Some(target), Some(_)) = (preferences, &finished) {
            return Some(Outcome::Preferences(target));
        }
        finished.map(|result| Outcome::Captured(Box::new(result)))
    })
}

impl Capture {
    /// Shipping `switchDisplay`: prepare the chosen display while the bar
    /// reads "Switching…", then reopen it there.
    fn switch_display(&mut self, index: usize) {
        let Phase::Selecting(overlay, _) = &mut self.phase else {
            return;
        };
        let Some((id, ..)) = overlay.menu.displays.get(index) else {
            return;
        };
        if self.switching.is_some() || index == overlay.menu.display_index {
            return;
        }
        let id = id.clone();
        overlay.menu.switching = true;
        let _ = overlay.render(&self.tokens);
        let generation = self.flow.generation();
        let prepared = prepare(generation, &self.settings, Some(id), false, &self.wake);
        self.switching = Some((prepared, None));
    }

    /// A capture shortcut while the bar is up.
    fn shortcut(&mut self, action: capture_error::Action) {
        let Phase::Selecting(overlay, session) = &mut self.phase else {
            return;
        };
        match overlay.menu.shortcut(action) {
            ShortcutEffect::Ignored => {}
            ShortcutEffect::Switched => {
                load_microphones(overlay, &self.wake);
                let _ = overlay.render(&self.tokens);
            }
            // Shipping freezes the screen with the open capture UI in it and
            // opens the bar or a direct selector on that snapshot.
            ShortcutEffect::Recapture {
                target,
                mode,
                direct,
            } if self.switching.is_none() => {
                let id = session.display().id.clone();
                overlay.menu.starting = true;
                let _ = overlay.render(&self.tokens);
                let mut settings = self.settings.clone();
                settings.freeze_screen = true;
                let generation = self.flow.generation();
                let prepared = prepare(generation, &settings, Some(id), false, &self.wake);
                self.switching = Some((prepared, Some((target, mode, direct))));
            }
            ShortcutEffect::Recapture { .. } => {}
        }
    }

    /// Show the switched or recaptured display once it is prepared. A failed
    /// switch keeps the bar as it was; a cancelled flow ends with the bar.
    fn finish_switch(&mut self) -> Result<()> {
        let Some((receiver, recapture)) = &self.switching else {
            return Ok(());
        };
        let recapture = *recapture;
        let prepared = match receiver.try_recv() {
            Err(TryRecvError::Empty) => return Ok(()),
            Ok(prepared) => prepared,
            Err(TryRecvError::Disconnected) => Err("The capture worker stopped".into()),
        };
        self.switching = None;
        match prepared {
            Ok(Some((session, displays))) => {
                self.recaptured = recapture.is_some();
                self.arrive_as = recapture;
                self.show(session, displays)?;
            }
            Ok(None) => {}
            Err(error) => {
                eprintln!("Could not prepare the capture: {error}");
                if let Phase::Selecting(overlay, _) = &mut self.phase {
                    overlay.menu.switching = false;
                    overlay.menu.starting = false;
                    let _ = overlay.render(&self.tokens);
                }
            }
        }
        Ok(())
    }

    fn show(&mut self, session: WindowSession, displays: Vec<DisplayDescriptor>) -> Result<()> {
        let display = session.display().clone();
        let displays_switched = matches!(self.phase, Phase::Selecting(..));
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
            menu.recording_locked = crate::recorder::active();
            if menu.recording_locked {
                menu.mode = MenuMode::Screenshot;
            } else {
                menu.mode = self.mode;
            }
            menu.controls_excluded = !self.settings.include_recording_controls_in_captures;
            menu.record = RecordOptions::new(&self.settings);
            menu.windows = window_rects(&session);
            menu.displays = display_options(&displays);
            menu.display_index = displays
                .iter()
                .position(|d| d.id == display.id)
                .unwrap_or_default();
            if menu.displays.is_empty() {
                menu.displays = display_options(std::slice::from_ref(&display));
            }
            // A display switch keeps the bar's target, mode, aspect and this
            // capture's Record options; the region and window pick reset. A
            // recapture lands where its shortcut routed.
            menu.direct = self.direct;
            let mut microphones = None;
            if let Phase::Selecting(previous, _) = &mut self.phase {
                let old = &mut previous.menu;
                menu.target = old.target;
                menu.mode = old.mode;
                menu.direct = old.direct;
                menu.aspect = old.aspect;
                std::mem::swap(&mut menu.record, &mut old.record);
                microphones = previous.microphones.take();
            }
            if let Some((target, mode, direct)) = self.arrive_as.take() {
                menu.target = target;
                menu.mode = mode;
                menu.direct = direct;
            }
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
                microphones,
            };
            load_microphones(&mut overlay, &self.wake);
            overlay.render(&self.tokens)?;
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
            let _ = SetFocus(Some(hwnd));
            // Dropping the previous display's overlay closes it.
            self.phase = Phase::Selecting(Box::new(overlay), Box::new(session));
        }
        // Shipping re-arms a Full screen auto-start on the new display.
        if let Phase::Selecting(overlay, _) = &self.phase
            && overlay.menu.target == Target::Display
            && overlay.menu.auto_start
            && displays_switched
        {
            self.confirm();
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
        if overlay.menu.mode == MenuMode::Recording {
            let display = session.display().clone();
            let whole = RecordingTarget::Display {
                display_id: display.id.clone(),
            };
            // Region rects stay in the overlay's display-local DIPs, which
            // the recording engine scales to the captured frame.
            let target = match (overlay.menu.target, overlay.menu.selection) {
                (Target::Region, Some(r)) => RecordingTarget::Region {
                    display_id: display.id.clone(),
                    rect: CaptureRect {
                        x: r.left.round() as i32,
                        y: r.top.round() as i32,
                        width: (r.right - r.left).round() as u32,
                        height: (r.bottom - r.top).round() as u32,
                    },
                },
                (Target::Window, _) => match overlay.menu.picked.flatten() {
                    Some(index) => match session.windows().get(index) {
                        Some(window) => RecordingTarget::Window {
                            window_id: window.id.clone(),
                        },
                        None => return,
                    },
                    None => whole,
                },
                (Target::Display, _) => whole,
                _ => return,
            };
            // This capture's options; the saved defaults stay in Preferences.
            overlay.menu.record.apply(&mut self.settings.recording);
            // Dropping the overlay closes the menu; the host hands off next.
            self.phase = Phase::Record(Box::new((display, target)));
            (self.wake)();
            return;
        }
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
            // A recaptured bar saves its snapshot of the capture UI.
            (Target::Display, _) if self.recaptured => {
                Choice::Session(window_target::Target::Display)
            }
            (Target::Display, _) => Choice::Display,
            _ => return,
        };
        let seconds = if self.recaptured {
            0
        } else {
            self.settings.screenshot_countdown_seconds
        };
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
            WM_TIMER if wp.0 == FRAME_TIMER => {
                let _ = overlay.render(&self.tokens);
            }
            WM_MOUSEMOVE => {
                let (x, y) = point(lp, overlay.scale);
                let mut changed = overlay.menu.set_shift(wp.0 & MK_SHIFT != 0);
                changed |= overlay.menu.moved(x, y);
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
                overlay.menu.set_shift(wp.0 & MK_SHIFT != 0);
                if overlay.menu.pressed(x, y) {
                    unsafe { SetCapture(hwnd) };
                    let _ = overlay.render(&self.tokens);
                }
            }
            WM_LBUTTONUP => {
                let (x, y) = point(lp, overlay.scale);
                let _ = unsafe { ReleaseCapture() };
                // Releasing a dragged bar off the panel does not pick a window.
                let outside = overlay.menu.pressed.is_none()
                    && overlay.menu.panel_drag.is_none()
                    && !overlay.menu.over_panel(x, y);
                action = if overlay.menu.target == Target::Window && outside {
                    overlay.menu.pick()
                } else {
                    overlay.menu.released(x, y)
                };
                let _ = overlay.render(&self.tokens);
            }
            WM_KEYDOWN | WM_KEYUP if VIRTUAL_KEY(wp.0 as u16) == VK_SHIFT => {
                if overlay.menu.set_shift(msg == WM_KEYDOWN) {
                    let _ = overlay.render(&self.tokens);
                }
            }
            WM_KEYDOWN if VIRTUAL_KEY(wp.0 as u16) == VK_TAB => {
                let back = unsafe { GetKeyState(VK_SHIFT.0.into()) } < 0;
                if overlay.menu.focus_step(back) {
                    let _ = overlay.render(&self.tokens);
                }
            }
            WM_KEYDOWN => {
                let vk = VIRTUAL_KEY(wp.0 as u16);
                let key = match vk {
                    VK_DOWN => Some(select::Key::ArrowDown),
                    VK_UP => Some(select::Key::ArrowUp),
                    VK_HOME => Some(select::Key::Home),
                    VK_END => Some(select::Key::End),
                    VK_RETURN => Some(select::Key::Enter),
                    VK_SPACE => Some(select::Key::Space),
                    VK_ESCAPE => Some(select::Key::Escape),
                    _ => None,
                };
                if key.is_some_and(|key| overlay.menu.listbox_key(key)) {
                    let switch = overlay.menu.switch_to.take();
                    let _ = overlay.render(&self.tokens);
                    if let Some(index) = switch {
                        self.switch_display(index);
                    }
                    return Some(LRESULT(0));
                }
                match vk {
                    VK_RETURN | VK_SPACE => {
                        action = overlay.menu.focus_key(vk == VK_RETURN);
                        let _ = overlay.render(&self.tokens);
                    }
                    VK_ESCAPE => action = Some(Hit::Close),
                    _ => {}
                }
            }
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
                    overlay.menu.region_cursor(x, y)
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
        if let Some(hit) = action {
            self.run(hit);
        }
        Some(LRESULT(0))
    }

    /// Run a control's action: a click, a key, or UI Automation.
    fn run(&mut self, action: Hit) {
        let Phase::Selecting(overlay, _) = &mut self.phase else {
            return;
        };
        match action {
            Hit::Close => self.cancel(),
            Hit::Primary => self.confirm(),
            // Shipping `openCapturePreference`: open Preferences, then cancel.
            Hit::Preferences(target) => {
                self.preferences = Some(target);
                self.cancel();
            }
            // Shipping's window target `onClick`: pick it, then auto-start.
            Hit::Pick(index) => {
                if overlay.menu.starting
                    || overlay.menu.switching
                    || index >= overlay.menu.windows.len()
                {
                    return;
                }
                overlay.menu.picked = Some(Some(index));
                overlay.menu.hovered = Some(Some(index));
                let auto = overlay.menu.auto_start;
                let _ = overlay.render(&self.tokens);
                if auto {
                    self.confirm();
                }
            }
            Hit::Target(target) => {
                overlay.menu.target = target;
                overlay.menu.open = None;
                let auto = target == Target::Display && overlay.menu.auto_start;
                let _ = overlay.render(&self.tokens);
                if auto {
                    self.confirm();
                }
            }
            hit @ (Hit::Screenshot | Hit::Record) => {
                overlay.menu.mode = if hit == Hit::Record {
                    MenuMode::Recording
                } else {
                    MenuMode::Screenshot
                };
                overlay.menu.open = None;
                load_microphones(overlay, &self.wake);
                let _ = overlay.render(&self.tokens);
            }
            hit @ (Hit::Toggle(_) | Hit::Select(_) | Hit::Option(_)) => {
                overlay.menu.option_action(hit);
                let switch = overlay.menu.switch_to.take();
                let _ = overlay.render(&self.tokens);
                if let Some(index) = switch {
                    self.switch_display(index);
                }
            }
        }
    }

    /// UI Automation focus: the control takes keyboard focus with its ring.
    fn focus(&mut self, hit: Hit) {
        if let Phase::Selecting(overlay, _) = &mut self.phase {
            overlay.menu.set_focus(hit, true);
            let _ = overlay.render(&self.tokens);
        }
    }
}

/// The open bar's accessible state for `panel_uia`.
pub fn uia_snapshot() -> Option<crate::panel_uia::Snapshot> {
    CAPTURE.with(|c| {
        let c = c.try_borrow().ok()?;
        let Phase::Selecting(overlay, _) = &c.as_ref()?.phase else {
            return None;
        };
        Some(crate::panel_uia::Snapshot {
            hwnd: overlay.hwnd,
            scale: overlay.scale,
            size: overlay.menu.size,
            name: overlay.menu.uia_name(),
            focusable: true,
            items: overlay.menu.uia_items(),
        })
    })
}

/// Apply a UI Automation request through the same paths as clicks and keys.
pub fn uia_act(key: u32, act: crate::panel_uia::Act) -> Option<()> {
    use crate::panel_uia::Act;
    let hit = uia_hit(key)?;
    CAPTURE.with(|c| {
        let mut c = c.try_borrow_mut().ok()?;
        let capture = c.as_mut()?;
        match act {
            Act::Focus => capture.focus(hit),
            Act::Invoke | Act::Select => capture.run(hit),
            Act::SetValue(_) => return None,
        }
        Some(())
    })
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
        menu.clock = crate::tooltip::now_ms();
        menu.reduced = crate::appearance::reduced_motion();
        gfx::draw_surface(surface, *px, (*scale, *scale), 0., |dc| {
            menu.paint(dc, g, t, frozen.as_ref())
        })?;
        // Repaint at about 60 fps only while a motion runs.
        unsafe {
            if menu.animating(t) {
                SetTimer(Some(self.hwnd), FRAME_TIMER, 16, None);
            } else {
                let _ = KillTimer(Some(self.hwnd), FRAME_TIMER);
            }
            g.dcomp.as_ref().ok_or_else(Error::empty)?.Commit()
        }
    }
}

/// The overlay's motion frame timer.
const FRAME_TIMER: usize = 1;

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    use crate::panel_uia::{self, Window};
    use windows::Win32::UI::Accessibility::{UiaReturnRawElementProvider, UiaRootObjectId};
    // The bar's overlay answers UI Automation (the countdown does not).
    if msg == WM_GETOBJECT
        && lp.0 as i32 == UiaRootObjectId
        && uia_snapshot().is_some_and(|s| s.hwnd == hwnd)
    {
        let root = panel_uia::node(Window::Capture, None);
        return unsafe { UiaReturnRawElementProvider(hwnd, wp, lp, &root) };
    }
    if msg == WM_DESTROY {
        unsafe {
            let _ = UiaReturnRawElementProvider(hwnd, WPARAM(0), LPARAM(0), None);
        }
    }
    let before = panel_uia::watch(Window::Capture);
    let handled = CAPTURE.with(|c| {
        let mut c = c.try_borrow_mut().ok()?;
        c.as_mut()?.handle(hwnd, msg, wp, lp)
    });
    if handled.is_some() {
        panel_uia::raise(Window::Capture, before);
    }
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
    fn record_options_couple_pointer_toggles_and_apply_only_to_the_take() {
        let mut saved = captures_settings::AppSettings::default();
        saved.recording.video_fps = 30;
        saved.recording.microphone_device_id = Some("usb".into());
        let mut options = RecordOptions::new(&saved);
        options.capabilities.cursor_control = true;
        options.capabilities.click_highlights = true;
        options.capabilities.system_audio = false;
        options.show_cursor = false;
        // Clicks need a visible cursor; hiding the cursor drops clicks.
        options.toggle(RecordingToggle::ShowClicks);
        assert!(options.show_cursor && options.highlight_clicks);
        options.toggle(RecordingToggle::ShowCursor);
        assert!(!options.show_cursor && !options.highlight_clicks);
        // An unavailable switch stays off.
        options.toggle(RecordingToggle::DesktopAudio);
        assert!(!options.system_audio);
        // The saved microphone is listed until devices load; FPS picks by index.
        assert_eq!(
            options.value(Field::Microphone),
            capture_menu::SELECTED_MICROPHONE
        );
        options.choose(Field::Fps, 2);
        let mut take = saved.recording.clone();
        options.apply(&mut take);
        assert_eq!(take.video_fps, 15);
        assert_eq!(take.microphone_device_id.as_deref(), Some("usb"));
        assert!(!take.capture_system_audio);
        assert_eq!(saved.recording.video_fps, 30);
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

    /// Lay the bar out as a paint would.
    fn lay_out(m: &mut Menu) {
        let t = Tokens::variant("dark", "mustard");
        let g = Gfx::new(&t, false).unwrap();
        m.layout = Some(m.layout(&g, &t).unwrap());
    }

    #[test]
    fn tab_walks_the_bar_and_keys_activate_like_shipping() {
        let mut m = menu(Target::Region, false);
        m.selection = Some(rect(100., 100., 400., 300.));
        lay_out(&mut m);
        // No windows listed: the disabled Window segment is skipped.
        let order = vec![
            Hit::Close,
            Hit::Screenshot,
            Hit::Record,
            Hit::Target(Target::Region),
            Hit::Target(Target::Display),
            Hit::Select(Field::Aspect),
            Hit::Primary,
            Hit::Preferences(PreferenceTarget::IncludeRecordingControlsInCaptures),
        ];
        assert_eq!(m.focus_order(), order);
        // Shift+Tab from nothing lands on the last control, the note's
        // Preferences link, which Enter follows; Tab wraps.
        assert!(m.focus_step(true));
        assert_eq!((m.focus, m.focus_visible), (Some(order[7]), true));
        assert_eq!(m.focus_key(true), Some(order[7]));
        m.focus_step(false);
        assert_eq!(m.focus, Some(Hit::Close));
        // Enter on Close closes; on a segment it confirms (shipping's
        // capture-phase handler) while Space switches.
        assert_eq!(m.focus_key(true), Some(Hit::Close));
        m.set_focus(Hit::Target(Target::Display), true);
        assert_eq!(m.focus_key(true), Some(Hit::Primary));
        assert_eq!(m.focus_key(false), Some(Hit::Target(Target::Display)));
        // A focused select opens on an arrow; Tab closes it and moves on.
        m.set_focus(Hit::Select(Field::Aspect), true);
        assert!(m.listbox_key(select::Key::ArrowDown));
        assert_eq!(m.open.map(|(f, _)| f), Some(Field::Aspect));
        m.focus_step(false);
        assert_eq!((m.open, m.focus), (None, Some(Hit::Primary)));
        // Escape on a closed select is left for the bar (it closes).
        m.set_focus(Hit::Select(Field::Aspect), true);
        assert!(!m.listbox_key(select::Key::Escape));
        // A click focuses without the ring; an empty click blurs.
        m.layout = None;
        lay_out(&mut m);
        let close = m.layout.as_ref().unwrap().close;
        m.pressed(close.left + 4., close.top + 4.);
        assert_eq!((m.focus, m.focus_visible), (Some(Hit::Close), false));
        m.released(close.left + 4., close.top + 4.);
        m.pressed(5., 5.);
        assert_eq!(m.focus, None);
        m.released(5., 5.);
        // Clicking the note's link follows it.
        let (link, ..) = m.layout.as_ref().unwrap().links[0];
        let (x, y) = (link.right - 3., (link.top + link.bottom) / 2.);
        m.pressed(x, y);
        assert_eq!(m.released(x, y), Some(order[7]));
        // With auto-start the confirm note is a second link.
        m.auto_start = true;
        lay_out(&mut m);
        let links: Vec<Hit> = m
            .layout
            .as_ref()
            .unwrap()
            .links
            .iter()
            .map(|l| l.2)
            .collect();
        assert_eq!(
            links,
            [
                order[7],
                Hit::Preferences(PreferenceTarget::AutoStartOnSelection)
            ]
        );
        m.auto_start = false;

        // Record: Space flips a switch, Enter on it does nothing.
        m.mode = MenuMode::Recording;
        lay_out(&mut m);
        let cursor = Hit::Toggle(RecordingToggle::ShowCursor);
        assert!(m.focus_order().contains(&cursor));
        m.set_focus(cursor, true);
        assert_eq!(m.focus_key(false), Some(cursor));
        assert_eq!(m.focus_key(true), None);

        // Window: the targets come first and light when focused.
        let mut m = menu(Target::Window, false);
        m.windows = vec![(rect(0., 0., 100., 100.), "Notes".into())];
        lay_out(&mut m);
        assert_eq!(m.focus_order()[..2], [Hit::Pick(0), Hit::Close]);
        m.focus_step(false);
        assert_eq!(m.hovered, Some(Some(0)));
        assert_eq!(m.focus_key(true), Some(Hit::Pick(0)));
    }

    #[test]
    fn the_bars_blank_glass_drags_it_within_the_display() {
        let mut m = menu(Target::Window, false);
        m.windows = vec![(rect(0., 0., 100., 100.), "Notes".into())];
        lay_out(&mut m);
        let p = m.layout.as_ref().unwrap().panel;
        let (pw, ph) = (p.right - p.left, p.bottom - p.top);
        // A button does not drag; the glass left of the note does.
        let close = m.layout.as_ref().unwrap().close;
        m.pressed(close.left + 4., close.top + 4.);
        assert_eq!(m.panel_drag, None);
        m.released(close.left + 4., close.top + 4.);
        let grip = (p.left + 6., p.bottom - 6.);
        assert!(m.pressed(grip.0, grip.1));
        m.moved(grip.0 - 100., grip.1 - 200.);
        lay_out(&mut m);
        let moved = m.layout.as_ref().unwrap().panel;
        assert_eq!((moved.left, moved.top), (p.left - 100., p.top - 200.));
        // Dragged past the corner it stops 8 px inside, and the release is
        // not a window pick or a control action.
        m.moved(-500., 2000.);
        assert_eq!(m.panel_at, Some((8., 800. - ph - 8.)));
        assert_eq!(m.released(-500., 2000.), None);
        assert_eq!((m.picked, m.panel_drag), (None, None));
        // It stays put, top-anchored, when Record grows the bar.
        m.mode = MenuMode::Recording;
        lay_out(&mut m);
        let grown = m.layout.as_ref().unwrap().panel;
        assert_eq!((grown.left, grown.top), (8., 800. - ph - 8.));
        assert!(grown.right - grown.left >= pw);
    }

    #[test]
    fn ui_automation_lists_the_bar_with_shipping_roles_and_names() {
        use crate::panel_uia::Role;
        let mut m = menu(Target::Region, false);
        m.mode = MenuMode::Recording;
        lay_out(&mut m);
        let items = m.uia_items();
        let names: Vec<(Role, &str)> = items.iter().map(|i| (i.role, i.name.as_str())).collect();
        assert_eq!(
            names,
            vec![
                (Role::Status, "Drag to select a region"),
                (Role::Button, "Close capture controls"),
                (Role::Group, "Capture type"),
                (Role::Group, "Capture target"),
                (Role::Toggle, "Screenshot"),
                (Role::Toggle, "Record"),
                (Role::Toggle, "Region"),
                (Role::Toggle, "Window"),
                (Role::Toggle, "Full screen"),
                (Role::ComboBox, "Region aspect ratio"),
                (Role::Button, "Start recording"),
                (Role::ComboBox, "Frames per second"),
                (Role::ComboBox, "Maximum resolution"),
                (Role::Check, "Show cursor"),
                (Role::Check, "Show clicks"),
                (Role::Check, "Record desktop audio"),
                (Role::ComboBox, "Microphone"),
                (Role::Button, "These controls won’t show in recordings"),
                (Role::Text, "Press Enter to confirm"),
            ]
        );
        let find = |items: &[crate::panel_uia::Item], name: &str| {
            items.iter().find(|i| i.name == name).cloned().unwrap()
        };
        // Pressed segments sit in their groups; Window is disabled with
        // shipping's reason.
        let record = find(&items, "Record");
        assert_eq!(
            (record.parent, record.selected),
            (Some(UIA_TYPE_GROUP), true)
        );
        assert!(!find(&items, "Screenshot").selected);
        let window = find(&items, "Window");
        assert!(!window.enabled);
        assert_eq!(
            window.help,
            "Window capture is not available in this desktop session"
        );
        assert_eq!(
            find(&items, "Frames per second").value.as_deref(),
            Some("60")
        );
        // Keys round-trip to the controls they act on.
        for item in &items {
            if let Some(hit) = uia_hit(item.key) {
                assert_eq!(uia_key(hit), item.key, "{}", item.name);
            }
        }

        // An open listbox: the combobox is expanded and the active option
        // has focus.
        m.set_focus(Hit::Select(Field::Fps), true);
        m.listbox_key(select::Key::ArrowDown);
        m.listbox_key(select::Key::ArrowDown);
        lay_out(&mut m);
        let items = m.uia_items();
        let fps = find(&items, "Frames per second");
        assert!(fps.selected && !fps.focused);
        let options: Vec<_> = items.iter().filter(|i| i.role == Role::Option).collect();
        let named: Vec<_> = options.iter().map(|o| o.name.as_str()).collect();
        assert_eq!(named, ["60", "30", "15"]);
        assert!(options[0].selected && !options[0].focused);
        assert!(options[1].focused && options[1].parent == Some(UIA_LISTBOX));

        // The direct selector is its chip alone, named like AppKit's.
        let mut m = menu(Target::Window, false);
        m.direct = true;
        lay_out(&mut m);
        assert_eq!(m.uia_name(), capture_menu::WINDOW_SELECTOR_LABEL);
        let items = m.uia_items();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "Select a window to continue");
    }

    fn edges(m: &Menu) -> (f32, f32, f32, f32) {
        let r = m.selection.expect("selection");
        (r.left, r.top, r.right, r.bottom)
    }

    #[test]
    fn a_drawn_region_moves_resizes_and_redraws_like_shipping() {
        let mut m = menu(Target::Region, true);
        m.pressed(100., 100.);
        m.moved(300., 200.);
        assert_eq!(m.released(300., 200.), Some(Hit::Primary));
        m.auto_start = false;
        // Inside the frame drags it, clamped to the display; no auto-start.
        assert!(m.pressed(150., 150.));
        m.moved(1250., 150.);
        assert_eq!(m.released(1250., 150.), None);
        assert_eq!(edges(&m), (1080., 100., 1280., 200.));
        // A corner handle (within its reach) resizes from the opposite corner.
        m.pressed(1083., 104.);
        m.moved(1000., 50.);
        m.released(1000., 50.);
        assert_eq!(edges(&m), (997., 46., 1280., 200.));
        // Outside the frame starts over.
        m.pressed(10., 700.);
        m.moved(60., 780.);
        m.released(60., 780.);
        assert_eq!(edges(&m), (10., 700., 60., 780.));
    }

    #[test]
    fn auto_start_ignores_move_and_resize() {
        let mut m = menu(Target::Region, true);
        m.selection = Some(rect(100., 100., 300., 200.));
        m.pressed(200., 150.);
        m.moved(220., 160.);
        assert_eq!(m.released(220., 160.), None);
        m.pressed(320., 210.);
        m.moved(340., 260.);
        assert_eq!(m.released(340., 260.), None);
        assert_eq!(edges(&m), (120., 110., 340., 260.));
    }

    #[test]
    fn shift_squares_a_drag_until_it_is_released() {
        let mut m = menu(Target::Region, false);
        m.pressed(100., 100.);
        m.moved(400., 200.);
        assert_eq!(edges(&m), (100., 100., 400., 200.));
        // Pressing Shift mid-drag re-runs it at once, without a move.
        assert!(m.set_shift(true));
        assert_eq!(edges(&m), (100., 100., 400., 400.));
        assert!(!m.set_shift(true));
        assert!(m.set_shift(false));
        assert_eq!(edges(&m), (100., 100., 400., 200.));
        m.released(400., 200.);
        // Outside a drag Shift changes nothing.
        assert!(!m.set_shift(true));
        assert_eq!(edges(&m), (100., 100., 400., 200.));
    }

    #[test]
    fn aspect_locks_drags_and_snaps_a_settled_region() {
        let mut m = menu(Target::Region, false);
        m.selection = Some(rect(100., 100., 500., 200.));
        // 1 : 1 refits around the center inside the old box.
        m.set_aspect(1);
        assert_eq!(edges(&m), (250., 100., 350., 200.));
        // 16 : 9 locks a new drag; Shift still forces a square over it.
        m.set_aspect(4);
        m.pressed(0., 0.);
        m.moved(320., 10.);
        assert_eq!(edges(&m), (0., 0., 320., 180.));
        m.set_shift(true);
        assert_eq!(edges(&m), (0., 0., 320., 320.));
        m.released(320., 10.);
        // Free leaves the region alone.
        m.set_aspect(0);
        assert_eq!(edges(&m), (0., 0., 320., 320.));
        let (labels, _, selected) = m.entries(Field::Aspect);
        assert_eq!((labels[4].as_str(), selected), ("16 : 9", 0));
    }

    #[test]
    fn full_screen_starts_without_a_selection() {
        assert!(menu(Target::Display, false).can_start());
    }

    #[test]
    fn display_select_lists_displays_and_switches_once() {
        let display = |id: &str, name: &str, primary| DisplayDescriptor {
            id: id.into(),
            name: name.into(),
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
            scale_factor: 1.,
            is_primary: primary,
        };
        let options = display_options(&[display("a", "  ", false), display("b", "DELL", true)]);
        assert_eq!(
            options,
            [
                ("a".into(), "Display 1".into(), "1920 × 1080".into()),
                ("b".into(), "DELL".into(), "1920 × 1080 · Primary".into()),
            ]
        );
        let mut m = menu(Target::Display, false);
        // One display: shipping disables the select.
        assert!(!m.field_enabled(Field::Display));
        m.displays = options;
        assert!(m.field_enabled(Field::Display));
        // Choosing the shown display does nothing; another asks to switch.
        m.choose(Field::Display, 0);
        assert_eq!(m.switch_to.take(), None);
        m.choose(Field::Display, 1);
        assert_eq!(m.switch_to.take(), Some(1));
        // While switching, the select and the primary are disabled.
        m.switching = true;
        assert!(!m.field_enabled(Field::Display) && !m.can_start());
        m.choose(Field::Display, 1);
        assert_eq!(m.switch_to, None);
    }

    #[test]
    fn bar_motions_play_on_mount_and_settle() {
        let t = Tokens::variant("dark", "mustard");
        let mut m = menu(Target::Region, false);
        m.reduced = false;
        m.clock = 1_000.;
        m.sync_motion(&t);
        let chip = |m: &Menu| m.guidance.as_ref().unwrap().1.pose(m.clock);
        // The chip mounts transparent, 6 px high, and settles.
        assert_eq!(chip(&m).opacity, 0.);
        assert!(m.animating(&t));
        m.clock = 2_000.;
        assert_eq!((chip(&m).opacity, chip(&m).translate_y), (1., 0.));
        assert!(!m.animating(&t));
        // Drawing a region fades it out; Window mounts a new chip.
        m.pressed(100., 100.);
        m.sync_motion(&t);
        m.clock = 3_000.;
        assert_eq!(chip(&m).opacity, 0.);
        m.released(300., 300.);
        m.target = Target::Window;
        m.sync_motion(&t);
        assert_eq!(chip(&m).opacity, 0.);
        // Record's options row arrives after its 40 ms delay, and the ready
        // ping keeps the frame timer running once a region can start.
        m.target = Target::Region;
        m.mode = MenuMode::Recording;
        m.sync_motion(&t);
        assert_eq!(m.options_pose(&t).opacity, 0.);
        m.clock = 3_020.;
        assert_eq!(m.options_pose(&t).translate_y, -5.);
        m.clock = 5_000.;
        assert_eq!(m.options_pose(&t), Pose::REST);
        assert!(m.can_start() && m.animating(&t));
        // Reduced motion draws everything at rest at once.
        m.reduced = true;
        m.mode = MenuMode::Screenshot;
        m.sync_motion(&t);
        m.mode = MenuMode::Recording;
        m.sync_motion(&t);
        assert_eq!(m.options_pose(&t), Pose::REST);
        assert!(!m.animating(&t));
    }

    #[test]
    fn shortcuts_switch_the_open_bar_or_recapture_it() {
        use capture_error::{Action, Target as T};
        let mut m = menu(Target::Window, false);
        m.windows = vec![(rect(0., 0., 100., 100.), "Notes".into())];
        m.picked = Some(Some(0));
        m.hovered = Some(Some(0));
        let recapture = |target, mode, direct| ShortcutEffect::Recapture {
            target,
            mode,
            direct,
        };
        // The same screenshot target again freezes the bar into that
        // target's bar-less selector.
        assert_eq!(
            m.shortcut(Action::Screenshot(T::Window)),
            recapture(Target::Window, MenuMode::Screenshot, true)
        );
        // Another mode or target switches in place; Window keeps its pick,
        // others drop it and the hover.
        assert_eq!(
            m.shortcut(Action::Record(T::Window)),
            ShortcutEffect::Switched
        );
        assert_eq!((m.mode, m.target), (MenuMode::Recording, Target::Window));
        assert_eq!((m.picked, m.hovered), (Some(Some(0)), None));
        // In Record, the Screenshot chord for the same target switches back.
        assert_eq!(
            m.shortcut(Action::Screenshot(T::Display)),
            ShortcutEffect::Switched
        );
        assert_eq!((m.mode, m.target), (MenuMode::Screenshot, Target::Display));
        assert_eq!(m.picked, None);
        // New Capture switches Full screen to Region, then recaptures it.
        assert_eq!(m.shortcut(Action::NewCapture), ShortcutEffect::Switched);
        assert_eq!(m.target, Target::Region);
        assert_eq!(
            m.shortcut(Action::NewCapture),
            recapture(Target::Region, MenuMode::Screenshot, false)
        );
        // On a direct selector every chord recaptures (shipping's selector
        // routes): Record into the bar in Record, Region into a selector.
        m.direct = true;
        assert_eq!(
            m.shortcut(Action::Record(T::Display)),
            recapture(Target::Display, MenuMode::Recording, false)
        );
        assert_eq!(
            m.shortcut(Action::Screenshot(T::Region)),
            recapture(Target::Region, MenuMode::Screenshot, true)
        );
        m.direct = false;
        // A running take's bar keeps Record disabled; nothing routes while
        // it recaptures or switches displays.
        m.recording_locked = true;
        assert_eq!(
            m.shortcut(Action::Record(T::Region)),
            ShortcutEffect::Ignored
        );
        m.recording_locked = false;
        // With no windows listed, Window is disabled for shortcuts too.
        m.windows.clear();
        assert_eq!(
            m.shortcut(Action::Record(T::Window)),
            ShortcutEffect::Ignored
        );
        m.starting = true;
        assert_eq!(
            m.shortcut(Action::Screenshot(T::Window)),
            ShortcutEffect::Ignored
        );
        assert_eq!(m.target, Target::Region);
    }

    #[test]
    fn direct_selectors_commit_at_once_and_nudge_an_empty_click() {
        let t = Tokens::variant("dark", "mustard");
        let mut m = menu(Target::Region, false);
        m.direct = true;
        m.reduced = false;
        m.sync_motion(&t);
        // No bar to hit.
        assert_eq!(m.hit(640., 760.), None);
        assert!(!m.over_panel(640., 760.));
        // A click without a drag commits nothing and nudges the chip with
        // the feedback copy for 1.8 s.
        m.pressed(200., 200.);
        assert_eq!(m.released(201., 200.), None);
        assert_eq!(m.selection, None);
        m.clock = 100.;
        m.sync_motion(&t);
        let chip = |m: &Menu| m.guidance.as_ref().unwrap().1;
        assert!(chip(&m).feedback() && m.feedback.0 == 1);
        assert!(chip(&m).pose(150.).translate_x != 0.);
        m.clock = 1_950.;
        m.sync_motion(&t);
        assert!(!chip(&m).feedback() && m.feedback == (0, None));
        // A drag commits on release even without auto-start.
        m.pressed(100., 100.);
        assert_eq!(m.released(300., 250.), Some(Hit::Primary));
        // The Window selector commits on click.
        let mut m = menu(Target::Window, false);
        m.direct = true;
        m.hover_window(Some(Some(0)));
        assert_eq!(m.pick(), Some(Hit::Primary));
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
