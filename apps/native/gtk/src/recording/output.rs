//! The recording editor's output options (shipping `.recording-editor-grid`):
//! GIF settings, Crop & size, Save quality with Est. size, and Audio, plus
//! the filename's format select. This module owns the widgets and the values
//! shipping keeps only in the UI; `specs` turns them into the session's
//! `EditSpec`/`ExportSpec` with shipping's `buildExportRequestSpecs` rules,
//! and `recording_editor_ui` owns every label, menu and the estimate copy.
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use captures_app::editor_export::FileSizeUnit;
use captures_app::recording_editor_ui::{
    self as ui, DEFAULT_COMPRESS_PRESET, EstimateInput, QualityMode, ResolutionChoice,
};
use captures_media::{
    AudioEdit, CropDragHandle, CropRect, CropResizeAxis, EditSpec, ExportFormat, ExportSpec,
    MediaKind, QualityPreset,
};
use captures_settings::{RecordingSettings, VideoFormat};
use gtk::prelude::*;

use crate::editor::controls::{Choice, NumberField};
use crate::editor::properties::{RangeSlider, check_row};

/// The opened source, as shipping's `RecordingArtifact` describes it.
#[derive(Clone, Copy)]
pub(super) struct Source {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) kind: MediaKind,
    pub(super) format: VideoFormat,
    pub(super) has_system_audio: bool,
    pub(super) has_microphone_audio: bool,
    pub(super) size_bytes: u64,
}

impl Source {
    pub(super) fn format_of(mime_type: &str) -> VideoFormat {
        let essence = mime_type.split(';').next().unwrap_or_default().trim();
        if essence.eq_ignore_ascii_case("image/gif") {
            VideoFormat::Gif
        } else if essence.eq_ignore_ascii_case("video/webm") {
            VideoFormat::WebM
        } else {
            VideoFormat::Mp4
        }
    }
}

/// What a control changed, so the host can follow up beyond a live apply.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Change {
    Edit,
    /// The format select: shipping's `updateOutputFormat` also saves a copy.
    Format,
    /// Crop recording was turned on or off: the preview swaps frames.
    CropShown,
    /// Only the Est. size inputs moved (Maximum's text or unit).
    Estimate,
}

/// The latest background estimate, kept while a newer one is pending.
#[derive(Clone, Copy, Default)]
pub(super) struct Estimate {
    pub(super) bytes: Option<u64>,
    pub(super) exact: bool,
    pub(super) pending: bool,
}

/// `evenDimension`: at least 2, rounded, and even.
fn even(value: f64) -> u32 {
    let rounded = value.round().max(2.) as u32;
    rounded - rounded % 2
}

/// `editorOutputDimensions`.
fn output_dimensions(
    width: u32,
    height: u32,
    resolution: ResolutionChoice,
    custom: (u32, u32),
) -> (u32, u32) {
    let maximum = match resolution {
        ResolutionChoice::Custom => {
            return (even(f64::from(custom.0)), even(f64::from(custom.1)));
        }
        ResolutionChoice::P1080 => 1_080.,
        ResolutionChoice::P720 => 720.,
        ResolutionChoice::Original => f64::from(height),
    };
    let (width, height) = (f64::from(width), f64::from(height));
    let scale = if height > maximum {
        maximum / height
    } else {
        1.
    };
    (even(width * scale), even(height * scale))
}

/// `dimensionsAtMaximumWidth`.
fn at_maximum_width((width, height): (u32, u32), maximum: u32) -> (u32, u32) {
    if width <= maximum {
        return (width, height);
    }
    let scale = f64::from(maximum) / f64::from(width);
    (even(f64::from(maximum)), even(f64::from(height) * scale))
}

/// `gifMaxColorsForQuality`.
fn gif_colors(quality: QualityPreset) -> u16 {
    match quality {
        QualityPreset::Tiny => 64,
        QualityPreset::Small => 96,
        QualityPreset::Standard => 128,
        QualityPreset::Preserve | QualityPreset::High | QualityPreset::Highest => 256,
    }
}

fn id_of<T: serde::Serialize>(value: T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn parse<T: serde::de::DeserializeOwned>(id: &str) -> Option<T> {
    serde_json::from_value(serde_json::Value::String(id.to_owned())).ok()
}

fn described(choices: Vec<ui::Choice>) -> Vec<[String; 3]> {
    choices
        .into_iter()
        .map(|choice| [choice.value, choice.label, choice.description])
        .collect()
}

/// A control's change to the model.
type ModelEdit<T> = dyn Fn(&Model, T);

/// Shipping's UI-only editor state.
struct Model {
    source: Cell<Option<Source>>,
    format: Cell<VideoFormat>,
    gif_fps: Cell<u16>,
    gif_width: Cell<u32>,
    crop_on: Cell<bool>,
    crop: Cell<CropRect>,
    locked: Cell<bool>,
    resolution: Cell<ResolutionChoice>,
    custom: Cell<(u32, u32)>,
    mode: Cell<QualityMode>,
    quality: Cell<QualityPreset>,
    limit: RefCell<String>,
    unit: Cell<FileSizeUnit>,
    volume: Cell<(f64, f64)>,
    mute: Cell<(bool, bool)>,
    mono: Cell<bool>,
    /// Widgets are being set from the model; their signals are echoes.
    syncing: Cell<bool>,
}

pub(super) struct Output {
    /// `.recording-editor-grid`.
    pub(super) root: gtk::Grid,
    /// The filename's `.mp4`/`.gif`/`.webm` select.
    pub(super) format: Rc<Choice>,
    gif_card: gtk::Box,
    gif_fps: Rc<Choice>,
    gif_width: Rc<Choice>,
    crop_check: gtk::ToggleButton,
    crop_fields: [NumberField; 4],
    lock: gtk::ToggleButton,
    resolution: Rc<Choice>,
    custom_row: gtk::Box,
    custom: [NumberField; 2],
    mode: Rc<Choice>,
    help: gtk::Label,
    preset_field: gtk::Box,
    preset: Rc<Choice>,
    maximum_field: gtk::Box,
    limit: NumberField,
    unit: Rc<Choice>,
    estimate: gtk::Label,
    delta: gtk::Label,
    audio_card: gtk::Box,
    system: (gtk::Box, gtk::ToggleButton, RangeSlider),
    microphone: (gtk::Box, gtk::ToggleButton, RangeSlider),
    mono: gtk::ToggleButton,
    audio_note: gtk::Box,
    model: Rc<Model>,
}

fn card(title: &str, classes: &[&str]) -> gtk::Box {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
    card.add_css_class("editor-card");
    for class in classes {
        card.add_css_class(class);
    }
    let heading = gtk::Label::new(Some(title));
    heading.add_css_class("editor-card-title");
    heading.set_xalign(0.);
    heading.set_accessible_role(gtk::AccessibleRole::Heading);
    card.append(&heading);
    card
}

/// `.editor-field`: caption over its control. The caption is presentational;
/// each control carries shipping's own accessible name.
fn field(caption: &str, control: &impl IsA<gtk::Widget>, class: &str) -> gtk::Box {
    let field = gtk::Box::new(gtk::Orientation::Vertical, 8);
    field.add_css_class("editor-field");
    if !class.is_empty() {
        field.add_css_class(class);
    }
    let label = gtk::Label::new(Some(caption));
    label.add_css_class("editor-field-caption");
    label.set_xalign(0.);
    label.set_accessible_role(gtk::AccessibleRole::Presentation);
    field.append(&label);
    field.append(control);
    field
}

fn columns(count: usize, children: &[&gtk::Widget]) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.add_css_class("editor-number-grid");
    row.set_homogeneous(true);
    for child in children.iter().take(count) {
        row.append(*child);
    }
    row
}

fn set_class(widget: &impl IsA<gtk::Widget>, class: &str, on: bool) {
    if on {
        widget.add_css_class(class);
    } else {
        widget.remove_css_class(class);
    }
}

impl Output {
    pub(super) fn new(changed: impl Fn(Change) + 'static) -> Rc<Self> {
        let changed: Rc<dyn Fn(Change)> = Rc::new(changed);
        let model = Rc::new(Model {
            source: Cell::new(None),
            format: Cell::new(VideoFormat::Mp4),
            gif_fps: Cell::new(15),
            gif_width: Cell::new(800),
            crop_on: Cell::new(false),
            crop: Cell::new(CropRect {
                x: 0,
                y: 0,
                width: 2,
                height: 2,
            }),
            locked: Cell::new(true),
            resolution: Cell::new(ResolutionChoice::Original),
            custom: Cell::new((1_920, 1_080)),
            mode: Cell::new(QualityMode::Preserve),
            quality: Cell::new(DEFAULT_COMPRESS_PRESET),
            limit: RefCell::new("10".into()),
            unit: Cell::new(FileSizeUnit::Mb),
            volume: Cell::new((100., 100.)),
            mute: Cell::new((false, false)),
            mono: Cell::new(false),
            syncing: Cell::new(false),
        });
        // A control's handler: ignore echoes, update the model, notify.
        let on = |change: Change, edit: Rc<ModelEdit<String>>| {
            let model = model.clone();
            let changed = changed.clone();
            move |value: String| {
                if model.syncing.get() {
                    return;
                }
                edit(&model, value);
                changed(change);
            }
        };
        let number = |change: Change, edit: Rc<ModelEdit<f64>>| {
            let model = model.clone();
            let changed = changed.clone();
            move |value: f64| {
                if model.syncing.get() {
                    return;
                }
                edit(&model, value);
                changed(change);
            }
        };

        // GIF settings.
        let gif_fps = Choice::new(
            None,
            "GIF frame rate",
            false,
            on(
                Change::Edit,
                Rc::new(|model, id| {
                    if let Ok(fps) = id.parse() {
                        model.gif_fps.set(fps);
                    }
                }),
            ),
        );
        let gif_width = Choice::new(
            None,
            "GIF maximum width",
            false,
            on(
                Change::Edit,
                Rc::new(|model, id| {
                    if let Ok(width) = id.parse() {
                        model.gif_width.set(width);
                    }
                }),
            ),
        );
        let menus = ui::menus(true, 2, 2);
        gif_fps.set_options(
            menus
                .gif_frame_rates
                .into_iter()
                .map(|c| (c.value, c.label))
                .collect(),
        );
        gif_width.set_options(
            menus
                .gif_maximum_widths
                .into_iter()
                .map(|c| (c.value, c.label))
                .collect(),
        );
        let gif_card = card("GIF settings", &["editor-output-card"]);
        gif_card.append(&columns(
            2,
            &[
                field("Frame rate", &gif_fps.root, "").upcast_ref(),
                field("Maximum width", &gif_width.root, "").upcast_ref(),
            ],
        ));

        // Crop & size.
        let crop_card = card("Crop & size", &[]);
        let crop_check = check_row("Crop recording", false);
        crop_card.append(&crop_check);
        let crop_field = |name: &str, index: usize| {
            NumberField::new(
                name,
                (0., 65_536.),
                number(
                    Change::Edit,
                    Rc::new(move |model: &Model, value: f64| model.edit_crop(index, value)),
                ),
            )
        };
        let crop_fields = [
            crop_field("X", 0),
            crop_field("Y", 1),
            crop_field("Width", 2),
            crop_field("Height", 3),
        ];
        let captioned: Vec<gtk::Widget> = ["X", "Y", "Width", "Height"]
            .iter()
            .zip(&crop_fields)
            .map(|(caption, field_)| field(caption, &field_.root, "").upcast())
            .collect();
        crop_card.append(&columns(4, &captioned.iter().collect::<Vec<_>>()));
        let lock = check_row("Lock aspect ratio", true);
        lock.add_css_class("compact");
        lock.add_css_class("editor-aspect-lock");
        crop_card.append(&lock);
        let resolution = Choice::new(
            None,
            "Output resolution",
            false,
            on(
                Change::Edit,
                Rc::new(|model, id| {
                    if let Some(choice) = parse(&id) {
                        model.resolution.set(choice);
                    }
                }),
            ),
        );
        crop_card.append(&field(
            "Output resolution",
            &resolution.root,
            "editor-resolution-field",
        ));
        let custom_field = |name: &str, width: bool| {
            NumberField::new(
                name,
                (2., 16_384.),
                number(
                    Change::Edit,
                    Rc::new(move |model: &Model, value: f64| {
                        let (w, h) = model.custom.get();
                        let value = value.round().max(2.) as u32;
                        model
                            .custom
                            .set(if width { (value, h) } else { (w, value) });
                    }),
                ),
            )
        };
        let custom = [custom_field("Width", true), custom_field("Height", false)];
        let custom_row = columns(
            2,
            &[
                field("Width", &custom[0].root, "").upcast_ref(),
                field("Height", &custom[1].root, "").upcast_ref(),
            ],
        );
        custom_row.add_css_class("dimensions");
        crop_card.append(&custom_row);

        // Save quality.
        let quality_card = card("Save quality", &["editor-quality-card"]);
        let mode = Choice::new(
            None,
            "Save quality",
            false,
            on(
                Change::Edit,
                Rc::new(|model, id| {
                    if let Some(mode) = parse(&id) {
                        model.mode.set(mode);
                    }
                }),
            ),
        );
        quality_card.append(&field(
            "Quality mode",
            &mode.root,
            "editor-quality-mode-field",
        ));
        let help = gtk::Label::new(None);
        help.add_css_class("editor-field-help");
        help.set_xalign(0.);
        help.set_wrap(true);
        quality_card.append(&help);
        let preset = Choice::new(
            None,
            "Compression quality",
            false,
            on(
                Change::Edit,
                Rc::new(|model, id| {
                    if let Some(quality) = parse(&id) {
                        model.quality.set(quality);
                    }
                }),
            ),
        );
        preset.set_described(described(ui::menus(false, 2, 2).quality_presets));
        let preset_field = field("Quality", &preset.root, "editor-quality-preset");
        quality_card.append(&preset_field);
        let limit = NumberField::new(
            "Maximum file size",
            (0., 1e12),
            number(
                Change::Estimate,
                Rc::new(|model: &Model, value: f64| {
                    let digits = model.unit.get().step().log10().abs().ceil() as usize;
                    let text = format!("{value:.digits$}");
                    let text = if text.contains('.') {
                        text.trim_end_matches('0').trim_end_matches('.').to_owned()
                    } else {
                        text
                    };
                    *model.limit.borrow_mut() = text;
                }),
            ),
        );
        let unit = Choice::new(
            None,
            "File size unit",
            false,
            on(
                Change::Estimate,
                Rc::new(|model, id| {
                    let next = FileSizeUnit::ALL
                        .into_iter()
                        .find(|unit| unit.label() == id)
                        .unwrap_or_default();
                    let unit = model.unit.replace(next);
                    let bytes = unit.bytes(&model.limit.borrow());
                    if let Some(bytes) = bytes {
                        *model.limit.borrow_mut() = next.value(bytes);
                    }
                }),
            ),
        );
        unit.set_options(
            FileSizeUnit::ALL
                .map(|unit| (unit.label().to_owned(), unit.label().to_owned()))
                .into(),
        );
        unit.root.set_size_request(92, -1);
        let size_limit = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        size_limit.add_css_class("editor-size-limit");
        limit.root.set_hexpand(true);
        size_limit.append(&limit.root);
        size_limit.append(&unit.root);
        let maximum_field = field("Maximum file size", &size_limit, "");
        quality_card.append(&maximum_field);
        let estimate = gtk::Label::new(None);
        estimate.add_css_class("recording-output-estimate");
        estimate.set_tooltip_text(Some(ui::ESTIMATE_HELP));
        let delta = gtk::Label::new(None);
        delta.add_css_class("recording-output-estimate-delta");
        delta.set_tooltip_text(Some(ui::DELTA_HELP));
        let estimate_row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        estimate_row.append(&estimate);
        estimate_row.append(&delta);
        estimate_row.set_accessible_role(gtk::AccessibleRole::Status);
        quality_card.append(&field(
            "Est. size",
            &estimate_row,
            "recording-output-estimate-field",
        ));

        // Audio.
        let audio_card = card("Audio", &["editor-audio-card"]);
        let volume_row = |caption: &str, name: &str, microphone: bool| {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 16);
            row.add_css_class("editor-volume");
            let check = check_row(caption, true);
            check.set_size_request(130, -1);
            // `.editor-volume`'s fixed 130 px column: the caption must not
            // expand the check and push the slider right.
            check.set_hexpand(false);
            check.set_valign(gtk::Align::Center);
            row.append(&check);
            let slider = RangeSlider::new("", name, (0., 200.), 100., &[], |value| {
                format!("{value:.0}%")
            });
            // Shipping labels the checkbox, not the slider row.
            if let Some(caption) = slider.root.first_child() {
                caption.set_visible(false);
            }
            slider.root.set_hexpand(true);
            row.append(&slider.root);
            {
                let model = model.clone();
                let changed = changed.clone();
                let scale = slider.scale.clone();
                check.connect_toggled(move |check| {
                    let muted = !check.is_active();
                    scale.set_sensitive(!muted);
                    if model.syncing.get() {
                        return;
                    }
                    let (system, mic) = model.mute.get();
                    model.mute.set(if microphone {
                        (system, muted)
                    } else {
                        (muted, mic)
                    });
                    changed(Change::Edit);
                });
            }
            {
                let model = model.clone();
                let changed = changed.clone();
                slider.scale.connect_value_changed(move |scale| {
                    if model.syncing.get() {
                        return;
                    }
                    let value = scale.value().round();
                    let (system, mic) = model.volume.get();
                    model.volume.set(if microphone {
                        (system, value)
                    } else {
                        (value, mic)
                    });
                    changed(Change::Edit);
                });
            }
            audio_card.append(&row);
            (row, check, slider)
        };
        let system = volume_row("System audio", "System audio volume", false);
        let microphone = volume_row("Microphone", "Microphone volume", true);
        let mono = check_row("Convert to mono", false);
        mono.add_css_class("compact");
        audio_card.append(&mono);
        let audio_note = card("Audio", &["editor-audio-warning"]);
        let note = gtk::Label::new(Some(ui::GIF_AUDIO_NOTE));
        note.set_xalign(0.);
        note.set_wrap(true);
        audio_note.append(&note);
        audio_note.set_accessible_role(gtk::AccessibleRole::Status);

        // The filename's format select.
        let format = Choice::new(
            None,
            "Format",
            false,
            on(
                Change::Format,
                Rc::new(|model, id| {
                    let format = match id.as_str() {
                        "gif" => VideoFormat::Gif,
                        "webm" => VideoFormat::WebM,
                        _ => VideoFormat::Mp4,
                    };
                    model.format.set(format);
                    // Shipping switches a GIF to Compress; Preserve is MP4-only.
                    if format != VideoFormat::Mp4 && model.mode.get() == QualityMode::Preserve {
                        model.mode.set(QualityMode::Compress);
                    }
                }),
            ),
        );
        format.set_options(vec![
            ("mp4".into(), "MP4".into()),
            ("gif".into(), "GIF".into()),
            ("webm".into(), "WebM".into()),
        ]);
        format.root.add_css_class("filename-format-select");
        format.root.set_hexpand(false);
        format.fill_height();

        let root = gtk::Grid::new();
        root.add_css_class("recording-editor-grid");
        root.set_column_homogeneous(true);
        root.set_column_spacing(16);
        root.attach(&gif_card, 0, 0, 2, 1);
        root.attach(&crop_card, 0, 1, 1, 1);
        root.attach(&quality_card, 1, 1, 1, 1);
        root.attach(&audio_card, 0, 2, 2, 1);
        root.attach(&audio_note, 0, 3, 2, 1);

        let output = Rc::new(Output {
            root,
            format,
            gif_card,
            gif_fps,
            gif_width,
            crop_check,
            crop_fields,
            lock,
            resolution,
            custom_row,
            custom,
            mode,
            help,
            preset_field,
            preset,
            maximum_field,
            limit,
            unit,
            estimate,
            delta,
            audio_card,
            system,
            microphone,
            mono,
            audio_note,
            model,
        });
        let weak = Rc::downgrade(&output);
        let notify = changed.clone();
        output.crop_check.connect_toggled(move |check| {
            let Some(output) = weak.upgrade() else { return };
            if output.model.syncing.get() {
                return;
            }
            output.model.crop_on.set(check.is_active());
            notify(Change::CropShown);
        });
        let weak = Rc::downgrade(&output);
        output.lock.connect_toggled(move |check| {
            if let Some(output) = weak.upgrade() {
                output.model.locked.set(check.is_active());
            }
        });
        let weak = Rc::downgrade(&output);
        let notify = changed;
        output.mono.connect_toggled(move |check| {
            let Some(output) = weak.upgrade() else { return };
            if output.model.syncing.get() {
                return;
            }
            output.model.mono.set(check.is_active());
            notify(Change::Edit);
        });
        output
    }

    /// The editor opened `source`: shipping's per-artifact initial state.
    pub(super) fn open(&self, source: Source, preferences: &RecordingSettings) {
        let model = &self.model;
        let format = match source.format {
            VideoFormat::Gif => "image/gif",
            VideoFormat::WebM => "video/webm",
            VideoFormat::Mp4 => "video/mp4",
        };
        let format = ui::initial_output_format(format, preferences.video_format);
        model.source.set(Some(source));
        model.format.set(format);
        model.mode.set(if format == VideoFormat::Gif {
            QualityMode::Compress
        } else {
            QualityMode::Preserve
        });
        model.gif_fps.set(preferences.gif_fps);
        model.gif_width.set(preferences.gif_max_width);
        model.crop.set(CropRect {
            x: 0,
            y: 0,
            width: source.width,
            height: source.height,
        });
        model.custom.set((source.width, source.height));
    }

    pub(super) fn format(&self) -> VideoFormat {
        self.model.format.get()
    }

    pub(super) fn extension(&self) -> &'static str {
        match self.format() {
            VideoFormat::Gif => ".gif",
            VideoFormat::WebM => ".webm",
            VideoFormat::Mp4 => ".mp4",
        }
    }

    pub(super) fn source(&self) -> Option<Source> {
        self.model.source.get()
    }

    /// The crop box shows over the uncropped source.
    pub(super) fn crop_shown(&self) -> Option<CropRect> {
        self.model.crop_on.get().then(|| self.model.crop.get())
    }

    /// A drag on the preview moved the crop box (source pixels).
    pub(super) fn drag_crop(&self, from: CropRect, handle: CropDragHandle, dx: f64, dy: f64) {
        let Some(source) = self.model.source.get() else {
            return;
        };
        if let Some(crop) = from.after_drag(
            source.width,
            source.height,
            handle,
            dx,
            dy,
            self.model.locked.get(),
        ) {
            self.model.crop.set(crop);
            self.sync_crop();
        }
    }

    /// Shipping estimates only without Maximum mode and for non-WebM output.
    pub(super) fn wants_estimate(&self) -> bool {
        self.model.mode.get() != QualityMode::Maximum && self.format() != VideoFormat::WebM
    }

    /// The typed Maximum limit when it is valid (at least 100 KB).
    fn maximum_bytes(&self) -> Option<u64> {
        self.model
            .unit
            .get()
            .bytes(&self.model.limit.borrow())
            .filter(|bytes| *bytes >= 100_000)
    }

    /// Shipping's `buildExportRequestSpecs` for the staged trim; `None` while
    /// the Maximum limit is invalid, so nothing unbounded is applied.
    pub(super) fn specs(
        &self,
        trim_start_ms: u64,
        trim_end_ms: Option<u64>,
    ) -> Option<(EditSpec, ExportSpec)> {
        let model = &self.model;
        let source = model.source.get()?;
        let gif = model.format.get() == VideoFormat::Gif;
        let crop = model.crop_on.get().then(|| model.crop.get());
        let (width, height) = crop.map_or((source.width, source.height), |c| (c.width, c.height));
        let base = output_dimensions(width, height, model.resolution.get(), model.custom.get());
        let output = if gif {
            at_maximum_width(base, model.gif_width.get())
        } else {
            base
        };
        let resized = model.resolution.get() != ResolutionChoice::Original || gif;
        let (mute_system, mute_microphone) = model.mute.get();
        let (system_volume, microphone_volume) = model.volume.get();
        let edit = EditSpec {
            trim_start_ms,
            trim_end_ms,
            crop,
            output_width: resized.then_some(output.0),
            output_height: resized.then_some(output.1),
            audio: AudioEdit {
                system_volume: (system_volume / 100.) as f32,
                microphone_volume: (microphone_volume / 100.) as f32,
                mute_system_audio: gif || mute_system,
                mute_microphone: gif || mute_microphone,
                mono_output: model.mono.get(),
                source_has_system_audio: source.has_system_audio,
                source_has_microphone_audio: source.has_microphone_audio,
            },
        };
        let mode = model.mode.get();
        let maximum = match mode {
            QualityMode::Maximum => Some(self.maximum_bytes()?),
            _ => None,
        };
        let export = ExportSpec {
            // WebM cannot be encoded by the bundled tools: the session keeps
            // MP4 settings and Save reports shipping's error.
            format: if gif {
                ExportFormat::Gif
            } else {
                ExportFormat::Mp4
            },
            quality: if mode == QualityMode::Compress {
                model.quality.get()
            } else {
                QualityPreset::Preserve
            },
            max_size_bytes: maximum,
            frames_per_second: gif.then(|| model.gif_fps.get()),
            gif_max_colors: gif.then(|| gif_colors(model.quality.get())),
        };
        Some((edit, export))
    }

    /// Show the model in every card; `estimate` is the latest Est. size.
    pub(super) fn sync(&self, estimate: Estimate) {
        let model = &self.model;
        let Some(source) = model.source.get() else {
            return;
        };
        model.syncing.set(true);
        let format = model.format.get();
        let gif = format == VideoFormat::Gif;
        self.format.set(match format {
            VideoFormat::Gif => "gif",
            VideoFormat::WebM => "webm",
            VideoFormat::Mp4 => "mp4",
        });
        self.format.set_caption(Some(self.extension()));

        self.gif_card.set_visible(gif);
        self.gif_fps.set(&model.gif_fps.get().to_string());
        self.gif_width.set(&model.gif_width.get().to_string());

        self.crop_check.set_active(model.crop_on.get());
        self.sync_crop();
        self.lock.set_active(model.locked.get());
        let crop = model.crop.get();
        let base = if model.crop_on.get() {
            (crop.width, crop.height)
        } else {
            (source.width, source.height)
        };
        let base = output_dimensions(base.0, base.1, ResolutionChoice::Original, (0, 0));
        let menus = ui::menus(format != VideoFormat::Mp4, base.0, base.1);
        self.resolution.set_described(described(menus.resolutions));
        self.resolution.set(&id_of(model.resolution.get()));
        let custom = model.resolution.get() == ResolutionChoice::Custom;
        self.custom_row.set_visible(custom);
        let (w, h) = model.custom.get();
        self.custom[0].set(f64::from(w), false);
        self.custom[1].set(f64::from(h), false);

        let mode = model.mode.get();
        self.mode.set_described(described(menus.quality_modes));
        self.mode.set(&id_of(mode));
        self.help.set_label(mode.description());
        self.preset_field.set_visible(mode == QualityMode::Compress);
        self.preset.set(&id_of(model.quality.get()));
        self.maximum_field.set_visible(mode == QualityMode::Maximum);
        let unit = model.unit.get();
        self.unit.set(unit.label());
        let digits = unit.step().log10().abs().ceil() as u32;
        self.limit.spin.set_digits(digits);
        self.limit
            .spin
            .set_increments(unit.step().max(0.1_f64.powi(digits as i32)), 1.);
        if let Ok(value) = model.limit.borrow().parse::<f64>() {
            self.limit.set(value, false);
        }

        let shown = ui::estimate(&EstimateInput {
            estimating: estimate.pending,
            webm: format == VideoFormat::WebM,
            maximum: mode == QualityMode::Maximum,
            maximum_bytes: self.maximum_bytes(),
            estimate_bytes: estimate.bytes,
            estimate_exact: estimate.exact,
            original_bytes: source.size_bytes,
        });
        self.estimate.set_label(&shown.label);
        set_class(&self.estimate, "pending", shown.muted);
        self.estimate
            .update_property(&[gtk::accessible::Property::Label(&format!(
                "Est. size: {}",
                shown.label
            ))]);
        self.delta.set_visible(shown.delta.is_some());
        if let Some(delta) = &shown.delta {
            self.delta.set_label(&delta.label);
            set_class(&self.delta, "is-smaller", delta.smaller());
            set_class(&self.delta, "is-larger", !delta.smaller());
        }

        let video = source.kind == MediaKind::Video;
        let audio = source.has_system_audio || source.has_microphone_audio;
        self.audio_card
            .set_visible(video && format == VideoFormat::Mp4 && audio);
        self.audio_note.set_visible(video && gif && audio);
        let (mute_system, mute_microphone) = model.mute.get();
        let (system_volume, microphone_volume) = model.volume.get();
        for ((row, check, slider), present, muted, volume) in [
            (
                &self.system,
                source.has_system_audio,
                mute_system,
                system_volume,
            ),
            (
                &self.microphone,
                source.has_microphone_audio,
                mute_microphone,
                microphone_volume,
            ),
        ] {
            row.set_visible(present);
            check.set_active(!muted);
            slider.scale.set_sensitive(!muted);
            slider.set(volume, false);
        }
        self.mono.set_active(model.mono.get());
        model.syncing.set(false);
    }

    fn sync_crop(&self) {
        let model = &self.model;
        let syncing = model.syncing.replace(true);
        let crop = model.crop.get();
        let on = model.crop_on.get();
        for (field, value) in self
            .crop_fields
            .iter()
            .zip([crop.x, crop.y, crop.width, crop.height])
        {
            field.set(f64::from(value), false);
            field.set_sensitive(on);
        }
        model.syncing.set(syncing);
    }
}

impl Model {
    /// `updateCropOrigin` (X, Y) and `updateCropDimension` (Width, Height).
    fn edit_crop(&self, index: usize, value: f64) {
        let Some(source) = self.source.get() else {
            return;
        };
        let mut crop = self.crop.get();
        let value = value.round().max(0.) as u32;
        match index {
            0 => crop.x = value.min(source.width.saturating_sub(crop.width)),
            1 => crop.y = value.min(source.height.saturating_sub(crop.height)),
            2 | 3 if self.locked.get() => {
                let axis = if index == 2 {
                    CropResizeAxis::Width
                } else {
                    CropResizeAxis::Height
                };
                crop = crop.resize_aspect_locked(source.width, source.height, axis, value);
            }
            2 => crop.width = value.clamp(2, source.width.saturating_sub(crop.x).max(2)),
            _ => crop.height = value.clamp(2, source.height.saturating_sub(crop.y).max(2)),
        }
        self.crop.set(crop);
    }
}
