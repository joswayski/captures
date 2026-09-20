//! Pixel rendering for the shared screenshot-editor document.
//!
//! Only image layers are supported in this first rendering slice. Assets are
//! supplied by exact document `src`, so rendering performs no filesystem,
//! network, host-font, or UI access.

use std::{collections::BTreeMap, sync::Arc};

use captures_image::{BlendMode, Layer, Point, Shape};
use image::{Rgba, RgbaImage};

use crate::editor::{Document, Element, ImageElement, ImageOrientation};

pub const MAX_RENDER_DIMENSION: u32 = 16_384;
pub const MAX_RENDER_PIXELS: u64 = 100_000_000;

/// Render visible image layers in document order through `captures-image`.
///
/// The input document and shared assets are borrowed and never mutated. Text,
/// shape, and path rendering will arrive in later slices and is rejected while
/// visible rather than silently omitted.
pub fn render(
    document: &Document,
    assets: &BTreeMap<String, Arc<RgbaImage>>,
) -> Result<RgbaImage, String> {
    let (width, height) = validate_canvas(document)?;
    let background = document
        .background
        .as_deref()
        .map(parse_color)
        .transpose()?
        .unwrap_or(Rgba([0, 0, 0, 0]));

    // Validate the complete visible stack before any orientation or canvas
    // allocation. A later unsupported layer must not leave earlier work done.
    for element in document
        .elements
        .iter()
        .filter(|element| element.base().visible)
    {
        match element {
            Element::Image(image) => validate_image(image, assets)?,
            Element::Text(text) => return Err(unsupported_layer("text", &text.base.id)),
            Element::Shape(shape) => return Err(unsupported_layer("shape", &shape.base.id)),
            Element::Path(path) => return Err(unsupported_layer("path", &path.base.id)),
        }
    }

    let mut layers = Vec::new();
    for (index, element) in document.elements.iter().enumerate() {
        let Element::Image(image) = element else {
            continue;
        };
        if !image.base.visible {
            continue;
        }
        let pixels = oriented_asset(
            assets
                .get(&image.src)
                .expect("visible image assets were validated"),
            image.resolved_orientation(),
        );
        layers.push(Layer {
            id: u64::try_from(index).map_err(|_| "too many editor layers to render".to_owned())?,
            shape: Shape::Image {
                origin: Point {
                    x: finite_f32(image.base.x, "x", &image.base.id)?,
                    y: finite_f32(image.base.y, "y", &image.base.id)?,
                },
                width: positive_f32(image.width, "width", &image.base.id)?,
                height: positive_f32(image.height, "height", &image.base.id)?,
                pixels,
            },
            color: [
                255,
                255,
                255,
                (image.base.opacity.clamp(0., 100.) * 2.55).round() as u8,
            ],
            stroke_width: 0.,
            fill: None,
            rotation_degrees: radians_to_degrees(image.base.rotation(), &image.base.id)?,
            blend_mode: blend_mode(&image.base.blend_mode, &image.base.id)?,
        });
    }

    captures_image::render(&captures_image::Document {
        source: Arc::new(RgbaImage::from_pixel(width, height, background)),
        crop: None,
        layers,
    })
}

fn validate_canvas(document: &Document) -> Result<(u32, u32), String> {
    let width = canvas_dimension(document.width, "width")?;
    let height = canvas_dimension(document.height, "height")?;
    let pixels = u64::from(width) * u64::from(height);
    if pixels > MAX_RENDER_PIXELS {
        return Err(render_limit_error());
    }
    Ok((width, height))
}

fn canvas_dimension(value: f64, name: &str) -> Result<u32, String> {
    if !value.is_finite() || value <= 0. {
        return Err(format!("editor canvas {name} must be finite and positive"));
    }
    let rounded = value.round();
    if rounded > f64::from(MAX_RENDER_DIMENSION) {
        return Err(render_limit_error());
    }
    Ok(rounded.max(1.) as u32)
}

fn render_limit_error() -> String {
    format!(
        "editor rendering is limited to {MAX_RENDER_DIMENSION} pixels per side and {MAX_RENDER_PIXELS} total pixels"
    )
}

fn validate_image(
    image: &ImageElement,
    assets: &BTreeMap<String, Arc<RgbaImage>>,
) -> Result<(), String> {
    finite_f32(image.base.x, "x", &image.base.id)?;
    finite_f32(image.base.y, "y", &image.base.id)?;
    positive_f32(image.width, "width", &image.base.id)?;
    positive_f32(image.height, "height", &image.base.id)?;
    positive_f64(image.natural_width, "natural width", &image.base.id)?;
    positive_f64(image.natural_height, "natural height", &image.base.id)?;
    radians_to_degrees(image.base.rotation(), &image.base.id)?;
    if !image.base.opacity.is_finite() {
        return Err(format!(
            "image layer {} opacity must be finite",
            image.base.id
        ));
    }
    blend_mode(&image.base.blend_mode, &image.base.id)?;
    let asset = assets.get(&image.src).ok_or_else(|| {
        format!(
            "image layer {} is missing asset {}",
            image.base.id, image.src
        )
    })?;
    let (width, height) = asset.dimensions();
    if width == 0 || height == 0 {
        return Err(format!("image layer {} asset is empty", image.base.id));
    }
    if width > MAX_RENDER_DIMENSION
        || height > MAX_RENDER_DIMENSION
        || u64::from(width) * u64::from(height) > MAX_RENDER_PIXELS
    {
        return Err(format!(
            "image layer {} asset exceeds renderer limits",
            image.base.id
        ));
    }
    Ok(())
}

fn finite_f32(value: f64, name: &str, id: &str) -> Result<f32, String> {
    let converted = value as f32;
    if !value.is_finite() || !converted.is_finite() {
        return Err(format!("image layer {id} {name} must be finite"));
    }
    Ok(converted)
}

fn positive_f32(value: f64, name: &str, id: &str) -> Result<f32, String> {
    let converted = finite_f32(value, name, id)?;
    if converted <= 0. {
        return Err(format!("image layer {id} {name} must be positive"));
    }
    Ok(converted)
}

fn positive_f64(value: f64, name: &str, id: &str) -> Result<(), String> {
    if !value.is_finite() || value <= 0. {
        return Err(format!(
            "image layer {id} {name} must be finite and positive"
        ));
    }
    Ok(())
}

fn radians_to_degrees(radians: f64, id: &str) -> Result<f32, String> {
    let degrees = radians.to_degrees() as f32;
    if !radians.is_finite() || !degrees.is_finite() {
        return Err(format!("image layer {id} rotation must be finite"));
    }
    Ok(degrees)
}

fn blend_mode(value: &str, id: &str) -> Result<BlendMode, String> {
    match value {
        "source-over" => Ok(BlendMode::Normal),
        "multiply" => Ok(BlendMode::Multiply),
        "screen" => Ok(BlendMode::Screen),
        "overlay" => Ok(BlendMode::Overlay),
        "darken" => Ok(BlendMode::Darken),
        "lighten" => Ok(BlendMode::Lighten),
        _ => Err(format!(
            "image layer {id} has unsupported blend mode {value}"
        )),
    }
}

fn unsupported_layer(kind: &str, id: &str) -> String {
    format!("visible {kind} layer {id} is not supported by the image renderer")
}

fn parse_color(value: &str) -> Result<Rgba<u8>, String> {
    let hex = value
        .trim()
        .strip_prefix('#')
        .ok_or_else(|| format!("unsupported editor background color {value}"))?;
    if !hex.is_ascii() {
        return Err(format!("unsupported editor background color {value}"));
    }
    let expanded;
    let hex = match hex.len() {
        3 | 4 => {
            expanded = hex
                .chars()
                .flat_map(|character| [character, character])
                .collect::<String>();
            expanded.as_str()
        }
        6 | 8 => hex,
        _ => return Err(format!("unsupported editor background color {value}")),
    };
    let channel = |start| {
        u8::from_str_radix(&hex[start..start + 2], 16)
            .map_err(|_| format!("unsupported editor background color {value}"))
    };
    Ok(Rgba([
        channel(0)?,
        channel(2)?,
        channel(4)?,
        if hex.len() == 8 { channel(6)? } else { 255 },
    ]))
}

fn oriented_asset(asset: &Arc<RgbaImage>, orientation: ImageOrientation) -> Arc<RgbaImage> {
    if orientation == ImageOrientation::Normal {
        return asset.clone();
    }
    let (width, height) = asset.dimensions();
    let swaps_axes = matches!(
        orientation,
        ImageOrientation::Rotate90
            | ImageOrientation::Rotate270
            | ImageOrientation::Transpose
            | ImageOrientation::Transverse
    );
    let (output_width, output_height) = if swaps_axes {
        (height, width)
    } else {
        (width, height)
    };
    Arc::new(RgbaImage::from_fn(output_width, output_height, |x, y| {
        let (source_x, source_y) = match orientation {
            ImageOrientation::Normal => unreachable!(),
            ImageOrientation::Rotate90 => (y, height - 1 - x),
            ImageOrientation::Rotate180 => (width - 1 - x, height - 1 - y),
            ImageOrientation::Rotate270 => (width - 1 - y, x),
            ImageOrientation::FlipHorizontal => (width - 1 - x, y),
            ImageOrientation::FlipVertical => (x, height - 1 - y),
            ImageOrientation::Transpose => (y, x),
            ImageOrientation::Transverse => (width - 1 - y, height - 1 - x),
        };
        *asset.get_pixel(source_x, source_y)
    }))
}
