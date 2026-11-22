//! Shipping's `.screenshot-export-options` panel and the save row's Export
//! settings disclosure, filename input, format select and Change…: Output
//! size (Custom adds width × height and the aspect lock), Save quality, the
//! Compress preset, Maximum file size with its unit, and Est. size.
//! `editor_export` owns every label, rule and the estimate, as for GTK's
//! `export_bar.rs`; this module owns the values shipping keeps only in the
//! UI (the custom size, its lock, the size-limit text and unit) and the
//! panel's layout and painting.
use captures_app::editor_export::{self, DEFAULT_MAX_SIZE_BYTES, EstimateState, FileSizeUnit};
use captures_app::editor_session::{ExportFormat, ExportQuality, ExportSize};
use windows::Win32::Graphics::Direct2D::*;
use windows::core::*;

use super::menu::MenuKind;
use super::props::{Num, Prop};
use super::{Editor, Event, Layer, Target};
use crate::gfx::{self, Style, rect};
use crate::scene::Rect;

/// `MAX_SCREENSHOT_OUTPUT_DIMENSION`.
pub const MAX_DIMENSION: f64 = 16_384.;
/// `.screenshot-export-size-control .custom-select { min-width: 108px }`.
const SIZE_SELECT_W: f32 = 108.;
/// `.screenshot-custom-dimensions { width: 268px }` and its 32 px lock.
const CUSTOM_W: f32 = 268.;
const LOCK_W: f32 = 32.;
/// `.screenshot-maximum-size .custom-select { width: 72px }`.
const UNIT_W: f32 = 72.;

/// An export control drawn and focused like a property control.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ex {
    /// Output size, Save quality, the Compress preset, the size limit's
    /// unit, and the filename's format select.
    Size,
    Quality,
    Compress,
    Unit,
    Format,
    /// Lock output aspect ratio.
    Lock,
    /// Comparison's Show before / after, while the comparison is hidden.
    Compare,
}

impl Ex {
    pub fn is_select(self) -> bool {
        !matches!(self, Ex::Lock | Ex::Compare)
    }
}

/// One `.screenshot-export-control`, in DOM order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    Size,
    Custom,
    Quality,
    Compress,
    Maximum,
    Estimate,
    Compare,
}

pub const ITEMS: [Item; 7] = [
    Item::Size,
    Item::Custom,
    Item::Quality,
    Item::Compress,
    Item::Maximum,
    Item::Estimate,
    Item::Compare,
];

/// The export state shipping keeps in the screenshot editor's UI.
pub struct ExportUi {
    /// `exportSettingsOpen`.
    pub open: bool,
    /// `customExportWidth` and `customExportHeight`.
    pub custom: (u32, u32),
    /// `exportAspectLocked`.
    pub locked: bool,
    /// `maximumFileSizeUnit` and `maximumFileSize` (its input text).
    pub unit: FileSizeUnit,
    pub limit: String,
    /// The latest estimate, and whether one is with the worker or due
    /// after it.
    pub estimate: EstimateState,
    pub estimating: bool,
    pub estimate_dirty: bool,
    /// Whether the estimate with the worker also decodes the comparison.
    pub compare_sent: bool,
}

impl Default for ExportUi {
    fn default() -> Self {
        Self {
            open: false,
            custom: (1_920, 1_080),
            locked: true,
            unit: FileSizeUnit::Mb,
            limit: FileSizeUnit::Mb.value(DEFAULT_MAX_SIZE_BYTES),
            estimate: EstimateState::default(),
            estimating: false,
            estimate_dirty: false,
            compare_sent: false,
        }
    }
}

impl ExportUi {
    fn limit_bytes(&self) -> u64 {
        self.unit.bytes(&self.limit).unwrap_or(0)
    }
}

fn scaled(value: u32, numerator: u32, denominator: u32) -> u32 {
    ((u64::from(value) * u64::from(numerator) + u64::from(denominator) / 2)
        / u64::from(denominator.max(1)))
    .clamp(1, MAX_DIMENSION as u64) as u32
}

const fn unit_digits(unit: FileSizeUnit) -> usize {
    match unit {
        FileSizeUnit::Kb => 0,
        FileSizeUnit::Mb => 2,
        FileSizeUnit::Gb => 5,
    }
}

impl Editor {
    /// The document's pixel size, at least 1 × 1.
    fn export_size(&self) -> (u32, u32) {
        self.presented.as_ref().map_or((1, 1), |p| {
            (
                (p.document.width.round() as u32).max(1),
                (p.document.height.round() as u32).max(1),
            )
        })
    }

    /// Whether an export item is shown for the current options.
    pub fn export_item_shown(&self, item: Item) -> bool {
        match item {
            Item::Custom => matches!(self.options.size, ExportSize::Custom { .. }),
            Item::Compress => self.options.quality == ExportQuality::Compress,
            Item::Maximum => self.options.quality == ExportQuality::Maximum,
            // `canPreviewCompression && compressCompareDismissed`.
            Item::Compare => {
                self.options.quality != ExportQuality::Preserve && self.compare.dismissed
            }
            Item::Size | Item::Quality | Item::Estimate => true,
        }
    }

    /// The panel's controls, in Tab order.
    pub fn export_props(&self) -> Vec<Prop> {
        if !self.export.open {
            return Vec::new();
        }
        let mut list = vec![Prop::Export(Ex::Size)];
        if self.export_item_shown(Item::Custom) {
            list.extend([
                Prop::Number(Num::OutputWidth),
                Prop::Number(Num::OutputHeight),
                Prop::Export(Ex::Lock),
            ]);
        }
        list.push(Prop::Export(Ex::Quality));
        if self.export_item_shown(Item::Compress) {
            list.push(Prop::Export(Ex::Compress));
        }
        if self.export_item_shown(Item::Maximum) {
            list.extend([Prop::Number(Num::MaxSize), Prop::Export(Ex::Unit)]);
        }
        if self.export_item_shown(Item::Compare) {
            list.push(Prop::Export(Ex::Compare));
        }
        list
    }

    /// `(value, label, description)` rows of an export select.
    pub fn export_options(&self, ex: Ex) -> Vec<(u32, String, String)> {
        let view = self.export_view();
        let choices = |c: Option<Vec<editor_export::Choice>>| {
            c.unwrap_or_default()
                .into_iter()
                .map(|c| (c.value, c.label, c.description))
                .collect()
        };
        let plain = |rows: &[(&str, &str)]| {
            rows.iter()
                .enumerate()
                .map(|(i, (label, description))| {
                    (i as u32, (*label).to_owned(), (*description).to_owned())
                })
                .collect()
        };
        match ex {
            Ex::Size => plain(&[
                ("Original", "Keep the capture’s pixel dimensions."),
                ("75%", "Save at 75% of the pixel width and height."),
                ("50%", "Save at half the pixel width and height."),
                ("Custom", "Choose exact pixel dimensions."),
            ]),
            Ex::Quality => choices(view.map(|v| v.quality_modes)),
            Ex::Compress => choices(view.map(|v| v.quality_presets)),
            Ex::Unit => FileSizeUnit::ALL
                .iter()
                .enumerate()
                .map(|(i, unit)| (i as u32, unit.label().to_owned(), String::new()))
                .collect(),
            Ex::Format => {
                let transparent = self
                    .presented
                    .as_ref()
                    .is_some_and(|p| p.document.background.is_none());
                let jpeg = if transparent {
                    "Fills in transparent areas."
                } else {
                    ""
                };
                plain(&[("PNG", ""), ("JPEG", jpeg), ("WebP", "")])
            }
            Ex::Lock | Ex::Compare => Vec::new(),
        }
    }

    /// The chosen row of an export select.
    pub fn export_selected(&self, ex: Ex) -> Option<usize> {
        let o = self.options;
        match ex {
            Ex::Size => Some(match o.size {
                ExportSize::Original => 0,
                ExportSize::Percent { percent: 75 } => 1,
                ExportSize::Percent { .. } => 2,
                ExportSize::Custom { .. } => 3,
            }),
            Ex::Quality => editor_export::SAVE_QUALITY_MODES
                .iter()
                .position(|m| *m == o.quality),
            Ex::Compress => self
                .export_options(ex)
                .iter()
                .position(|(value, ..)| *value == u32::from(o.quality_value)),
            Ex::Unit => FileSizeUnit::ALL
                .iter()
                .position(|u| *u == self.export.unit),
            Ex::Format => Some(match o.format {
                ExportFormat::Png => 0,
                ExportFormat::Jpeg => 1,
                ExportFormat::Webp => 2,
            }),
            Ex::Lock | Ex::Compare => None,
        }
    }

    /// A row picked from an export select.
    pub(super) fn apply_export_choice(&mut self, ex: Ex, index: usize) {
        let mut options = self.options;
        match ex {
            Ex::Size => {
                let custom = matches!(options.size, ExportSize::Custom { .. });
                if index == 3 && !custom {
                    self.export.custom = self.export_size();
                }
                let (width, height) = self.export.custom;
                options.size = match index {
                    1 => ExportSize::Percent { percent: 75 },
                    2 => ExportSize::Percent { percent: 50 },
                    3 => ExportSize::Custom { width, height },
                    _ => ExportSize::Original,
                };
            }
            Ex::Quality => {
                let mode = editor_export::SAVE_QUALITY_MODES[index.min(2)];
                options.quality = mode;
                options.max_size_bytes =
                    (mode == ExportQuality::Maximum).then_some(self.export.limit_bytes());
            }
            Ex::Compress => {
                let Some(&(value, ..)) = self.export_options(ex).get(index) else {
                    return;
                };
                options.quality_value = value as u8;
                // Shared encoding owns PNG palette selection.
                options.png.max_colors = None;
            }
            Ex::Unit => {
                let next = FileSizeUnit::ALL[index.min(2)];
                let bytes = self.export.limit_bytes();
                self.export.unit = next;
                self.export.limit = next.value(bytes);
                self.prop_dirty(Prop::Number(Num::MaxSize));
            }
            Ex::Format => {
                options.format = match index {
                    1 => ExportFormat::Jpeg,
                    2 => ExportFormat::Webp,
                    _ => ExportFormat::Png,
                };
            }
            Ex::Lock | Ex::Compare => {}
        }
        self.set_export_options(options);
    }

    /// New export options: the bar, its controls and the estimate follow.
    pub(super) fn set_export_options(&mut self, options: ExportOptions) {
        if options == self.options {
            self.export_dirty();
            return;
        }
        let previous = self.options.quality;
        self.options = options;
        self.compare_quality_changed(previous);
        self.notice = None;
        self.events.push(Event::Notice);
        self.schedule_estimate();
        self.export_dirty();
    }

    /// Lay out and repaint everything that reads the export state.
    pub(super) fn export_dirty(&mut self) {
        let before = self.export_props();
        self.export_changed();
        self.props_changed();
        self.sync_compare();
        if before != self.export_props() {
            self.events.push(Event::Structure);
        }
    }

    pub(super) fn export_number(&self, num: Num) -> f64 {
        match num {
            Num::OutputWidth => f64::from(self.export.custom.0),
            Num::OutputHeight => f64::from(self.export.custom.1),
            _ => self.export.limit.trim().parse().unwrap_or(0.),
        }
    }

    /// A valid custom dimension or size limit typed or stepped.
    pub(super) fn set_export_number(&mut self, num: Num, value: f64) {
        let mut options = self.options;
        match num {
            Num::OutputWidth | Num::OutputHeight => {
                let value = value.round().clamp(1., MAX_DIMENSION) as u32;
                let (doc_w, doc_h) = self.export_size();
                let (mut w, mut h) = self.export.custom;
                if num == Num::OutputWidth {
                    w = value;
                    if self.export.locked {
                        h = scaled(value, doc_h, doc_w);
                    }
                } else {
                    h = value;
                    if self.export.locked {
                        w = scaled(value, doc_w, doc_h);
                    }
                }
                self.export.custom = (w, h);
                options.size = ExportSize::Custom {
                    width: w,
                    height: h,
                };
                // The other field shows the locked value.
                let other = if num == Num::OutputWidth {
                    Num::OutputHeight
                } else {
                    Num::OutputWidth
                };
                self.prop_dirty(Prop::Number(other));
            }
            _ => {
                let unit = self.export.unit;
                let text = format!("{value:.*}", unit_digits(unit));
                let text = if text.contains('.') {
                    text.trim_end_matches('0').trim_end_matches('.').to_owned()
                } else {
                    text
                };
                self.export.limit = text;
                options.max_size_bytes = Some(self.export.limit_bytes());
            }
        }
        self.set_export_options(options);
    }

    /// The size limit's bounds and step follow its unit.
    pub fn num_bounds(&self, num: Num) -> (f64, f64) {
        match num {
            Num::MaxSize => (self.export.unit.minimum(), 1_000_000.),
            _ => num.bounds(),
        }
    }

    pub fn num_step(&self, num: Num) -> f64 {
        match num {
            Num::MaxSize => self.export.unit.step(),
            _ => 1.,
        }
    }

    pub fn export_open(&self) -> bool {
        self.export.open
    }

    /// The filename input's text, drawn like a property field's.
    pub(super) fn draw_filename(&self, dc: &ID2D1DeviceContext, size: (f32, f32)) -> Result<()> {
        self.draw_field_text(dc, Prop::Filename, size)
    }

    /// Lock output aspect ratio.
    pub fn toggle_lock(&mut self) {
        self.export.locked = !self.export.locked;
        self.prop_dirty(Prop::Export(Ex::Lock));
        self.events
            .push(Event::Toggled(Target::Prop(Prop::Export(Ex::Lock))));
    }

    /// The disclosure.
    pub fn toggle_export_settings(&mut self) {
        self.export.open = !self.export.open;
        if !self.export.open
            && self
                .menu
                .is_some_and(|m| matches!(m.kind, MenuKind::Export(ex) if ex != Ex::Format))
        {
            self.close_menu();
        }
        if let Some(Target::Prop(prop)) = self.focus
            && !self.export_props().contains(&prop)
            && self.export_props_all().contains(&prop)
        {
            self.set_focus(Some(Target::ExportSettings), self.focus_visible);
        }
        self.export_dirty();
        self.events.push(Event::Expanded(Target::ExportSettings));
        self.events.push(Event::Structure);
    }

    /// Every panel control, shown or not.
    fn export_props_all(&self) -> [Prop; 9] {
        [
            Prop::Export(Ex::Size),
            Prop::Number(Num::OutputWidth),
            Prop::Number(Num::OutputHeight),
            Prop::Export(Ex::Lock),
            Prop::Export(Ex::Quality),
            Prop::Export(Ex::Compress),
            Prop::Number(Num::MaxSize),
            Prop::Export(Ex::Unit),
            Prop::Export(Ex::Compare),
        ]
    }

    /// Typing in the filename: shipping's `onChange`.
    pub(super) fn set_filename(&mut self, stem: String) {
        let before = self.pressed(Target::SaveAsNew);
        if let Some(target) = self.target.as_mut() {
            target.set_stem(stem);
        }
        self.notice = None;
        self.events.push(Event::Notice);
        if before != self.pressed(Target::SaveAsNew) {
            self.events.push(Event::Toggled(Target::SaveAsNew));
        }
        self.export_changed();
    }

    /// Change…'s folder.
    pub fn set_save_directory(&mut self, directory: std::path::PathBuf) {
        let before = self.pressed(Target::SaveAsNew);
        if let Some(target) = self.target.as_mut() {
            target.set_directory(directory);
        }
        self.notice = None;
        self.events.push(Event::Notice);
        if before != self.pressed(Target::SaveAsNew) {
            self.events.push(Event::Toggled(Target::SaveAsNew));
        }
        self.export_changed();
    }

    /// Re-encode exactly as Save would, off the UI thread; one at a time,
    /// the newest edit or options next.
    pub(super) fn schedule_estimate(&mut self) {
        if self.presented.is_none() {
            return;
        }
        self.export.estimate.pending = true;
        if self.export.estimating {
            self.export.estimate_dirty = true;
            return;
        }
        self.export.estimating = true;
        self.export.estimate_dirty = false;
        let compare = self.compare_visible();
        self.export.compare_sent = compare;
        self.compare_changed();
        let _ = self.jobs.send(super::Job::Estimate {
            options: self.options,
            compare,
        });
    }

    /// The worker's estimate; a superseded one is dropped.
    pub(super) fn receive_estimate(
        &mut self,
        result: std::result::Result<editor_export::ExportEstimate, String>,
        after: Option<super::Image>,
    ) {
        self.export.estimating = false;
        // Superseded, or the comparison opened while a plain estimate ran.
        if self.export.estimate_dirty || (self.compare_visible() && !self.export.compare_sent) {
            self.schedule_estimate();
            return;
        }
        self.export.estimate = match result {
            Ok(estimate) => EstimateState {
                bytes: Some(estimate.bytes),
                baseline_bytes: estimate.baseline_bytes,
                pending: false,
            },
            Err(_) => EstimateState::default(),
        };
        self.receive_after(after);
        self.export_dirty();
    }

    // ---- layout ----

    /// The panel's height and its items, relative to the panel, for a
    /// footer `w` wide.
    pub(super) fn export_panel(&self, w: f32, compact: bool) -> Result<(f32, Vec<(Item, Rect)>)> {
        let t = &self.t;
        let n = |name: &str| t.n(name);
        let (s3, s4, s5) = (n("s-3"), n("s-4"), n("s-5"));
        let caption_h = self.g.text(t, Style::Status, "Ag", 1e4)?.height;
        let line_h = (caption_h + s3 + n("h-md")).ceil();
        let inner = (w - 2. * s5 - 2. - 2. * s5).max(1.);
        let view = self.export_view();
        let caption_w = |text: &str| {
            self.g
                .text(t, Style::Status, text, 1e4)
                .map_or(0., |t| t.width)
        };
        let mut x = 0.;
        let mut y = 0.;
        let mut items = Vec::new();
        for item in ITEMS {
            if !self.export_item_shown(item) {
                continue;
            }
            let width = match item {
                Item::Size => {
                    let dims = self
                        .g
                        .text(t, Style::MonoTiny, &self.output_dimensions(), 1e4)?
                        .width;
                    (SIZE_SELECT_W + s4 + dims).max(caption_w("Output size"))
                }
                Item::Custom => CUSTOM_W,
                Item::Quality => 188.,
                Item::Compress if compact => 136.,
                Item::Compress => 148.,
                Item::Maximum if compact => 176.,
                Item::Maximum => 212.,
                Item::Compare => self
                    .compare_button_w()
                    .max(caption_w(captures_app::compression_compare::SHOW_CAPTION)),
                Item::Estimate => {
                    let label = view.as_ref().map_or("", |v| &v.estimate_label);
                    let mut width = self.g.text(t, Style::MonoSmall, label, 1e4)?.width;
                    if let Some(delta) = view.as_ref().and_then(|v| v.delta.as_ref()) {
                        width += s3 + self.g.text(t, Style::MonoSmall, &delta.label, 1e4)?.width;
                    }
                    width.max(72.).max(caption_w("Est. size")).ceil()
                }
            };
            if x > 0. && x + width > inner {
                x = 0.;
                y += line_h + s5;
            }
            items.push((item, Rect::new(1. + s5 + x, 1. + s5 + y, width, line_h)));
            x += width + s5;
        }
        Ok((2. + 2. * s5 + y + line_h, items))
    }

    /// `{output.width} × {output.height}`.
    fn output_dimensions(&self) -> String {
        let (w, h) = self.export_size();
        self.options
            .size
            .dimensions(w, h)
            .map_or_else(|_| "—".into(), |(w, h)| format!("{w} × {h}"))
    }

    /// `.screenshot-output-estimate`'s control while Export settings is open.
    pub(super) fn estimate_rect(&self) -> Option<Rect> {
        self.export_item(Item::Estimate)
            .filter(|_| self.export.open)
    }

    fn export_item(&self, item: Item) -> Option<Rect> {
        self.l
            .items
            .iter()
            .zip(ITEMS)
            .find(|(r, i)| *i == item && r.w > 0.)
            .map(|(r, _)| *r)
    }

    /// An export control's rect, relative to the bar.
    pub(super) fn export_prop_rect(&self, prop: Prop) -> Option<Rect> {
        self.presented.as_ref()?;
        let bar = self.l.bar;
        let local = |r: Rect| Rect::new(r.x - bar.x, r.y - bar.y, r.w, r.h);
        let t = &self.t;
        let s2 = t.n("s-2");
        let h = t.n("h-md");
        let control = |item: Item| {
            self.export_item(item)
                .filter(|_| self.export.open)
                .map(|r| local(Rect::new(r.x, r.y + r.h - h, r.w, h)))
        };
        let times_w = self
            .g
            .text(t, Style::Status, "×", 1e4)
            .map_or(7., |t| t.width);
        let field_w = (CUSTOM_W - LOCK_W - times_w - 3. * s2) / 2.;
        match prop {
            Prop::Filename => {
                let f = self.l.filename;
                Some(local(Rect::new(
                    f.x,
                    f.y,
                    (self.l.suffix.x - f.x).max(1.),
                    f.h,
                )))
            }
            Prop::Export(Ex::Format) => Some(local(self.l.suffix)),
            Prop::Export(Ex::Size) => control(Item::Size).map(|r| Rect {
                w: SIZE_SELECT_W,
                ..r
            }),
            Prop::Export(Ex::Quality) => control(Item::Quality),
            Prop::Export(Ex::Compress) => control(Item::Compress),
            Prop::Export(Ex::Compare) => control(Item::Compare).map(|r| Rect {
                w: self.compare_button_w(),
                ..r
            }),
            Prop::Export(Ex::Unit) => control(Item::Maximum).map(|r| Rect {
                x: r.x + r.w - UNIT_W,
                w: UNIT_W,
                ..r
            }),
            Prop::Export(Ex::Lock) => control(Item::Custom).map(|r| Rect {
                x: r.x + r.w - LOCK_W,
                w: LOCK_W,
                ..r
            }),
            Prop::Number(Num::OutputWidth) => {
                control(Item::Custom).map(|r| Rect { w: field_w, ..r })
            }
            Prop::Number(Num::OutputHeight) => control(Item::Custom).map(|r| Rect {
                x: r.x + field_w + s2 + times_w + s2,
                w: field_w,
                ..r
            }),
            Prop::Number(Num::MaxSize) => control(Item::Maximum).map(|r| Rect {
                w: r.w - UNIT_W - s2,
                ..r
            }),
            _ => None,
        }
    }

    // ---- painting ----

    /// The open panel, its captions and read-only values, on the bar.
    pub(super) fn draw_export_panel(&self, dc: &ID2D1DeviceContext) -> Result<()> {
        if !self.export.open {
            return Ok(());
        }
        let t = &self.t;
        let g = &self.g;
        let c = |name: &str| t.color(name);
        let (y0, x0) = (self.l.bar.y, self.l.bar.x);
        let p = self.l.panel;
        let panel = rect(p.x - x0, p.y - y0, p.x - x0 + p.w, p.y - y0 + p.h);
        let radius = t.n("r-xl");
        gfx::fill_round(dc, panel, radius, c("surface-sunken"))?;
        gfx::border(dc, panel, radius, 1., c("border-subtle"))?;
        let (s2, s3, s4) = (t.n("s-2"), t.n("s-3"), t.n("s-4"));
        let h = t.n("h-md");
        let view = self.export_view();
        let captions = [
            (Item::Size, "Output size"),
            (Item::Custom, "Width × height"),
            (Item::Quality, "Save quality"),
            (Item::Compress, "Quality"),
            (Item::Maximum, "Maximum file size"),
            (Item::Estimate, "Est. size"),
            (
                Item::Compare,
                captures_app::compression_compare::SHOW_CAPTION,
            ),
        ];
        for (item, caption) in captions {
            let Some(r) = self.export_item(item) else {
                continue;
            };
            let (x, y) = (r.x - x0, r.y - y0);
            let text = g.text(t, Style::Status, caption, 1e4)?;
            gfx::text(dc, &text, x, y, c("text-muted"))?;
            let mid = y + r.h - h / 2.;
            match item {
                Item::Size => {
                    let dims = g.text(t, Style::MonoTiny, &self.output_dimensions(), 1e4)?;
                    gfx::text(
                        dc,
                        &dims,
                        x + SIZE_SELECT_W + s4,
                        (mid - dims.height / 2.).round(),
                        c("text-subtle"),
                    )?;
                }
                Item::Custom => {
                    let times = g.text(t, Style::Status, "×", 1e4)?;
                    let field_w = (CUSTOM_W - LOCK_W - times.width - 3. * s2) / 2.;
                    gfx::text(
                        dc,
                        &times,
                        x + field_w + s2,
                        (mid - times.height / 2.).round(),
                        c("text-subtle"),
                    )?;
                }
                Item::Estimate => {
                    let Some(view) = view.as_ref() else {
                        continue;
                    };
                    let label = g.text(t, Style::MonoSmall, &view.estimate_label, 1e4)?;
                    let ink = if view.estimate_label == "—" || self.export.estimate.pending {
                        c("text-subtle")
                    } else {
                        c("text-muted")
                    };
                    let top = (mid - label.height / 2.).round();
                    gfx::text(dc, &label, x, top, ink)?;
                    if let Some(delta) = &view.delta {
                        let text = g.text(t, Style::MonoSmall, &delta.label, 1e4)?;
                        unsafe {
                            let _ = text.layout.SetFontWeight(
                                windows::Win32::Graphics::DirectWrite::DWRITE_FONT_WEIGHT_SEMI_BOLD,
                                windows::Win32::Graphics::DirectWrite::DWRITE_TEXT_RANGE {
                                    startPosition: 0,
                                    length: u32::MAX,
                                },
                            );
                        }
                        let ink = if delta.percent < 0 {
                            c("positive-text")
                        } else {
                            c("danger-text")
                        };
                        gfx::text(dc, &text, x + label.width + s3, top, ink)?;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// The aspect lock: `.screenshot-custom-dimensions > div > button`.
    pub(super) fn draw_lock(
        &self,
        dc: &ID2D1DeviceContext,
        layer: Layer,
        (w, h): (f32, f32),
    ) -> Result<()> {
        let t = &self.t;
        let c = |name: &str| t.color(name);
        let all = rect(0., 0., w, h);
        let radius = t.n("r-md");
        let on = self.export.locked;
        if layer == Layer::Lit {
            return if on {
                Ok(())
            } else {
                gfx::border(dc, all, radius, 1., c("border-strong"))
            };
        }
        let (fill, border, ink) = if on {
            ("theme-accent", "theme-accent", "theme-accent-ink")
        } else {
            ("surface-field", "control-border", "text-subtle")
        };
        gfx::fill_round(dc, all, radius, c(fill))?;
        gfx::border(dc, all, radius, 1., c(border))?;
        gfx::icon(
            dc,
            &self.g.factory,
            if on { "lock" } else { "unlock" },
            &[],
            ((w - 14.) / 2., (h - 14.) / 2., 14.),
            1.8,
            c(ink),
        )
    }

    /// The filename's `.filename-format-select`: a borderless trigger in
    /// the field showing the suffix.
    pub(super) fn draw_format_select(
        &self,
        dc: &ID2D1DeviceContext,
        layer: Layer,
        (w, h): (f32, f32),
    ) -> Result<()> {
        let t = &self.t;
        let c = |name: &str| t.color(name);
        let all = rect(0., 0., w, h);
        let prop = Prop::Export(Ex::Format);
        let active =
            self.menu_open(MenuKind::Export(Ex::Format)) || self.focus == Some(Target::Prop(prop));
        let enabled = self.enabled(Target::Prop(prop));
        let lit = layer == Layer::Lit;
        if lit && (active || !enabled) {
            return Ok(());
        }
        let radius = (t.n("r-md") - 1.).max(0.);
        let ink = if lit || active {
            gfx::fill_round(dc, all, radius, c("control-hover"))?;
            c("text")
        } else if enabled {
            c("text-subtle")
        } else {
            t.color_alpha("text-subtle", 0.45)
        };
        let s4 = t.n("s-4");
        let suffix = self
            .export_view()
            .map_or_else(|| ".png".into(), |v| v.suffix);
        let text = self.g.text(t, Style::Option, &suffix, 1e4)?;
        gfx::text(dc, &text, s4, ((h - text.height) / 2.).round(), ink)?;
        gfx::icon(
            dc,
            &self.g.factory,
            "chevron-down",
            &[],
            (w - 18., (h - 11.) / 2., 11.),
            1.8,
            ink,
        )
    }
}

use captures_app::editor_session::ExportOptions;
