//! Shipping editor paragraph, auto-width and interaction rules.
//!
//! Paint uses explicit font measurement. Selection/resize deliberately use the
//! shipping UTF-16 width estimate, not glyph ink or host font access.

use serde::Serialize;

use crate::editor::{ElementStyle, Point, Rect, TextElement, annotation_drop_shadow_pad};

#[derive(Debug, PartialEq, Serialize)]
pub struct TextRow {
    pub text: String,
    /// Logical left edge after alignment; ink bearings are applied separately.
    pub x: f64,
    /// Top of the 1.25-em line box, not its baseline.
    pub y: f64,
    pub advance: f64,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct TextPlate {
    pub bounds: Rect,
    pub radius: f64,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct TextLayout {
    pub rows: Vec<TextRow>,
    pub content: Rect,
    pub plate: Option<TextPlate>,
}

fn minimum_width(size: f64) -> f64 {
    8_f64.max((size * 0.5).round())
}

// ECMAScript \s/trim, deliberately not Rust's Unicode White_Space property:
// BOM is whitespace here and NEXT LINE (U+0085) is not.
fn whitespace(c: char) -> bool {
    matches!(c, '\u{0009}'..='\u{000d}' | ' ' | '\u{00a0}' | '\u{1680}' |
        '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' |
        '\u{205f}' | '\u{3000}' | '\u{feff}')
}

fn validate(element: &TextElement) -> Result<(), String> {
    if element.text.len() > 4096 {
        return Err("Text paragraphs support at most 4096 UTF-8 bytes.".into());
    }
    if !element.font_size.is_finite() || element.font_size <= 0. || element.font_size > 512. {
        return Err("Text size must be greater than zero and at most 512.".into());
    }
    if !element.base.x.is_finite() || !element.base.y.is_finite() || !element.width.is_finite() {
        return Err("Text box geometry must be finite.".into());
    }
    if !matches!(element.align.as_str(), "left" | "center" | "right") {
        return Err("Unsupported text alignment.".into());
    }
    Ok(())
}

fn advance(
    text: &str,
    measure: &mut impl FnMut(&str) -> Result<f64, String>,
) -> Result<f64, String> {
    let value = measure(if text.is_empty() { " " } else { text })?;
    if !value.is_finite() || value < 0. {
        return Err("Text measurement must be finite and nonnegative.".into());
    }
    Ok(value)
}

fn wrap(
    text: &str,
    width: f64,
    measure: &mut impl FnMut(&str) -> Result<f64, String>,
) -> Result<Vec<String>, String> {
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let mut remaining = paragraph;
        let mut current = String::new();
        while let Some(first) = remaining.chars().next() {
            let is_space = whitespace(first);
            let end = remaining
                .find(|c| whitespace(c) != is_space)
                .unwrap_or(remaining.len());
            let token = &remaining[..end];
            remaining = &remaining[end..];
            if is_space {
                if !current.is_empty() {
                    current.push_str(token);
                }
                continue;
            }
            let mut pieces = Vec::new();
            if advance(token, measure)? <= width {
                pieces.push(token.to_owned());
            } else {
                let mut piece = String::new();
                // The shipping implementation breaks oversized tokens by Unicode
                // scalars, not bytes, UTF-16 code units or grapheme clusters.
                for c in token.chars() {
                    let next = format!("{piece}{c}");
                    if !piece.is_empty() && advance(&next, measure)? > width {
                        pieces.push(piece);
                        piece = c.to_string();
                    } else {
                        piece = next;
                    }
                }
                pieces.push(piece);
            }
            for piece in pieces {
                let candidate = format!("{current}{piece}");
                if !current.is_empty() && advance(&candidate, measure)? > width {
                    lines.push(current.trim_end_matches(whitespace).to_owned());
                    current = piece;
                } else {
                    current = candidate;
                }
            }
        }
        lines.push(current.trim_end_matches(whitespace).to_owned());
    }
    Ok(lines)
}

/// Compute wrapped paint rows and an optional plate. Measurements must use the
/// same face/style as rasterization. A glyph wider than the box is not discarded.
pub fn layout(
    element: &TextElement,
    mut measure: impl FnMut(&str) -> Result<f64, String>,
) -> Result<TextLayout, String> {
    validate(element)?;
    // Preserve drawText's unrounded paint minimum; wrapTextLines separately uses
    // the rounded eight-pixel minimum. They differ for narrow/fractional boxes.
    let width = element.width.max(element.font_size * 0.5);
    let lines = wrap(
        &element.text,
        width.max(minimum_width(element.font_size)),
        &mut measure,
    )?;
    let line_height = element.font_size * 1.25;
    let content = Rect {
        x: element.base.x,
        y: element.base.y,
        width,
        height: lines.len() as f64 * line_height,
    };
    let mut rows = Vec::with_capacity(lines.len());
    for (index, text) in lines.into_iter().enumerate() {
        let advance = advance(&text, &mut measure)?;
        let offset = match element.align.as_str() {
            "center" => (width - advance) / 2.,
            "right" => width - advance,
            _ => 0.,
        };
        let x = content.x + offset;
        if !x.is_finite() {
            return Err("Aligned text box geometry must be finite.".into());
        }
        rows.push(TextRow {
            text,
            x,
            y: content.y + index as f64 * line_height,
            advance,
        });
    }
    let plate = element
        .background
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(|_| {
            let pad_x = element.font_size * 0.36;
            let pad_y = element.font_size * 0.22;
            let bounds = Rect {
                x: content.x - pad_x,
                y: content.y - pad_y,
                width: content.width + pad_x * 2.,
                height: content.height + pad_y * 2.,
            };
            let shortest = bounds.width.min(bounds.height);
            TextPlate {
                bounds,
                radius: if element.rounded_background {
                    (shortest * 0.28)
                        .min(element.font_size * 0.34)
                        .min(shortest / 2.)
                } else {
                    0.
                },
            }
        });
    Ok(TextLayout {
        rows,
        content,
        plate,
    })
}

fn fitted_width(
    text: &str,
    size: f64,
    measure: &mut impl FnMut(&str) -> Result<f64, String>,
) -> Result<f64, String> {
    let mut widest = 0_f64;
    for line in text.split('\n') {
        widest = widest.max(advance(line, measure)?);
    }
    if widest <= 0. {
        widest = advance(" ", measure)?;
    }
    Ok(minimum_width(size).max((widest + size * 0.35).ceil()))
}

// JavaScript text.length counts UTF-16 code units, even though hard wrapping
// splits oversized tokens by Unicode scalars. Keep these two rules distinct.
fn estimate(text: &str, size: f64) -> Result<f64, String> {
    Ok(text.encode_utf16().count().max(1) as f64 * size * 0.56)
}

/// Project text onto the shared annotation shadow rules. `size` may be the
/// candidate font size during a resize, rather than the authored size.
pub fn shadow_style(element: &TextElement, size: f64) -> ElementStyle {
    ElementStyle {
        color: element.color.clone(),
        fill: None,
        stroke_width: (size * 0.22).max(4.),
        stroke_enabled: None,
        drop_shadow: element.drop_shadow,
        drop_shadow_style: element.drop_shadow_style.clone(),
        extra: Default::default(),
    }
}

fn interaction_pad(element: &TextElement, size: f64) -> Point {
    let shadow = annotation_drop_shadow_pad(&shadow_style(element, size));
    let plate = element
        .background
        .as_deref()
        .is_some_and(|color| !color.is_empty());
    Point {
        x: shadow + if plate { size * 0.36 } else { 0. },
        y: shadow + if plate { size * 0.22 } else { 0. },
    }
}

/// Unrotated shipping interaction bounds, including plate/shadow padding.
/// These intentionally use estimated wrapping; painting uses real shaping.
pub fn selection_bounds(element: &TextElement) -> Result<Rect, String> {
    validate(element)?;
    let width = element.width.max(minimum_width(element.font_size));
    let rows = wrap(&element.text, width, &mut |line| {
        estimate(line, element.font_size)
    })?;
    let pad = interaction_pad(element, element.font_size);
    Ok(Rect {
        x: element.base.x - pad.x,
        y: element.base.y - pad.y,
        width: width + pad.x * 2.,
        height: rows.len() as f64 * element.font_size * 1.25 + pad.y * 2.,
    })
}

/// Map a text layer between selection boxes. Side resizing reflows fixed-width
/// text; other drags scale type. Auto-width labels refit rather than stretch ink.
pub fn resize(element: &TextElement, initial: Rect, next: Rect) -> Result<TextElement, String> {
    validate(element)?;
    if ![
        initial.width,
        initial.height,
        next.x,
        next.y,
        next.width,
        next.height,
    ]
    .into_iter()
    .all(f64::is_finite)
        || next.width <= 0.
        || next.height <= 0.
    {
        return Err("Text resize requires finite, positive bounds.".into());
    }
    let scale_x = next.width / initial.width.max(1.);
    let scale_y = next.height / initial.height.max(1.);
    let width_only = (scale_y - 1.).abs() < 0.001 && (scale_x - 1.).abs() >= 0.001;
    let height_only = (scale_x - 1.).abs() < 0.001 && (scale_y - 1.).abs() >= 0.001;
    let auto = element.uses_auto_width();
    let mut resized = element.clone();
    if width_only && !auto {
        let pad = interaction_pad(element, element.font_size);
        resized.base.x = next.x + pad.x;
        resized.base.y = next.y + pad.y;
        resized.width = minimum_width(element.font_size).max(next.width - pad.x * 2.);
    } else {
        let scale = if width_only {
            scale_x
        } else if height_only {
            scale_y
        } else {
            scale_x.abs().min(scale_y.abs())
        }
        .max(0.05);
        let size = (element.font_size * scale).round().clamp(8., 512.);
        let pad = interaction_pad(element, size);
        resized.font_size = size;
        resized.base.x = next.x + pad.x;
        resized.base.y = next.y + pad.y;
        resized.width = if auto {
            fitted_width(&element.text, size, &mut |line| estimate(line, size))?
        } else {
            minimum_width(size).max(element.width * scale)
        };
    }
    resized.auto_width = Some(auto);
    validate(&resized)?;
    Ok(resized)
}

/// Fit an auto-width element while preserving every unrelated document field.
/// While editing, blank text retains the shipping eight-em composing field.
pub fn fit_auto_width(
    element: &TextElement,
    editing: bool,
    mut measure: impl FnMut(&str) -> Result<f64, String>,
) -> Result<TextElement, String> {
    if !element.uses_auto_width() {
        return Ok(element.clone());
    }
    validate(element)?;
    let minimum = minimum_width(element.font_size);
    let width = if editing && element.text.trim_matches(whitespace).is_empty() {
        minimum.max((element.font_size * 8.).round())
    } else {
        fitted_width(&element.text, element.font_size, &mut measure)?
    };
    let delta = width - element.width;
    let mut fitted = element.clone();
    if delta.abs() >= 0.5 {
        fitted.base.x -= match element.align.as_str() {
            "center" => delta / 2.,
            "right" => delta,
            _ => 0.,
        };
    }
    fitted.width = width;
    if !fitted.width.is_finite() || !fitted.base.x.is_finite() {
        return Err("Fitted text box geometry must be finite.".into());
    }
    Ok(fitted)
}
