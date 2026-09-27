//! Shipping `CompressionPreview`: the automatic before/after split both
//! editors show while a save quality compresses the output.
//!
//! The screenshot editor covers its canvas with the flattened edit (Before)
//! and the encoded file (After) while Compress or Maximum file size is chosen
//! and Export settings are open. The recording editor covers its preview with
//! the source frame and an encoded sample while Compress or Maximum is chosen
//! and playback is paused. Either can be hidden with Hide until the quality
//! mode changes, and the screenshot editor offers Show before / after in its
//! export settings meanwhile. Hosts own the drawing; this module owns the copy,
//! the badge text, the savings figure and the split bounds.

use captures_image::ExportOptions;
use image::RgbaImage;
use serde::Serialize;

use crate::{editor_export, recording_editor_ui::format_file_size};

/// Accessible name of the comparison frame.
pub const GROUP_LABEL: &str = "Compression comparison";
/// Accessible name of the round divider handle.
pub const HANDLE_LABEL: &str = "Drag to compare before and after";
/// Accessible name of the full-width split range.
pub const RANGE_LABEL: &str = "Before and after comparison";
/// The handle's glyph.
pub const HANDLE_GLYPH: &str = "‹ ›";
pub const BEFORE: &str = "Before";
pub const AFTER: &str = "After";
/// The dismiss pill in the frame's top-right corner.
pub const DISMISS: &str = "Hide";
pub const DISMISS_LABEL: &str = "Hide compression comparison";
/// The centred status pill while a new encode is running.
pub const PROCESSING: &str = "Processing";
/// Screenshot export settings: caption and button that bring a hidden
/// comparison back.
pub const SHOW_CAPTION: &str = "Comparison";
pub const SHOW: &str = "Show before / after";
/// Cursor hint on the After side while a drawing tool is selected.
pub const AFTER_HINT: &str = "Edits apply to the original. This side updates after you finish.";

/// Split bounds that keep the handle clear of the badges (shipping 6–94 %).
pub const MIN_SPLIT: f64 = 0.06;
pub const MAX_SPLIT: f64 = 0.94;
pub const DEFAULT_SPLIT: f64 = 0.5;
/// The screenshot editor waits this long after the last change before
/// encoding the After side.
pub const SCREENSHOT_REFRESH_DELAY_MS: u64 = 280;
/// The recording editor waits this long before encoding a sample frame.
pub const REFRESH_DELAY_MS: u64 = 350;
/// The divider handle's diameter in points.
pub const HANDLE_SIZE: f64 = 36.;

/// A split fraction kept inside the shipping bounds; NaN falls back to centre.
pub fn clamp_split(split: f64) -> f64 {
    if split.is_nan() {
        return DEFAULT_SPLIT;
    }
    split.clamp(MIN_SPLIT, MAX_SPLIT)
}

/// The split under a pointer `x` points into a frame `width` points wide.
pub fn split_at(x: f64, width: f64) -> f64 {
    clamp_split(x / width.max(1.))
}

/// Whole-percent reduction from Before to After, only when After is smaller.
pub fn savings_percent(before: Option<u64>, after: Option<u64>) -> Option<u64> {
    let (before, after) = (before.filter(|bytes| *bytes > 0)?, after?);
    (after < before).then(|| ((1. - after as f64 / before as f64) * 100.).round() as u64)
}

/// The two corner badges. `savings` is drawn after `after` in the positive
/// text colour, as shipping's `.compression-preview-savings`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Badges {
    pub before: String,
    pub after: String,
    pub savings: Option<String>,
}

/// "Before · 1.2 MB" and "After · 480 KB" + " · 61% smaller", or
/// "After · Processing…" while a new encode runs.
pub fn badges(before: Option<u64>, after: Option<u64>, processing: bool) -> Badges {
    let before_label = match before {
        Some(bytes) => format!("{BEFORE} · {}", format_file_size(bytes)),
        None => BEFORE.to_owned(),
    };
    if processing {
        return Badges {
            before: before_label,
            after: format!("{AFTER} · Processing…"),
            savings: None,
        };
    }
    let Some(after_bytes) = after else {
        return Badges {
            before: before_label,
            after: AFTER.to_owned(),
            savings: None,
        };
    };
    Badges {
        before: before_label,
        after: format!("{AFTER} · {}", format_file_size(after_bytes)),
        savings: savings_percent(before, Some(after_bytes))
            .map(|percent| format!(" · {percent}% smaller")),
    }
}

/// Screenshot editor: shown while Compress or Maximum is chosen, Export
/// settings are open and the user has not hidden it.
pub fn screenshot_visible(compresses: bool, settings_open: bool, dismissed: bool) -> bool {
    compresses && settings_open && !dismissed
}

/// Recording editor: shown while Compress or Maximum is chosen for MP4 or
/// GIF, playback is paused and the user has not hidden it.
pub fn recording_visible(compresses: bool, playing: bool, dismissed: bool) -> bool {
    compresses && !playing && !dismissed
}

/// The screenshot After side: the flattened edit encoded exactly as Save
/// would, without the session worker or any I/O.
pub fn encode(image: &RgbaImage, options: ExportOptions) -> Result<Vec<u8>, String> {
    editor_export::validate_options(options, image.width(), image.height())?;
    captures_image::encode_export(image, options)
}

/// [`encode`], decoded back to pixels for hosts that draw with their own
/// textures, plus the encoded length.
pub fn encode_after(image: &RgbaImage, options: ExportOptions) -> Result<(RgbaImage, u64), String> {
    let bytes = encode(image, options)?;
    let decoded = image::load_from_memory(&bytes)
        .map_err(|error| error.to_string())?
        .into_rgba8();
    Ok((decoded, bytes.len() as u64))
}

/// Everything a host needs to draw the comparison over one frame.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Copy {
    pub group_label: &'static str,
    pub handle_label: &'static str,
    pub range_label: &'static str,
    pub handle_glyph: &'static str,
    pub dismiss: &'static str,
    pub dismiss_label: &'static str,
    pub processing: &'static str,
    pub show_caption: &'static str,
    pub show: &'static str,
    pub after_hint: &'static str,
}

pub const COPY: Copy = Copy {
    group_label: GROUP_LABEL,
    handle_label: HANDLE_LABEL,
    range_label: RANGE_LABEL,
    handle_glyph: HANDLE_GLYPH,
    dismiss: DISMISS,
    dismiss_label: DISMISS_LABEL,
    processing: PROCESSING,
    show_caption: SHOW_CAPTION,
    show: SHOW,
    after_hint: AFTER_HINT,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn savings_round_like_shipping_and_hide_growth_or_unknown_sizes() {
        assert_eq!(savings_percent(Some(1_000), Some(385)), Some(62));
        assert_eq!(savings_percent(Some(1_000), Some(995)), Some(1));
        assert_eq!(savings_percent(Some(1_000), Some(1_000)), None);
        assert_eq!(savings_percent(Some(1_000), Some(1_200)), None);
        assert_eq!(savings_percent(Some(0), Some(10)), None);
        assert_eq!(savings_percent(None, Some(10)), None);
        assert_eq!(savings_percent(Some(10), None), None);
    }

    #[test]
    fn badges_carry_sizes_savings_and_processing() {
        assert_eq!(
            badges(Some(1_240_000), Some(480_000), false),
            Badges {
                before: "Before · 1.2 MB".into(),
                after: "After · 480 KB".into(),
                savings: Some(" · 61% smaller".into()),
            }
        );
        let growing = badges(Some(1_000), Some(2_000), false);
        assert_eq!(growing.after, "After · 2.0 KB");
        assert_eq!(growing.savings, None);
        let processing = badges(Some(1_000), Some(10), true);
        assert_eq!(processing.after, "After · Processing…");
        assert_eq!(processing.savings, None);
        let unknown = badges(None, None, false);
        assert_eq!(
            (unknown.before.as_str(), unknown.after.as_str()),
            ("Before", "After")
        );
    }

    #[test]
    fn split_stays_clear_of_the_badges() {
        assert_eq!(clamp_split(0.), MIN_SPLIT);
        assert_eq!(clamp_split(1.), MAX_SPLIT);
        assert_eq!(clamp_split(0.3), 0.3);
        assert_eq!(clamp_split(f64::NAN), DEFAULT_SPLIT);
        assert_eq!(split_at(50., 200.), 0.25);
        assert_eq!(split_at(-10., 200.), MIN_SPLIT);
        assert_eq!(split_at(10., 0.), MAX_SPLIT);
    }

    #[test]
    fn after_side_encodes_like_save_and_rejects_invalid_options() {
        use captures_image::{ExportFormat, ExportQuality, ExportSize, PngOptions};
        let image = RgbaImage::from_fn(12, 8, |x, y| {
            image::Rgba([x as u8 * 20, y as u8 * 30, 90, 255])
        });
        let mut options = ExportOptions {
            format: ExportFormat::Jpeg,
            quality: ExportQuality::Compress,
            quality_value: 55,
            max_size_bytes: None,
            png: PngOptions::default(),
            size: ExportSize::Original,
        };
        let (after, bytes) = encode_after(&image, options).unwrap();
        assert_eq!(after.dimensions(), (12, 8));
        assert_eq!(
            bytes,
            captures_image::encode_export(&image, options)
                .unwrap()
                .len() as u64
        );
        options.quality = ExportQuality::Maximum;
        options.max_size_bytes = Some(10);
        assert!(
            encode(&image, options)
                .unwrap_err()
                .contains("at least 10 KB")
        );
    }

    #[test]
    fn visibility_follows_the_shipping_editors() {
        assert!(screenshot_visible(true, true, false));
        assert!(!screenshot_visible(false, true, false));
        assert!(!screenshot_visible(true, false, false));
        assert!(!screenshot_visible(true, true, true));
        assert!(recording_visible(true, false, false));
        assert!(!recording_visible(true, true, false));
        assert!(!recording_visible(true, false, true));
        assert!(!recording_visible(false, false, false));
    }
}
