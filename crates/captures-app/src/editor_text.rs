//! Shipping editor paragraph and auto-width rules, with explicit font measurement.
//!
//! No estimated glyph widths or host font access. This is paint layout, not the
//! shipping estimated selection geometry. Hosts still need font ownership, text
//! commands and input before they can use it. Shadows and glyph ink are separate.

use serde::Serialize;

use crate::editor::{Rect, TextElement};

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
        let mut widest = 0_f64;
        for line in element.text.split('\n') {
            widest = widest.max(advance(line, &mut measure)?);
        }
        if widest <= 0. {
            widest = advance(" ", &mut measure)?;
        }
        minimum.max((widest + element.font_size * 0.35).ceil())
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
