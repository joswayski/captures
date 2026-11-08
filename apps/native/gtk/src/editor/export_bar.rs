//! Shipping's `.screenshot-export-options` panel and its Export settings
//! disclosure: Output size (Custom adds width × height and the aspect lock),
//! Save quality, the Compress preset, Maximum file size with its unit, and
//! Est. size; plus the filename's format select. `editor_export` owns every
//! label, rule and the estimate; this module owns the widgets and the values
//! shipping keeps only in the UI (custom size, lock, size-limit text).
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use captures_app::compression_compare;
use captures_app::editor_export::{self, DEFAULT_MAX_SIZE_BYTES, ExportBarView, FileSizeUnit};
use captures_app::editor_session::{ExportFormat, ExportOptions, ExportQuality, ExportSize};
use gtk::prelude::*;

use super::controls::{Choice, NumberField};
use crate::widgets::Icon;

/// `MAX_SCREENSHOT_OUTPUT_DIMENSION`.
const MAX_DIMENSION: f64 = 16_384.;

type Changed = Rc<dyn Fn(ExportOptions)>;

pub(super) struct ExportBar {
    /// `.screenshot-export-options`, shown while the disclosure is open.
    pub(super) panel: gtk::Box,
    pub(super) disclosure: gtk::Button,
    /// The filename's `.png`/`.jpg`/`.webp` select.
    pub(super) format: Rc<Choice>,
    summary: gtk::Label,
    chevron: Icon,
    size: Rc<Choice>,
    dimensions: gtk::Label,
    custom: gtk::Box,
    custom_width: NumberField,
    custom_height: NumberField,
    quality: Rc<Choice>,
    preset_box: gtk::Box,
    preset: Rc<Choice>,
    maximum_box: gtk::Box,
    estimate: gtk::Label,
    delta: gtk::Label,
    /// Shipping's Comparison · Show before / after, while it is hidden.
    show_box: gtk::Box,
    pub(super) show_compare: gtk::Button,
    model: Rc<Model>,
}

/// The last options and document size the bar showed, and UI-only fields.
struct Model {
    options: Cell<Option<ExportOptions>>,
    document: Cell<(u32, u32)>,
    custom: Cell<(u32, u32)>,
    locked: Cell<bool>,
    unit: Cell<FileSizeUnit>,
    limit: RefCell<String>,
    syncing: Cell<bool>,
    changed: Changed,
}

impl Model {
    fn edit(&self, change: impl FnOnce(&mut ExportOptions)) {
        if self.syncing.get() {
            return;
        }
        let Some(mut options) = self.options.get() else {
            return;
        };
        change(&mut options);
        self.options.set(Some(options));
        (self.changed)(options);
    }

    fn limit_bytes(&self) -> u64 {
        self.unit.get().bytes(&self.limit.borrow()).unwrap_or(0)
    }
}

const fn unit_digits(unit: FileSizeUnit) -> u32 {
    match unit {
        FileSizeUnit::Kb => 0,
        FileSizeUnit::Mb => 2,
        FileSizeUnit::Gb => 5,
    }
}

fn control(caption: &str, class: &str, child: &impl IsA<gtk::Widget>) -> gtk::Box {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 8);
    column.add_css_class("export-control");
    column.add_css_class(class);
    column.set_valign(gtk::Align::End);
    // Fixed shipping widths: a select's caption must not stretch the panel.
    column.set_hexpand(false);
    let label = gtk::Label::new(Some(caption));
    label.add_css_class("export-caption");
    label.set_xalign(0.);
    label.set_accessible_role(gtk::AccessibleRole::Presentation);
    column.append(&label);
    column.append(child);
    column
}

fn scaled(value: u32, numerator: u32, denominator: u32) -> u32 {
    ((u64::from(value) * u64::from(numerator) + u64::from(denominator) / 2)
        / u64::from(denominator.max(1)))
    .clamp(1, MAX_DIMENSION as u64) as u32
}

impl ExportBar {
    /// Shipping's `max-width: 1040px` control widths.
    pub(super) fn set_compact(&self, compact: bool) {
        self.preset_box
            .set_size_request(if compact { 136 } else { 148 }, -1);
        self.maximum_box
            .set_size_request(if compact { 176 } else { 212 }, -1);
    }

    pub(super) fn new(changed: impl Fn(ExportOptions) + 'static) -> Rc<Self> {
        let model = Rc::new(Model {
            options: Cell::new(None),
            document: Cell::new((1, 1)),
            custom: Cell::new((1_920, 1_080)),
            locked: Cell::new(true),
            unit: Cell::new(FileSizeUnit::Mb),
            limit: RefCell::new(FileSizeUnit::Mb.value(DEFAULT_MAX_SIZE_BYTES)),
            syncing: Cell::new(false),
            changed: Rc::new(changed),
        });
        let panel = gtk::Box::new(gtk::Orientation::Horizontal, 16);
        panel.add_css_class("export-settings");
        panel.set_visible(false);
        panel.update_property(&[gtk::accessible::Property::Label("Export settings")]);

        // Output size.
        let size = {
            let model = model.clone();
            Choice::new(None, "Output size", false, move |id| {
                let document = model.document.get();
                if id == "custom"
                    && !matches!(
                        model.options.get().map(|o| o.size),
                        Some(ExportSize::Custom { .. })
                    )
                {
                    model.custom.set(document);
                }
                let (width, height) = model.custom.get();
                model.edit(|options| {
                    options.size = match id.as_str() {
                        "75" => ExportSize::Percent { percent: 75 },
                        "50" => ExportSize::Percent { percent: 50 },
                        "custom" => ExportSize::Custom { width, height },
                        _ => ExportSize::Original,
                    };
                });
            })
        };
        size.set_described(
            [
                (
                    "original",
                    "Original",
                    "Keep the capture’s pixel dimensions.",
                ),
                ("75", "75%", "Save at 75% of the pixel width and height."),
                ("50", "50%", "Save at half the pixel width and height."),
                ("custom", "Custom", "Choose exact pixel dimensions."),
            ]
            .map(|(id, label, description)| [id.into(), label.into(), description.into()])
            .into(),
        );
        size.root.set_size_request(108, -1);
        size.root.set_hexpand(false);
        let dimensions = gtk::Label::new(None);
        dimensions.add_css_class("export-dimensions");
        let size_row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        size_row.append(&size.root);
        size_row.append(&dimensions);
        panel.append(&control("Output size", "export-size", &size_row));

        // Custom width × height with the aspect lock.
        let custom_field = |name: &str, width: bool| {
            let model = model.clone();
            NumberField::new(name, (1., MAX_DIMENSION), move |value| {
                if model.syncing.get() {
                    return;
                }
                let value = value.round().clamp(1., MAX_DIMENSION) as u32;
                let (doc_w, doc_h) = model.document.get();
                let (mut w, mut h) = model.custom.get();
                if width {
                    w = value;
                    if model.locked.get() {
                        h = scaled(value, doc_h, doc_w);
                    }
                } else {
                    h = value;
                    if model.locked.get() {
                        w = scaled(value, doc_w, doc_h);
                    }
                }
                model.custom.set((w, h));
                model.edit(|options| {
                    options.size = ExportSize::Custom {
                        width: w,
                        height: h,
                    };
                });
            })
        };
        let custom_width = custom_field("Custom output width", true);
        let custom_height = custom_field("Custom output height", false);
        let lock = gtk::ToggleButton::new();
        crate::widgets::action_cursor(&lock);
        lock.add_css_class("export-lock");
        lock.set_active(true);
        lock.add_css_class("active");
        let lock_icon = Icon::new("lock", 14).with_stroke(1.8);
        lock.set_child(Some(&lock_icon));
        lock.set_tooltip_text(Some("Lock output aspect ratio"));
        lock.update_property(&[gtk::accessible::Property::Label("Lock output aspect ratio")]);
        {
            let model = model.clone();
            lock.connect_toggled(move |lock| {
                let on = lock.is_active();
                model.locked.set(on);
                lock_icon.set_name(if on { "lock" } else { "unlock" });
                if on {
                    lock.add_css_class("active");
                } else {
                    lock.remove_css_class("active");
                }
            });
        }
        let custom_row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        custom_width.root.set_hexpand(true);
        custom_height.root.set_hexpand(true);
        custom_row.append(&custom_width.root);
        let times = gtk::Label::new(Some("×"));
        times.add_css_class("export-times");
        times.set_accessible_role(gtk::AccessibleRole::Presentation);
        custom_row.append(&times);
        custom_row.append(&custom_height.root);
        custom_row.append(&lock);
        let custom = control("Width × height", "export-custom", &custom_row);
        custom.set_size_request(268, -1);
        custom.set_visible(false);
        panel.append(&custom);

        // Save quality.
        let quality = {
            let model = model.clone();
            Choice::new(None, "Save quality", false, move |id| {
                let index: usize = id.parse().unwrap_or(0);
                let mode = editor_export::SAVE_QUALITY_MODES[index.min(2)];
                let bytes = model.limit_bytes();
                model.edit(|options| {
                    options.quality = mode;
                    options.max_size_bytes = (mode == ExportQuality::Maximum).then_some(bytes);
                });
            })
        };
        let quality_box = control("Save quality", "export-quality-mode", &quality.root);
        quality_box.set_size_request(188, -1);
        panel.append(&quality_box);

        // Compress preset.
        let preset = {
            let model = model.clone();
            Choice::new(None, "Compression quality", false, move |id| {
                let Ok(value) = id.parse::<u8>() else { return };
                model.edit(|options| {
                    options.quality_value = value;
                    // Shared encoding owns PNG palette selection.
                    options.png.max_colors = None;
                });
            })
        };
        let preset_box = control("Quality", "export-quality", &preset.root);
        preset_box.set_size_request(148, -1);
        preset_box.set_visible(false);
        panel.append(&preset_box);

        // Maximum file size and unit.
        let maximum = {
            let model = model.clone();
            NumberField::new("Maximum file size", (0.01, 1_000_000.), move |value| {
                if model.syncing.get() {
                    return;
                }
                let unit = model.unit.get();
                *model.limit.borrow_mut() = format!("{value:.*}", unit_digits(unit) as usize);
                let bytes = model.limit_bytes();
                model.edit(|options| options.max_size_bytes = Some(bytes));
            })
        };
        {
            // Shipping shows "10", not the spin button's fixed "10.00".
            let model = model.clone();
            maximum.spin.connect_output(move |spin| {
                let text = format!(
                    "{:.*}",
                    unit_digits(model.unit.get()) as usize,
                    spin.value()
                );
                let text = if text.contains('.') {
                    text.trim_end_matches('0').trim_end_matches('.')
                } else {
                    &text
                };
                spin.set_text(text);
                gtk::glib::Propagation::Stop
            });
        }
        let unit = {
            let model = model.clone();
            let spin = maximum.spin.clone();
            Choice::new(None, "Screenshot file size unit", false, move |id| {
                let next = FileSizeUnit::ALL
                    .into_iter()
                    .find(|unit| unit.label() == id)
                    .unwrap_or_default();
                let bytes = model.limit_bytes();
                model.unit.set(next);
                *model.limit.borrow_mut() = next.value(bytes);
                let text = model.limit.borrow().clone();
                model.syncing.set(true);
                configure_limit(&spin, next, &text);
                model.syncing.set(false);
            })
        };
        unit.set_options(
            FileSizeUnit::ALL
                .map(|unit| (unit.label().to_owned(), unit.label().to_owned()))
                .into(),
        );
        unit.set(FileSizeUnit::Mb.label());
        unit.root.set_size_request(72, -1);
        let text = model.limit.borrow().clone();
        model.syncing.set(true);
        configure_limit(&maximum.spin, FileSizeUnit::Mb, &text);
        model.syncing.set(false);
        let maximum_row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        maximum.root.set_hexpand(true);
        maximum_row.append(&maximum.root);
        maximum_row.append(&unit.root);
        let maximum_box = control("Maximum file size", "export-maximum", &maximum_row);
        maximum_box.set_size_request(212, -1);
        maximum_box.set_visible(false);
        panel.append(&maximum_box);

        // Est. size and its change versus the original.
        let estimate = gtk::Label::new(Some("—"));
        estimate.add_css_class("export-estimate");
        estimate.set_tooltip_text(Some(
            "Estimated export file size for the current format, quality, and output size",
        ));
        let delta = gtk::Label::new(None);
        delta.add_css_class("export-delta");
        delta.set_tooltip_text(Some("Change versus the original image, before this export"));
        let estimate_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        estimate_row.add_css_class("export-estimate-row");
        estimate_row.append(&estimate);
        estimate_row.append(&delta);
        panel.append(&control(
            "Est. size",
            "export-estimate-control",
            &estimate_row,
        ));
        let show_compare = gtk::Button::with_label(compression_compare::SHOW);
        crate::widgets::action_cursor(&show_compare);
        show_compare.add_css_class("export-show-compare");
        let show_box = control(
            compression_compare::SHOW_CAPTION,
            "export-compare-control",
            &show_compare,
        );
        show_box.set_visible(false);
        panel.append(&show_box);

        // `.screenshot-export-disclosure`.
        let disclosure = gtk::Button::new();
        crate::widgets::action_cursor(&disclosure);
        disclosure.add_css_class("export-disclosure");
        let copy = gtk::Box::new(gtk::Orientation::Vertical, 2);
        copy.set_hexpand(true);
        let title = gtk::Label::new(Some("Export settings"));
        title.add_css_class("export-disclosure-label");
        title.set_xalign(0.);
        let summary = gtk::Label::new(None);
        summary.add_css_class("export-summary");
        summary.set_xalign(0.);
        summary.set_ellipsize(gtk::pango::EllipsizeMode::End);
        summary.set_width_chars(1);
        copy.append(&title);
        copy.append(&summary);
        let chevron = Icon::new("chevron-down", 15).with_stroke(1.8);
        chevron.set_valign(gtk::Align::Center);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.append(&copy);
        row.append(&chevron);
        disclosure.set_child(Some(&row));
        disclosure.update_property(&[gtk::accessible::Property::Label("Export settings")]);
        disclosure.update_state(&[gtk::accessible::State::Expanded(Some(false))]);

        let format = {
            let model = model.clone();
            Choice::new(None, "Format", false, move |id| {
                let format = match id.as_str() {
                    "jpeg" => ExportFormat::Jpeg,
                    "webp" => ExportFormat::Webp,
                    _ => ExportFormat::Png,
                };
                model.edit(|options| options.format = format);
            })
        };
        format.root.add_css_class("filename-format-select");
        format.root.set_hexpand(false);
        format.fill_height();

        let bar = Rc::new(ExportBar {
            panel,
            disclosure,
            format,
            summary,
            chevron,
            size,
            dimensions,
            custom,
            custom_width,
            custom_height,
            quality,
            preset_box,
            preset,
            maximum_box,
            estimate,
            delta,
            show_box,
            show_compare,
            model,
        });
        let weak = Rc::downgrade(&bar);
        bar.disclosure.connect_clicked(move |_| {
            if let Some(bar) = weak.upgrade() {
                bar.set_open(!bar.panel.is_visible());
            }
        });
        bar
    }

    pub(super) fn set_open(&self, open: bool) {
        self.panel.set_visible(open);
        self.chevron
            .set_name(if open { "chevron-up" } else { "chevron-down" });
        self.disclosure
            .update_state(&[gtk::accessible::State::Expanded(Some(open))]);
    }

    /// Show `view`, computed by `editor_export::present` for `options`.
    pub(super) fn sync(
        &self,
        options: ExportOptions,
        view: &ExportBarView,
        document: (u32, u32),
        transparent: bool,
        pending: bool,
    ) {
        let model = &self.model;
        model.syncing.set(true);
        model.options.set(Some(options));
        model.document.set(document);
        self.summary.set_label(&view.summary);
        self.disclosure.set_tooltip_text(Some(&view.summary));
        self.size.set(match options.size {
            ExportSize::Original => "original",
            ExportSize::Percent { percent: 75 } => "75",
            ExportSize::Percent { .. } => "50",
            ExportSize::Custom { .. } => "custom",
        });
        self.dimensions.set_label(
            &view
                .dimensions
                .map_or_else(|| "—".to_owned(), |(w, h)| format!("{w} × {h}")),
        );
        let custom = matches!(options.size, ExportSize::Custom { .. });
        self.custom.set_visible(custom);
        if let ExportSize::Custom { width, height } = options.size {
            self.custom_width.set(f64::from(width), false);
            self.custom_height.set(f64::from(height), false);
        }
        let described = |choices: &[editor_export::Choice]| {
            choices
                .iter()
                .map(|c| [c.value.to_string(), c.label.clone(), c.description.clone()])
                .collect::<Vec<_>>()
        };
        self.quality.set_described(described(&view.quality_modes));
        let mode = editor_export::SAVE_QUALITY_MODES
            .iter()
            .position(|mode| *mode == options.quality)
            .unwrap_or(0);
        self.quality.set(&mode.to_string());
        self.preset_box
            .set_visible(options.quality == ExportQuality::Compress);
        self.preset.set_described(described(&view.quality_presets));
        self.preset.set(&options.quality_value.to_string());
        self.preset.set_caption(
            editor_export::quality_preset(options.quality_value)
                .is_none()
                .then_some("Custom"),
        );
        self.maximum_box
            .set_visible(options.quality == ExportQuality::Maximum);
        self.maximum_box.set_tooltip_text(Some(&view.maximum_help));
        self.estimate.set_label(&view.estimate_label);
        set_class(&self.estimate, "pending", pending);
        match &view.delta {
            Some(delta) => {
                self.delta.set_label(&delta.label);
                self.delta.set_visible(true);
                set_class(&self.delta, "smaller", delta.percent < 0);
                set_class(&self.delta, "larger", delta.percent >= 0);
            }
            None => self.delta.set_visible(false),
        }
        self.format.set_described(vec![
            ["png".into(), "PNG".into(), String::new()],
            [
                "jpeg".into(),
                "JPEG".into(),
                if transparent {
                    "Fills in transparent areas.".into()
                } else {
                    String::new()
                },
            ],
            ["webp".into(), "WebP".into(), String::new()],
        ]);
        self.format.set(match options.format {
            ExportFormat::Png => "png",
            ExportFormat::Jpeg => "jpeg",
            ExportFormat::Webp => "webp",
        });
        self.format.set_caption(Some(&view.suffix));
        model.syncing.set(false);
    }

    pub(super) fn set_sensitive(&self, on: bool) {
        self.format.set_sensitive(on);
    }

    pub(super) fn is_open(&self) -> bool {
        self.panel.is_visible()
    }

    pub(super) fn set_show_compare(&self, visible: bool) {
        self.show_box.set_visible(visible);
    }
}

/// The size-limit field's minimum, step and decimals for `unit`.
fn configure_limit(spin: &gtk::SpinButton, unit: FileSizeUnit, text: &str) {
    spin.set_digits(unit_digits(unit));
    spin.set_range(unit.minimum(), 1_000_000.);
    spin.set_increments(unit.step(), unit.step() * 10.);
    spin.set_value(text.parse().unwrap_or(unit.minimum()));
}

fn set_class(widget: &impl IsA<gtk::Widget>, class: &str, on: bool) {
    if on {
        widget.add_css_class(class);
    } else {
        widget.remove_css_class(class);
    }
}
