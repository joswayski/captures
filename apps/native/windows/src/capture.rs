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
//! `recorder`; the Aspect and Display selects, keyboard focus through the
//! bar and UI Automation are not ported yet.

use std::cell::RefCell;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::{Duration, Instant};

use captures_app::capture_flow::CaptureFlow;
use captures_app::capture_menu::{self, MenuMode, PrimaryState, RecordingToggle};
use captures_app::controls::select;
use captures_app::selection::Point;
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
}

/// The Record options row's selects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Fps,
    Resolution,
    Microphone,
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
    primary: Option<(Rect, gfx::Text, bool)>,
    note: Vec<(gfx::Text, f32, bool)>,
    note_y: f32,
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
    pub record: RecordOptions,
    /// A recording is running, so Record stays disabled (the recording
    /// controls' Screenshot opens the bar on its take).
    pub recording_locked: bool,
    /// The open listbox and its highlighted row.
    pub open: Option<(Field, select::State)>,
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
            mode: MenuMode::Screenshot,
            controls_excluded: true,
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
            record: RecordOptions::new(&captures_settings::AppSettings::default()),
            recording_locked: false,
            open: None,
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
                true,
                target == self.target,
            ));
        }
        let box_width = |items: &[(gfx::Text, Option<&str>, Hit, bool, bool)]| {
            8. + items.iter().map(|i| width(&i.0)).sum::<f32>()
        };
        let (aw, tw) = (box_width(&action), box_width(&targets));
        let primary =
            capture_menu::primary_action(self.mode, self.auto_start, PrimaryState::default());
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
        let options = if record {
            Some(self.options_layout(g, t, left, top + 1. + 56.)?)
        } else {
            None
        };
        let listbox = match (self.open, &options) {
            (Some((field, _)), Some((_, fields))) => {
                match fields.iter().find(|f| f.hit == Hit::Select(field)) {
                    Some(f) => Some(self.listbox_layout(g, t, field, f.control)?),
                    None => None,
                }
            }
            _ => None,
        };
        Ok(Layout {
            panel,
            options,
            listbox,
            close,
            switches: [action_box, target_box],
            divider,
            segments,
            primary,
            note: note
                .into_iter()
                .map(|(text, nx, strong)| (text, left + (pw - note_w) / 2. + nx, strong))
                .collect(),
            note_y: top + 1. + 56. + options_h + 10.,
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
        let (labels, disabled, selected) = self.record.entries(field);
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
        let row_h = texts
            .iter()
            .map(|text| text.height + 2. + 12.)
            .fold(28_f32, f32::max);
        let label_w = texts.iter().map(|text| text.width).fold(0., f32::max);
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
            check,
        })
    }

    fn hit(&self, x: f32, y: f32) -> Option<Hit> {
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
        if let Some((_, fields)) = &layout.options
            && let Some(field) = fields
                .iter()
                .find(|f| f.enabled && contains(&f.control, x, y))
        {
            return Some(field.hit);
        }
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

    /// Over the bar or its open listbox.
    fn over_panel(&self, x: f32, y: f32) -> bool {
        self.layout.as_ref().is_some_and(|l| {
            contains(&l.panel, x, y) || l.listbox.as_ref().is_some_and(|b| contains(&b.rect, x, y))
        })
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
            if self.mode == MenuMode::Recording {
                // `.recording-start .capture-record-dot` on the accent.
                let dot = rect(x + 2., cy - 6., x + 14., cy + 6.);
                gfx::fill_round(dc, dot, 6., t.color_alpha("theme-signal", opacity))?;
                gfx::border(dc, dot, 6., 1.5, alpha(ink, 0.6))?;
            } else {
                gfx::icon(dc, &g.factory, "capture", &[], (x, cy - 8., 16.), 1.8, ink)?;
            }
            gfx::text(dc, label, x + 22., cy - label.height / 2., ink)?;
        }
        if let Some((row_top, fields)) = &layout.options {
            self.paint_options(dc, g, t, &layout.panel, *row_top, fields)?;
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
        if let Some(listbox) = &layout.listbox {
            self.paint_listbox(dc, t, listbox)?;
        }
        self.layout = Some(layout);
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
                    let edge = if open {
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
                gfx::text(dc, label, row.left + 8., cy - label.height / 2., color)?;
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
        let mut changed = false;
        // Pointing at an enabled row highlights it (`onPointerEnter`).
        if let (Some(Hit::Option(index)), Some((field, state))) = (hover, self.open.as_mut())
            && !self.record.entries(*field).1[index]
            && state.active != index
        {
            state.active = index;
            changed = true;
        }
        std::mem::replace(&mut self.hover, hover) != hover || changed
    }

    fn pressed(&mut self, x: f32, y: f32) -> bool {
        self.pressed = self.hit(x, y);
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
        if closed && self.pressed.is_none() && self.over_panel(x, y) {
            return true;
        }
        if self.pressed.is_none() && self.target == Target::Region && !self.over_panel(x, y) {
            self.drag = Some((x, y));
            self.selection = None;
            return true;
        }
        closed
    }

    /// Run a click on an options-row control or listbox row.
    fn option_action(&mut self, hit: Hit) {
        match hit {
            Hit::Toggle(toggle) => self.record.toggle(toggle),
            Hit::Select(field) if self.open.is_some_and(|(o, _)| o == field) => self.open = None,
            Hit::Select(field) if self.record.enabled(field) => {
                let (_, disabled, selected) = self.record.entries(field);
                self.open = Some((field, select::open(&disabled, selected)));
            }
            // A disabled row leaves the listbox open, like shipping.
            Hit::Option(index) => {
                if let Some((field, _)) = self.open
                    && !self
                        .record
                        .entries(field)
                        .1
                        .get(index)
                        .copied()
                        .unwrap_or(true)
                {
                    self.record.choose(field, index);
                    self.open = None;
                }
            }
            _ => {}
        }
    }

    /// Keys for the open listbox; true when consumed.
    fn listbox_key(&mut self, key: select::Key) -> bool {
        let Some((field, state)) = self.open else {
            return false;
        };
        let (_, disabled, selected) = self.record.entries(field);
        let outcome = select::key(state, &disabled, selected, key);
        if let Some(index) = outcome.chosen {
            self.record.choose(field, index);
        }
        self.open = outcome.state.open.then_some((field, outcome.state));
        outcome.handled
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

pub type CaptureResult = std::result::Result<Option<captures_app::Artifact>, String>;

/// What a finished New Capture leaves for the host.
pub enum Outcome {
    Captured(Box<CaptureResult>),
    /// Record was confirmed: the recorder takes over the flow and its Escape.
    Record(Box<Handoff>),
}

pub struct Handoff {
    pub flow: CaptureFlow,
    pub display: DisplayDescriptor,
    pub target: RecordingTarget,
    pub settings: captures_settings::AppSettings,
    pub tokens: Tokens,
}

enum Phase {
    Preparing(Receiver<std::result::Result<Option<WindowSession>, String>>),
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
            mode,
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
        if finished.is_some() {
            *slot = None;
        }
        finished.map(|result| Outcome::Captured(Box::new(result)))
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
            menu.recording_locked = crate::recorder::active();
            if menu.recording_locked {
                menu.mode = MenuMode::Screenshot;
            } else {
                menu.mode = self.mode;
            }
            menu.controls_excluded = !self.settings.include_recording_controls_in_captures;
            menu.record = RecordOptions::new(&self.settings);
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
                microphones: None,
            };
            load_microphones(&mut overlay, &self.wake);
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
                if overlay.menu.open.is_some()
                    && key.is_some_and(|key| overlay.menu.listbox_key(key))
                {
                    let _ = overlay.render(&self.tokens);
                    return Some(LRESULT(0));
                }
                match vk {
                    VK_RETURN => action = Some(Hit::Primary),
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
            Some(hit @ (Hit::Screenshot | Hit::Record)) => {
                overlay.menu.mode = if hit == Hit::Record {
                    MenuMode::Recording
                } else {
                    MenuMode::Screenshot
                };
                overlay.menu.open = None;
                load_microphones(overlay, &self.wake);
                let _ = overlay.render(&self.tokens);
            }
            Some(hit @ (Hit::Toggle(_) | Hit::Select(_) | Hit::Option(_))) => {
                overlay.menu.option_action(hit);
                let _ = overlay.render(&self.tokens);
            }
            None => {}
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
