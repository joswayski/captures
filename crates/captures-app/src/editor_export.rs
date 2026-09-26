//! Host-independent model for the screenshot editor's bottom export bar.
//!
//! Mirrors the shipping editor's save model: Save overwrites the source by
//! default when the screenshot has a saved file, a "Save as new file" switch
//! keeps the original, changing the filename or folder turns that switch on, and
//! a format that does not match the source always saves a new file. Hosts own
//! widgets; this module owns the target state, summary/hint copy, file-size
//! formatting, the size estimate and the dispatch to the publication routines.

use std::{
    fs,
    path::{Path, PathBuf},
};

use captures_capture::CaptureMode;
use captures_image::{ExportFormat, ExportOptions, ExportQuality};
use image::RgbaImage;
use serde::{Deserialize, Serialize};

use crate::editor_output::{self, SavedExport};

/// Maximum file size limits below this are rejected, matching the shipping editor.
pub const MINIMUM_MAX_SIZE_BYTES: u64 = 10_000;

const FALLBACK_STEM: &str = "Captures_screenshot";

/// A saved screenshot file that Save may overwrite, and its History entry.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExportSource {
    pub artifact_id: String,
    pub path: PathBuf,
}

/// Where Save writes: the folder, the filename stem (the format owns the
/// suffix) and whether it keeps the source untouched.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExportTarget {
    pub source: Option<ExportSource>,
    #[serde(default)]
    pub source_missing: bool,
    pub directory: PathBuf,
    pub stem: String,
    pub save_as_new: bool,
}

/// The one publication Save performs for the current target and format.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SavePlan {
    /// Replace the source file and update that same History entry.
    Overwrite { artifact_id: String, path: PathBuf },
    /// Publish a new file and History entry; existing files are never replaced.
    NewFile { path: PathBuf },
}

impl SavePlan {
    pub fn path(&self) -> &Path {
        match self {
            Self::Overwrite { path, .. } | Self::NewFile { path } => path,
        }
    }

    pub const fn overwrites(&self) -> bool {
        matches!(self, Self::Overwrite { .. })
    }
}

impl ExportTarget {
    /// A saved source starts in overwrite mode beside that file. Without one,
    /// the first Save writes a new file in the default folder.
    pub fn new(source: Option<ExportSource>, default_directory: &Path, default_stem: &str) -> Self {
        match source {
            Some(source) => Self {
                source_missing: !source.path.is_file(),
                directory: parent_directory(&source.path)
                    .unwrap_or_else(|| default_directory.into()),
                stem: file_stem(&source.path),
                save_as_new: false,
                source: Some(source),
            },
            None => Self {
                source: None,
                source_missing: false,
                directory: default_directory.into(),
                stem: default_stem.into(),
                save_as_new: true,
            },
        }
    }

    fn source_stem(&self) -> Option<String> {
        self.source.as_ref().map(|source| file_stem(&source.path))
    }

    fn source_directory(&self) -> Option<PathBuf> {
        self.source
            .as_ref()
            .and_then(|source| parent_directory(&source.path))
    }

    fn differs_from_source(&self) -> bool {
        self.source.is_some()
            && (self.source_stem().as_deref() != Some(self.stem.as_str())
                || self.source_directory().as_deref() != Some(self.directory.as_path()))
    }

    /// Editing the filename away from the source turns "Save as new file" on.
    pub fn set_stem(&mut self, stem: impl Into<String>) {
        self.stem = stem.into();
        if self.differs_from_source() {
            self.save_as_new = true;
        }
    }

    /// Choosing a folder other than the source's turns "Save as new file" on.
    pub fn set_directory(&mut self, directory: impl Into<PathBuf>) {
        self.directory = directory.into();
        if self.differs_from_source() {
            self.save_as_new = true;
        }
    }

    /// The switch cannot be changed while the format forces a copy. Turning it
    /// on beside the source suggests an "-edited" name; turning it off restores
    /// the source name and folder when the suggestion is still in place.
    pub fn set_save_as_new(&mut self, enabled: bool, format: ExportFormat) {
        if self.format_requires_copy(format) {
            return;
        }
        let (Some(stem), Some(directory)) = (self.source_stem(), self.source_directory()) else {
            self.save_as_new = enabled;
            return;
        };
        self.save_as_new = enabled;
        if enabled && self.stem == stem && self.directory == directory {
            self.stem = edited_stem(&stem);
        } else if !enabled
            && (self.stem == edited_stem(&stem) || self.stem == format!("{stem}-copy"))
        {
            self.stem = stem;
            self.directory = directory;
        }
    }

    /// No saved source, a missing source, or a different format than the
    /// source file always writes a new file.
    pub fn format_requires_copy(&self, format: ExportFormat) -> bool {
        self.source_missing
            || self
                .source
                .as_ref()
                .is_none_or(|source| !path_matches_format(&source.path, format))
    }

    pub fn saving_copy(&self, format: ExportFormat) -> bool {
        self.save_as_new || self.format_requires_copy(format)
    }

    /// Filename suffix without the dot. JPEG keeps a `.jpeg` source spelling.
    pub fn extension(&self, format: ExportFormat) -> &'static str {
        match format {
            ExportFormat::Png => "png",
            ExportFormat::Webp => "webp",
            ExportFormat::Jpeg
                if self.source.as_ref().is_some_and(|source| {
                    source
                        .path
                        .extension()
                        .and_then(|value| value.to_str())
                        .is_some_and(|value| value.eq_ignore_ascii_case("jpeg"))
                }) =>
            {
                "jpeg"
            }
            ExportFormat::Jpeg => "jpg",
        }
    }

    pub fn destination(&self, format: ExportFormat) -> PathBuf {
        self.directory
            .join(format!("{}.{}", self.stem, self.extension(format)))
    }

    /// Resolve Save into exactly one publication, or explain what to fix.
    pub fn plan(&self, format: ExportFormat) -> Result<SavePlan, String> {
        if let Some(error) = filename_error(&self.stem) {
            return Err(error.into());
        }
        if self.directory.as_os_str().is_empty() {
            return Err("Choose a save location.".into());
        }
        let path = self.destination(format);
        match &self.source {
            Some(source) if !self.saving_copy(format) && source.path == path => {
                Ok(SavePlan::Overwrite {
                    artifact_id: source.artifact_id.clone(),
                    path,
                })
            }
            _ => Ok(SavePlan::NewFile { path }),
        }
    }

    /// Adopt a saved file as the new source, as the shipping editor does: the
    /// next Save overwrites it. A file without a History entry cannot be safely
    /// overwritten later, so that result keeps the current target.
    pub fn record_saved(&mut self, saved: &SavedExport) {
        if let SavedExport::Saved { path, artifact } = saved {
            self.adopt(ExportSource {
                artifact_id: artifact.entry.id.clone(),
                path: path.clone(),
            });
        }
    }

    /// Make a saved file with a History entry the new overwrite source.
    pub fn adopt(&mut self, source: ExportSource) {
        *self = Self::new(Some(source), &self.directory, &self.stem);
    }
}

/// Publish the edited pixels exactly as planned. Overwrites re-validate the
/// History entry and saved path from disk; new files never replace anything.
pub fn publish(
    history_root: &Path,
    image: &RgbaImage,
    plan: &SavePlan,
    options: ExportOptions,
    mode: CaptureMode,
) -> Result<SavedExport, String> {
    validate_options(options, image.width(), image.height())?;
    match plan {
        SavePlan::Overwrite { artifact_id, path } => {
            editor_output::save_original_export(history_root, artifact_id, path, image, options)
                .map_err(|error| error.to_string())
        }
        SavePlan::NewFile { path } => {
            editor_output::save_new_export(history_root, image, path, options, mode).map_err(
                |error| match error {
                    editor_output::Error::Io(io)
                        if io.kind() == std::io::ErrorKind::AlreadyExists =>
                    {
                        format!(
                            "{} already exists. Choose another filename.",
                            path.file_name()
                                .map(|name| name.to_string_lossy().into_owned())
                                .unwrap_or_else(|| path.display().to_string())
                        )
                    }
                    error => error.to_string(),
                },
            )
        }
    }
}

/// Output options Save and the estimate can encode, or the fix to show.
pub fn validate_options(
    options: ExportOptions,
    width: u32,
    height: u32,
) -> Result<(u32, u32), String> {
    let dimensions = options.size.dimensions(width, height)?;
    if options.quality == ExportQuality::Maximum
        && options
            .max_size_bytes
            .is_none_or(|bytes| bytes < MINIMUM_MAX_SIZE_BYTES)
    {
        return Err("Enter a maximum file size of at least 10 KB.".into());
    }
    Ok(dimensions)
}

/// Transient estimate state a host keeps while it re-encodes in the background.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct EstimateState {
    pub bytes: Option<u64>,
    pub baseline_bytes: Option<u64>,
    #[serde(default)]
    pub pending: bool,
}

/// Encoded bytes plus the "original" size the % change compares against.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExportEstimate {
    pub bytes: u64,
    pub baseline_bytes: Option<u64>,
}

/// Encode exactly as Save would, without I/O. Preserve compares with the
/// original file; compression modes compare with the lossless flattened image.
pub fn estimate_export(
    image: &RgbaImage,
    options: ExportOptions,
    original_bytes: Option<u64>,
) -> Result<ExportEstimate, String> {
    validate_options(options, image.width(), image.height())?;
    let bytes = captures_image::encode_export(image, options)?.len() as u64;
    let baseline_bytes = if options.quality == ExportQuality::Preserve {
        original_bytes.filter(|bytes| *bytes > 0)
    } else {
        Some(
            captures_history::encode_png(image)
                .map_err(|error| error.to_string())?
                .len() as u64,
        )
    };
    Ok(ExportEstimate {
        bytes,
        baseline_bytes,
    })
}

/// The History entry's recorded file size, used as the Preserve baseline.
pub fn original_size_bytes(history_root: &Path, artifact_id: &str) -> Option<u64> {
    let directory = captures_history::entry_directory(history_root, artifact_id).ok()?;
    let metadata = fs::read(directory.join(captures_history::HISTORY_METADATA_FILE)).ok()?;
    let entry: captures_history::HistoryEntry = serde_json::from_slice(&metadata).ok()?;
    (entry.size_bytes > 0).then_some(entry.size_bytes)
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SizeDelta {
    pub percent: i64,
    pub label: String,
}

/// Everything the export bar shows, computed identically for every host.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExportBarView {
    pub format_label: &'static str,
    pub suffix: String,
    pub dimensions: Option<(u32, u32)>,
    pub summary: String,
    pub estimate_label: String,
    pub delta: Option<SizeDelta>,
    pub hint: String,
    pub hint_warning: bool,
    pub format_requires_copy: bool,
    pub saving_copy: bool,
    pub plan: Option<SavePlan>,
    /// Filename, folder or option problems; Save stays disabled until fixed.
    pub error: Option<String>,
    /// Save quality listbox rows for the current format.
    pub quality_modes: Vec<Choice>,
    /// Compress preset rows with their per-format descriptions.
    pub quality_presets: Vec<Choice>,
    /// Hover help on Maximum file size for the current format.
    pub maximum_help: String,
    /// Compress or Maximum: the automatic before/after comparison applies.
    pub compresses: bool,
}

/// One listbox row: a label, its `<small>` description and the value it sets
/// (a quality value for presets, the mode's index for Save quality).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Choice {
    pub label: String,
    pub description: String,
    pub value: u32,
}

pub fn present(
    target: &ExportTarget,
    options: ExportOptions,
    document_size: (u32, u32),
    transparent_background: bool,
    estimate: EstimateState,
) -> ExportBarView {
    let format = options.format;
    let validated = validate_options(options, document_size.0, document_size.1);
    let dimensions = options
        .size
        .dimensions(document_size.0, document_size.1)
        .ok();
    let estimate_label = estimate_label(&estimate, options);
    let capped = estimate_is_cap(&estimate, options);
    let delta = if capped || estimate.pending {
        None
    } else {
        size_delta(estimate.bytes, estimate.baseline_bytes)
    };
    let jpeg_drops_transparency = format == ExportFormat::Jpeg && transparent_background;
    let plan = target.plan(format);
    let dimensions_label = dimensions.map_or_else(
        || "invalid size".to_owned(),
        |(width, height)| format!("{width} × {height}"),
    );
    ExportBarView {
        format_label: format_label(format),
        suffix: format!(".{}", target.extension(format)),
        dimensions,
        summary: format!(
            "{} · {dimensions_label} · {estimate_label}",
            format_label(format)
        ),
        estimate_label,
        delta,
        hint: save_hint(target, options, jpeg_drops_transparency),
        hint_warning: jpeg_drops_transparency,
        format_requires_copy: target.format_requires_copy(format),
        saving_copy: target.saving_copy(format),
        error: validated.err().or_else(|| plan.as_ref().err().cloned()),
        plan: plan.ok(),
        quality_modes: SAVE_QUALITY_MODES
            .iter()
            .enumerate()
            .map(|(index, mode)| Choice {
                label: quality_mode_label(*mode).into(),
                description: quality_mode_description(*mode, format).into(),
                value: index as u32,
            })
            .collect(),
        quality_presets: QUALITY_PRESETS
            .iter()
            .map(|preset| Choice {
                label: preset.label.into(),
                description: preset.description(format).into(),
                value: u32::from(preset.quality),
            })
            .collect(),
        maximum_help: maximum_size_help(format),
        compresses: options.quality != ExportQuality::Preserve,
    }
}

/// Status copy after a successful save.
pub fn saved_notice(plan: &SavePlan, saved: &SavedExport) -> String {
    match saved {
        SavedExport::Saved { .. } if plan.overwrites() => "Saved changes to the original".into(),
        SavedExport::Saved { path, .. } => format!("Saved {}", path.display()),
        SavedExport::SavedWithoutHistory { path, warning } => {
            format!(
                "Saved {}. History was not updated: {warning}",
                path.display()
            )
        }
    }
}

pub const fn format_label(format: ExportFormat) -> &'static str {
    match format {
        ExportFormat::Png => "PNG",
        ExportFormat::Jpeg => "JPEG",
        ExportFormat::Webp => "WebP",
    }
}

/// Footer copy next to Save: first write, overwrite, or a separate file.
pub fn save_hint(
    target: &ExportTarget,
    options: ExportOptions,
    jpeg_drops_transparency: bool,
) -> String {
    let format = format_label(options.format);
    if target.source_missing {
        return "The original was deleted. You can still copy or save this edit.".into();
    }
    if jpeg_drops_transparency {
        return "JPEG will fill in transparent areas. Use PNG or WebP to keep them.".into();
    }
    let first_save = target.source.is_none();
    let copy = target.saving_copy(options.format);
    match options.quality {
        ExportQuality::Preserve if first_save => {
            format!("Save writes a {format} at original quality.")
        }
        ExportQuality::Preserve if copy => format!(
            "Save writes a new {format} at original quality and leaves the original untouched."
        ),
        ExportQuality::Preserve => {
            format!("Save keeps original quality as {format} and overwrites the original.")
        }
        ExportQuality::Maximum if options.format == ExportFormat::Jpeg => {
            if first_save {
                "Save writes a JPEG within the selected limit.".into()
            } else if copy {
                "Save writes a new JPEG within the selected limit and leaves the original untouched."
                    .into()
            } else {
                "Save writes a JPEG within the selected limit and overwrites the original.".into()
            }
        }
        ExportQuality::Maximum if first_save => {
            format!("Save writes a {format} within the selected size limit.")
        }
        ExportQuality::Maximum if copy => format!(
            "Save writes a new {format} within the selected size limit and leaves the original untouched."
        ),
        ExportQuality::Maximum => format!(
            "Save writes a {format} within the selected size limit and overwrites the original."
        ),
        ExportQuality::Compress if first_save => format!("Save writes a compressed {format}."),
        ExportQuality::Compress if copy => {
            format!("Save writes a compressed {format} and leaves the original untouched.")
        }
        ExportQuality::Compress => format!(
            "Save overwrites the original with compressed {format}. Turn on Save as new file to keep it."
        ),
    }
}

fn estimate_is_cap(estimate: &EstimateState, options: ExportOptions) -> bool {
    options.quality == ExportQuality::Maximum
        && matches!(options.format, ExportFormat::Jpeg | ExportFormat::Webp)
        && options.max_size_bytes.is_some_and(|maximum| {
            maximum >= MINIMUM_MAX_SIZE_BYTES && estimate.bytes.is_some_and(|bytes| bytes > maximum)
        })
}

/// "≈ 240 KB", "≤ 1.0 MB" for an unreachable JPEG/WebP cap, "Estimating…" or "—".
pub fn estimate_label(estimate: &EstimateState, options: ExportOptions) -> String {
    match estimate.bytes {
        None if estimate.pending => "Estimating…".into(),
        None => "—".into(),
        Some(_) if estimate_is_cap(estimate, options) => {
            format!(
                "≤ {}",
                format_file_size(options.max_size_bytes.unwrap_or_default())
            )
        }
        Some(bytes) => format!("≈ {}", format_file_size(bytes)),
    }
}

/// Decimal units with one decimal below 100, matching the shipping app.
pub fn format_file_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    if bytes == 0 {
        return "0 B".into();
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1_000. && unit < UNITS.len() - 1 {
        value /= 1_000.;
        unit += 1;
    }
    if unit == 0 {
        return format!("{} {}", value.round(), UNITS[unit]);
    }
    if value >= 100. {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Percent change from the original to the estimate; `None` when unknown or 0%.
pub fn size_delta(estimated: Option<u64>, baseline: Option<u64>) -> Option<SizeDelta> {
    let (estimated, baseline) = (estimated?, baseline.filter(|value| *value > 0)?);
    let percent = ((estimated as f64 / baseline as f64 - 1.) * 100.).round() as i64;
    (percent != 0).then(|| SizeDelta {
        percent,
        label: if percent < 0 {
            format!("−{}%", percent.unsigned_abs())
        } else {
            format!("+{percent}%")
        },
    })
}

pub fn path_matches_format(path: &Path, format: ExportFormat) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|extension| editor_output::extension_matches(format, extension))
}

pub fn file_stem(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| FALLBACK_STEM.into())
}

fn parent_directory(path: &Path) -> Option<PathBuf> {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(Path::to_owned)
}

/// Suggested name for "Save as new file" beside the source.
pub fn edited_stem(stem: &str) -> String {
    let trimmed = stem.trim();
    if trimmed.is_empty() {
        return format!("{FALLBACK_STEM}-edited");
    }
    if trimmed.ends_with("-edited") || trimmed.ends_with("-copy") {
        return trimmed.into();
    }
    format!("{trimmed}-edited")
}

/// Reject names that are empty, padded, contain folders or reserved characters.
pub fn filename_error(stem: &str) -> Option<&'static str> {
    let trimmed = stem.trim();
    let upper = trimmed.to_ascii_uppercase();
    let base = upper.split('.').next().unwrap_or_default();
    let reserved = matches!(base, "CON" | "PRN" | "AUX" | "NUL")
        || ((base.starts_with("COM") || base.starts_with("LPT"))
            && base.len() == 4
            && matches!(base.as_bytes()[3], b'1'..=b'9'));
    let invalid = trimmed.is_empty()
        || trimmed != stem
        || trimmed == "."
        || trimmed == ".."
        || trimmed
            .chars()
            .any(|character| character < ' ' || "<>:\"/\\|?*".contains(character))
        || trimmed.ends_with('.')
        || trimmed.ends_with(' ')
        || reserved;
    invalid.then_some("Enter a filename without folders or reserved characters.")
}

/// One shipping Compress preset (`SCREENSHOT_QUALITY_OPTIONS`). JPEG and WebP
/// use it as encode quality; PNG maps Tiny–High onto palette size and Highest
/// onto compact lossless packing only.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct QualityPreset {
    pub label: &'static str,
    pub quality: u8,
    jpeg: &'static str,
    webp: &'static str,
    png: &'static str,
}

impl QualityPreset {
    /// The listbox description for `format`.
    pub const fn description(&self, format: ExportFormat) -> &'static str {
        match format {
            ExportFormat::Jpeg => self.jpeg,
            ExportFormat::Webp => self.webp,
            ExportFormat::Png => self.png,
        }
    }
}

pub const QUALITY_PRESETS: [QualityPreset; 5] = [
    QualityPreset {
        label: "Tiny",
        quality: 55,
        jpeg: "Smallest file with the most visible compression.",
        webp: "Smallest lossy WebP with the most visible compression.",
        png: "Smallest PNG with the most visible dithering.",
    },
    QualityPreset {
        label: "Smaller",
        quality: 70,
        jpeg: "Very small file with more visible compression.",
        webp: "Very small lossy WebP with more visible compression.",
        png: "Very small PNG with more visible dithering.",
    },
    QualityPreset {
        label: "Balanced",
        quality: 85,
        jpeg: "Good quality with a meaningfully smaller file.",
        webp: "Good lossy WebP quality with a meaningfully smaller file.",
        png: "Good quality with a meaningfully smaller PNG.",
    },
    QualityPreset {
        label: "High",
        quality: 92,
        jpeg: "Much smaller file with little visible quality loss.",
        webp: "Much smaller lossy WebP with little visible quality loss.",
        png: "Much smaller PNG by reducing colors; usually looks similar.",
    },
    QualityPreset {
        label: "Highest",
        quality: 98,
        jpeg: "Light JPEG compression. Near-original quality, a modest size cut.",
        webp: "Light lossy WebP. Near-original quality, a modest size cut.",
        png: "Same pixels, tighter packing. No color reduction.",
    },
];

/// The preset whose quality is exactly `quality`, if any.
pub fn quality_preset(quality: u8) -> Option<&'static QualityPreset> {
    QUALITY_PRESETS
        .iter()
        .find(|preset| preset.quality == quality)
}

pub const SAVE_QUALITY_MODES: [ExportQuality; 3] = [
    ExportQuality::Preserve,
    ExportQuality::Compress,
    ExportQuality::Maximum,
];

pub const fn quality_mode_label(mode: ExportQuality) -> &'static str {
    match mode {
        ExportQuality::Preserve => "Preserve quality",
        ExportQuality::Compress => "Compress",
        ExportQuality::Maximum => "Maximum file size",
    }
}

/// The Save quality listbox description for `mode` and `format`.
pub const fn quality_mode_description(mode: ExportQuality, format: ExportFormat) -> &'static str {
    match (mode, format) {
        (ExportQuality::Preserve, _) => {
            "Original quality with no extra compression unless an edit requires it."
        }
        (ExportQuality::Compress, ExportFormat::Png) => {
            "Smaller PNG with Tiny through Highest quality presets."
        }
        (ExportQuality::Compress, ExportFormat::Webp) => {
            "Smaller lossy WebP with Tiny through Highest quality presets."
        }
        (ExportQuality::Compress, ExportFormat::Jpeg) => {
            "Smaller JPEG with Tiny through Highest quality presets."
        }
        (ExportQuality::Maximum, _) => "Set a hard size limit for the saved file.",
    }
}

/// Hover help on the Maximum file size control.
pub fn maximum_size_help(format: ExportFormat) -> String {
    let label = format_label(format);
    match format {
        ExportFormat::Jpeg | ExportFormat::Webp => format!(
            "{label} quality is lowered only when needed to meet this limit. If the original already fits, it stays uncompressed."
        ),
        ExportFormat::Png => {
            format!("Uses stronger {label} compression only when the original exceeds this limit.")
        }
    }
}

/// Decimal file-size units of the Maximum file size field (shipping
/// `SCREENSHOT_FILE_SIZE_UNIT_BYTES` and the recording editor's units).
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileSizeUnit {
    Kb,
    #[default]
    Mb,
    Gb,
}

impl FileSizeUnit {
    pub const ALL: [Self; 3] = [Self::Kb, Self::Mb, Self::Gb];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Kb => "KB",
            Self::Mb => "MB",
            Self::Gb => "GB",
        }
    }

    const fn digits(self) -> usize {
        match self {
            Self::Kb => 3,
            Self::Mb => 6,
            Self::Gb => 9,
        }
    }

    /// The screenshot field's minimum and stepper increment in this unit.
    pub const fn step(self) -> f64 {
        match self {
            Self::Kb => 1.,
            Self::Mb => 0.01,
            Self::Gb => 0.000_01,
        }
    }

    pub const fn minimum(self) -> f64 {
        match self {
            Self::Kb => 10.,
            Self::Mb => 0.01,
            Self::Gb => 0.000_01,
        }
    }

    /// Whole bytes for decimal `text` in this unit, floored without
    /// floating-point rounding; `None` for anything but digits and one dot.
    pub fn bytes(self, text: &str) -> Option<u64> {
        let value = text.trim();
        let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
        if whole.is_empty() && fraction.is_empty()
            || !whole
                .bytes()
                .chain(fraction.bytes())
                .all(|c| c.is_ascii_digit())
        {
            return None;
        }
        let whole = if whole.is_empty() {
            0
        } else {
            whole.parse::<u64>().ok()?
        };
        let fraction = &fraction[..fraction.len().min(self.digits())];
        let part = if fraction.is_empty() {
            0
        } else {
            fraction.parse::<u64>().ok()?
        };
        whole
            .checked_mul(10_u64.pow(self.digits() as u32))?
            .checked_add(part * 10_u64.pow((self.digits() - fraction.len()) as u32))
    }

    /// `bytes` written in this unit without trailing zeros ("10", "0.25").
    pub fn value(self, bytes: u64) -> String {
        let factor = 10_u64.pow(self.digits() as u32);
        let fraction = format!("{:0width$}", bytes % factor, width = self.digits());
        let fraction = fraction.trim_end_matches('0');
        if fraction.is_empty() {
            (bytes / factor).to_string()
        } else {
            format!("{}.{}", bytes / factor, fraction)
        }
    }
}

/// Shipping's screenshot Maximum file size default: 10 MB.
pub const DEFAULT_MAX_SIZE_BYTES: u64 = 10_000_000;

#[cfg(test)]
mod tests {
    use super::*;
    use captures_image::{ExportSize, PngOptions};
    use image::Rgba;

    fn options(format: ExportFormat, quality: ExportQuality) -> ExportOptions {
        ExportOptions {
            format,
            quality,
            quality_value: 85,
            max_size_bytes: None,
            png: PngOptions::default(),
            size: ExportSize::Original,
        }
    }

    fn sourced(dir: &Path, name: &str) -> ExportTarget {
        let path = dir.join(name);
        fs::write(&path, b"source").unwrap();
        ExportTarget::new(
            Some(ExportSource {
                artifact_id: "shot".into(),
                path,
            }),
            Path::new("/unused"),
            "unused",
        )
    }

    #[test]
    fn saved_source_overwrites_by_default_and_fresh_capture_saves_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let target = sourced(dir.path(), "Shot.png");
        assert_eq!((target.stem.as_str(), target.save_as_new), ("Shot", false));
        assert_eq!(target.directory, dir.path());
        assert_eq!(
            target.plan(ExportFormat::Png).unwrap(),
            SavePlan::Overwrite {
                artifact_id: "shot".into(),
                path: dir.path().join("Shot.png")
            }
        );

        let fresh = ExportTarget::new(None, Path::new("/exports"), "Captures_edited");
        assert!(fresh.save_as_new && fresh.format_requires_copy(ExportFormat::Png));
        assert_eq!(
            fresh.plan(ExportFormat::Png).unwrap(),
            SavePlan::NewFile {
                path: PathBuf::from("/exports/Captures_edited.png")
            }
        );
    }

    #[test]
    fn filename_folder_and_format_changes_force_a_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut target = sourced(dir.path(), "Shot.png");
        // A different format never overwrites a file of another type.
        assert!(target.format_requires_copy(ExportFormat::Webp));
        assert!(!target.plan(ExportFormat::Webp).unwrap().overwrites());
        assert!(
            !target.save_as_new,
            "a format change forces a copy without flipping the switch"
        );
        target.set_save_as_new(false, ExportFormat::Webp);
        assert!(!target.plan(ExportFormat::Webp).unwrap().overwrites());

        target.set_stem("Renamed");
        assert!(target.save_as_new);
        assert_eq!(
            target.plan(ExportFormat::Png).unwrap(),
            SavePlan::NewFile {
                path: dir.path().join("Renamed.png")
            }
        );
        target.set_stem("Shot");
        assert!(
            target.save_as_new,
            "returning to the source name keeps the explicit copy"
        );
        target.set_save_as_new(false, ExportFormat::Png);
        assert!(target.plan(ExportFormat::Png).unwrap().overwrites());

        target.set_directory("/elsewhere");
        assert!(target.save_as_new);
        assert_eq!(
            target.plan(ExportFormat::Png).unwrap().path(),
            Path::new("/elsewhere/Shot.png")
        );
    }

    #[test]
    fn switch_suggests_and_restores_the_edited_name() {
        let dir = tempfile::tempdir().unwrap();
        let mut target = sourced(dir.path(), "Shot.jpeg");
        target.set_save_as_new(true, ExportFormat::Jpeg);
        assert_eq!(target.stem, "Shot-edited");
        assert_eq!(target.extension(ExportFormat::Jpeg), "jpeg");
        assert_eq!(
            target.plan(ExportFormat::Jpeg).unwrap(),
            SavePlan::NewFile {
                path: dir.path().join("Shot-edited.jpeg")
            }
        );
        target.set_directory("/elsewhere");
        target.set_save_as_new(false, ExportFormat::Jpeg);
        assert_eq!(
            (target.stem.as_str(), target.directory.as_path()),
            ("Shot", dir.path())
        );
        assert!(target.plan(ExportFormat::Jpeg).unwrap().overwrites());

        target.set_stem("Custom name");
        target.set_save_as_new(false, ExportFormat::Jpeg);
        assert_eq!(
            target.stem, "Custom name",
            "a typed name is never discarded"
        );
        assert!(!target.plan(ExportFormat::Jpeg).unwrap().overwrites());
    }

    #[test]
    fn missing_source_and_invalid_names_never_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let target = ExportTarget::new(
            Some(ExportSource {
                artifact_id: "shot".into(),
                path: dir.path().join("gone.png"),
            }),
            Path::new("/unused"),
            "unused",
        );
        assert!(target.source_missing && target.saving_copy(ExportFormat::Png));
        assert!(!target.plan(ExportFormat::Png).unwrap().overwrites());
        let hint = save_hint(
            &target,
            options(ExportFormat::Png, ExportQuality::Preserve),
            false,
        );
        assert_eq!(
            hint,
            "The original was deleted. You can still copy or save this edit."
        );

        for stem in ["", " padded", "a/b", "trailing.", "CON", "lpt3", "x:y"] {
            let mut target = sourced(dir.path(), "Shot.png");
            target.set_stem(stem);
            assert_eq!(
                target.plan(ExportFormat::Png).unwrap_err(),
                "Enter a filename without folders or reserved characters.",
                "{stem:?}"
            );
        }
        let mut target = sourced(dir.path(), "Shot.png");
        target.set_stem("COM10 notes");
        assert!(target.plan(ExportFormat::Png).is_ok());
    }

    #[test]
    fn hints_follow_the_shipping_save_model() {
        let dir = tempfile::tempdir().unwrap();
        let mut target = sourced(dir.path(), "Shot.png");
        let preserve = options(ExportFormat::Png, ExportQuality::Preserve);
        assert_eq!(
            save_hint(&target, preserve, false),
            "Save keeps original quality as PNG and overwrites the original."
        );
        assert_eq!(
            save_hint(
                &target,
                options(ExportFormat::Png, ExportQuality::Compress),
                false
            ),
            "Save overwrites the original with compressed PNG. Turn on Save as new file to keep it."
        );
        assert_eq!(
            save_hint(
                &target,
                options(ExportFormat::Jpeg, ExportQuality::Preserve),
                true
            ),
            "JPEG will fill in transparent areas. Use PNG or WebP to keep them."
        );
        target.set_save_as_new(true, ExportFormat::Png);
        assert_eq!(
            save_hint(
                &target,
                options(ExportFormat::Webp, ExportQuality::Maximum),
                false
            ),
            "Save writes a new WebP within the selected size limit and leaves the original untouched."
        );
        let fresh = ExportTarget::new(None, Path::new("/exports"), "edit");
        assert_eq!(
            save_hint(&fresh, preserve, false),
            "Save writes a PNG at original quality."
        );
    }

    #[test]
    fn sizes_deltas_and_summary_match_the_shipping_labels() {
        assert_eq!(format_file_size(0), "0 B");
        assert_eq!(format_file_size(999), "999 B");
        assert_eq!(format_file_size(1_000), "1.0 KB");
        assert_eq!(format_file_size(240_400), "240 KB");
        assert_eq!(format_file_size(12_345_678), "12.3 MB");
        assert_eq!(size_delta(Some(88), Some(100)).unwrap().label, "−12%");
        assert_eq!(size_delta(Some(108), Some(100)).unwrap().label, "+8%");
        assert_eq!(size_delta(Some(1_002), Some(1_000)), None);
        assert_eq!(size_delta(Some(10), None), None);

        let target = ExportTarget::new(None, Path::new("/exports"), "edit");
        let mut png = options(ExportFormat::Png, ExportQuality::Preserve);
        let view = present(
            &target,
            png,
            (1920, 1080),
            false,
            EstimateState {
                bytes: Some(240_000),
                baseline_bytes: Some(300_000),
                pending: false,
            },
        );
        assert_eq!(view.summary, "PNG · 1920 × 1080 · ≈ 240 KB");
        assert_eq!(view.suffix, ".png");
        assert_eq!(view.delta.unwrap().label, "−20%");
        assert!(view.error.is_none() && view.plan.is_some());

        png.size = ExportSize::Percent { percent: 50 };
        let pending = present(
            &target,
            png,
            (1920, 1080),
            false,
            EstimateState {
                bytes: None,
                baseline_bytes: None,
                pending: true,
            },
        );
        assert_eq!(pending.summary, "PNG · 960 × 540 · Estimating…");
        assert!(pending.delta.is_none());

        let mut jpeg = options(ExportFormat::Jpeg, ExportQuality::Maximum);
        jpeg.max_size_bytes = Some(50_000);
        let capped = present(
            &target,
            jpeg,
            (100, 100),
            true,
            EstimateState {
                bytes: Some(70_000),
                baseline_bytes: Some(10_000),
                pending: false,
            },
        );
        assert_eq!(capped.estimate_label, "≤ 50.0 KB");
        assert!(capped.delta.is_none() && capped.hint_warning);
        jpeg.max_size_bytes = Some(9_999);
        assert_eq!(
            present(&target, jpeg, (100, 100), false, EstimateState::default())
                .error
                .as_deref(),
            Some("Enter a maximum file size of at least 10 KB.")
        );
    }

    #[test]
    fn estimate_matches_save_and_uses_the_shipping_baseline() {
        let image = RgbaImage::from_fn(31, 17, |x, y| {
            Rgba([(x * 7) as u8, (y * 13) as u8, ((x + y) * 5) as u8, 255])
        });
        let preserve = options(ExportFormat::Png, ExportQuality::Preserve);
        let estimate = estimate_export(&image, preserve, Some(4_321)).unwrap();
        assert_eq!(
            estimate.bytes,
            captures_image::encode_export(&image, preserve)
                .unwrap()
                .len() as u64
        );
        assert_eq!(estimate.baseline_bytes, Some(4_321));
        let compress = options(ExportFormat::Webp, ExportQuality::Compress);
        let estimate = estimate_export(&image, compress, Some(4_321)).unwrap();
        assert_eq!(
            estimate.baseline_bytes,
            Some(captures_history::encode_png(&image).unwrap().len() as u64)
        );
        let mut invalid = preserve;
        invalid.size = ExportSize::Custom {
            width: 0,
            height: 3,
        };
        assert!(estimate_export(&image, invalid, None).is_err());
    }

    #[test]
    fn publish_overwrites_then_adopts_new_files_and_reports_collisions() {
        let history = tempfile::tempdir().unwrap();
        let exports = tempfile::tempdir().unwrap();
        let original = RgbaImage::from_pixel(4, 3, Rgba([1, 2, 3, 255]));
        let artifact =
            crate::persist_screenshot(history.path(), &original, CaptureMode::Region).unwrap();
        let mut target = ExportTarget::new(None, exports.path(), "edited");
        let edited = RgbaImage::from_pixel(4, 3, Rgba([200, 20, 40, 255]));
        let png = options(ExportFormat::Png, ExportQuality::Preserve);

        let plan = target.plan(ExportFormat::Png).unwrap();
        let saved = publish(history.path(), &edited, &plan, png, CaptureMode::Region).unwrap();
        assert_eq!(
            saved_notice(&plan, &saved),
            format!("Saved {}", plan.path().display())
        );
        target.record_saved(&saved);
        let SavedExport::Saved {
            artifact: first, ..
        } = &saved
        else {
            panic!("History should be available")
        };
        assert_ne!(first.entry.id, artifact.entry.id);
        assert!(!target.save_as_new);
        let overwrite = target.plan(ExportFormat::Png).unwrap();
        assert_eq!(
            overwrite,
            SavePlan::Overwrite {
                artifact_id: first.entry.id.clone(),
                path: exports.path().join("edited.png")
            }
        );
        let second = RgbaImage::from_pixel(4, 3, Rgba([9, 99, 199, 255]));
        let replaced = publish(
            history.path(),
            &second,
            &overwrite,
            png,
            CaptureMode::Region,
        )
        .unwrap();
        assert_eq!(
            saved_notice(&overwrite, &replaced),
            "Saved changes to the original"
        );
        assert_eq!(
            image::open(exports.path().join("edited.png"))
                .unwrap()
                .to_rgba8(),
            second
        );
        assert_eq!(
            original_size_bytes(history.path(), &first.entry.id),
            Some(
                fs::metadata(exports.path().join("edited.png"))
                    .unwrap()
                    .len()
            )
        );

        target.set_save_as_new(true, ExportFormat::Png);
        target.set_stem("edited");
        let collision = target.plan(ExportFormat::Png).unwrap();
        assert!(!collision.overwrites());
        assert_eq!(
            publish(
                history.path(),
                &edited,
                &collision,
                png,
                CaptureMode::Region
            )
            .unwrap_err(),
            "edited.png already exists. Choose another filename."
        );
        assert_eq!(
            image::open(exports.path().join("edited.png"))
                .unwrap()
                .to_rgba8(),
            second
        );
    }

    #[test]
    fn quality_copy_follows_the_shipping_export_bar() {
        assert_eq!(
            QUALITY_PRESETS.map(|preset| preset.quality),
            [55, 70, 85, 92, 98]
        );
        let tiny = quality_preset(55).unwrap();
        assert_eq!(tiny.label, "Tiny");
        assert_eq!(
            tiny.description(ExportFormat::Png),
            "Smallest PNG with the most visible dithering."
        );
        assert_eq!(
            quality_preset(98).unwrap().description(ExportFormat::Webp),
            "Light lossy WebP. Near-original quality, a modest size cut."
        );
        assert!(quality_preset(80).is_none());
        assert_eq!(
            quality_mode_description(ExportQuality::Compress, ExportFormat::Jpeg),
            "Smaller JPEG with Tiny through Highest quality presets."
        );
        assert!(maximum_size_help(ExportFormat::Png).starts_with("Uses stronger PNG"));
        assert!(maximum_size_help(ExportFormat::Jpeg).starts_with("JPEG quality is lowered"));

        let target = ExportTarget::new(None, Path::new("/tmp"), "shot");
        let view = present(
            &target,
            options(ExportFormat::Webp, ExportQuality::Compress),
            (10, 10),
            false,
            EstimateState::default(),
        );
        assert!(view.compresses);
        assert_eq!(view.quality_modes.len(), 3);
        assert_eq!(view.quality_modes[2].label, "Maximum file size");
        assert_eq!(view.quality_presets[1].value, 70);
        assert_eq!(
            view.quality_presets[1].description,
            "Very small lossy WebP with more visible compression."
        );
        assert!(
            !present(
                &target,
                options(ExportFormat::Png, ExportQuality::Preserve),
                (10, 10),
                false,
                EstimateState::default(),
            )
            .compresses
        );
    }

    #[test]
    fn file_size_units_are_decimal_and_floor_exactly() {
        assert_eq!(FileSizeUnit::Mb.bytes("10"), Some(10_000_000));
        assert_eq!(FileSizeUnit::Kb.bytes("10.5"), Some(10_500));
        assert_eq!(FileSizeUnit::Kb.bytes("0.0009"), Some(0));
        assert_eq!(FileSizeUnit::Gb.bytes(".25"), Some(250_000_000));
        assert_eq!(FileSizeUnit::Mb.bytes(""), None);
        assert_eq!(FileSizeUnit::Mb.bytes("1e3"), None);
        assert_eq!(FileSizeUnit::Mb.bytes("-1"), None);
        assert_eq!(FileSizeUnit::Mb.value(10_000_000), "10");
        assert_eq!(FileSizeUnit::Kb.value(10_500), "10.5");
        assert_eq!(FileSizeUnit::Gb.value(250_000_000), "0.25");
        assert_eq!(
            FileSizeUnit::Kb.minimum() * 1_000.,
            MINIMUM_MAX_SIZE_BYTES as f64
        );
        assert_eq!(FileSizeUnit::default(), FileSizeUnit::Mb);
        assert_eq!(
            FileSizeUnit::ALL.map(FileSizeUnit::label),
            ["KB", "MB", "GB"]
        );
        let bytes = FileSizeUnit::Mb.bytes("1.5").unwrap();
        assert_eq!(FileSizeUnit::Kb.value(bytes), "1500");
    }
}
