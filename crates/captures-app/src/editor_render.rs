//! Pixel rendering for the shared screenshot-editor document.
//!
//! Image assets are supplied by exact document `src`, so rendering performs no
//! filesystem, network, host-font, or UI access. The five closed annotation
//! shapes are rendered; text, open shapes, and freehand paths remain explicit
//! unsupported cases.

use std::{collections::BTreeMap, sync::Arc};

use captures_image::{BlendMode, Layer, Point, Shape};
use image::{Rgba, RgbaImage};

use crate::editor::{Document, Element, ImageElement, ImageOrientation, ShapeElement};

pub const MAX_RENDER_DIMENSION: u32 = 16_384;
pub const MAX_RENDER_PIXELS: u64 = 100_000_000;

/// Render supported visible layers in document order through `captures-image`.
///
/// The input document and shared assets are borrowed and never mutated. Text,
/// line/arrow, and path rendering will arrive in later slices and is rejected
/// while visible rather than silently omitted.
pub fn render(
    document: &Document,
    assets: &BTreeMap<String, Arc<RgbaImage>>,
) -> Result<RgbaImage, String> {
    let (width, height) = validate_canvas(document)?;
    let background = document
        .background
        .as_deref()
        .map(|color| parse_color(color, "editor background"))
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
            Element::Shape(shape) => validate_shape(shape)?,
            Element::Path(path) => return Err(unsupported_layer("path", &path.base.id)),
        }
    }

    let mut layers = Vec::new();
    for (index, element) in document.elements.iter().enumerate() {
        if !element.base().visible {
            continue;
        }
        let id = u64::try_from(index).map_err(|_| "too many editor layers to render".to_owned())?;
        match element {
            Element::Image(image) => layers.push(image_layer(id, image, assets)?),
            Element::Shape(shape) => layers.push(shape_layer(id, shape)?),
            Element::Text(_) | Element::Path(_) => {}
        }
    }

    captures_image::render(&captures_image::Document {
        source: Arc::new(RgbaImage::from_pixel(width, height, background)),
        crop: None,
        layers,
    })
}

fn image_layer(
    id: u64,
    image: &ImageElement,
    assets: &BTreeMap<String, Arc<RgbaImage>>,
) -> Result<Layer, String> {
    let pixels = oriented_asset(
        assets
            .get(&image.src)
            .expect("visible image assets were validated"),
        image.resolved_orientation(),
    );
    Ok(Layer {
        id,
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
        rotation_degrees: radians_to_degrees(image.base.rotation(), "image", &image.base.id)?,
        rotation_origin: None,
        blend_mode: blend_mode(&image.base.blend_mode, "image", &image.base.id)?,
    })
}

fn shape_layer(id: u64, element: &ShapeElement) -> Result<Layer, String> {
    let left = element.base.x.min(element.end_x);
    let top = element.base.y.min(element.end_y);
    let width = (element.end_x - element.base.x).abs();
    let height = (element.end_y - element.base.y).abs();
    let origin = Point {
        x: finite_shape_f32(left, "left", &element.base.id)?,
        y: finite_shape_f32(top, "top", &element.base.id)?,
    };
    let width = positive_shape_f32(width, "width", &element.base.id)?;
    let height = positive_shape_f32(height, "height", &element.base.id)?;
    let center = Point {
        x: origin.x + width / 2.,
        y: origin.y + height / 2.,
    };
    let shape = match element.shape.as_str() {
        "rectangle" => Shape::RoundedRectangle {
            origin,
            width,
            height,
            radius: 12_f32.min(width / 6.).min(height / 6.),
        },
        "ellipse" => Shape::Ellipse {
            origin,
            width,
            height,
        },
        "triangle" => Shape::Polygon(vec![
            Point {
                x: center.x,
                y: origin.y,
            },
            Point {
                x: origin.x + width,
                y: origin.y + height,
            },
            Point {
                x: origin.x,
                y: origin.y + height,
            },
        ]),
        "diamond" => Shape::Polygon(vec![
            Point {
                x: center.x,
                y: origin.y,
            },
            Point {
                x: origin.x + width,
                y: center.y,
            },
            Point {
                x: center.x,
                y: origin.y + height,
            },
            Point {
                x: origin.x,
                y: center.y,
            },
        ]),
        "star" => Shape::Polygon(
            (0..10)
                .map(|index| {
                    let angle =
                        -std::f32::consts::FRAC_PI_2 + index as f32 * std::f32::consts::PI / 5.;
                    let radius = if index % 2 == 0 { 1. } else { 0.39 };
                    Point {
                        x: center.x + angle.cos() * width / 2. * radius,
                        y: center.y + angle.sin() * height / 2. * radius,
                    }
                })
                .collect(),
        ),
        _ => unreachable!("visible shape kinds were validated"),
    };
    let opacity = element.base.opacity.clamp(0., 100.);
    let stroke = parse_color(
        &element.style.color,
        &format!("shape layer {} stroke", element.base.id),
    )?;
    let fill = element
        .style
        .fill
        .as_deref()
        .map(|fill| parse_color(fill, &format!("shape layer {} fill", element.base.id)))
        .transpose()?;
    Ok(Layer {
        id,
        shape,
        color: apply_opacity(stroke, opacity),
        stroke_width: if element.style.has_stroke() {
            element.style.stroke_width as f32
        } else {
            0.
        },
        fill: fill.map(|color| apply_opacity(color, opacity)),
        rotation_degrees: radians_to_degrees(element.base.rotation(), "shape", &element.base.id)?,
        rotation_origin: Some(center),
        blend_mode: blend_mode(&element.base.blend_mode, "shape", &element.base.id)?,
    })
}

fn apply_opacity(color: Rgba<u8>, opacity: f64) -> [u8; 4] {
    let [red, green, blue, alpha] = color.0;
    [
        red,
        green,
        blue,
        (f64::from(alpha) * opacity / 100.).round() as u8,
    ]
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
    radians_to_degrees(image.base.rotation(), "image", &image.base.id)?;
    if !image.base.opacity.is_finite() {
        return Err(format!(
            "image layer {} opacity must be finite",
            image.base.id
        ));
    }
    blend_mode(&image.base.blend_mode, "image", &image.base.id)?;
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

fn validate_shape(element: &ShapeElement) -> Result<(), String> {
    finite_shape_f32(element.base.x, "x", &element.base.id)?;
    finite_shape_f32(element.base.y, "y", &element.base.id)?;
    finite_shape_f32(element.end_x, "end x", &element.base.id)?;
    finite_shape_f32(element.end_y, "end y", &element.base.id)?;
    positive_shape_f32(
        (element.end_x - element.base.x).abs(),
        "width",
        &element.base.id,
    )?;
    positive_shape_f32(
        (element.end_y - element.base.y).abs(),
        "height",
        &element.base.id,
    )?;
    radians_to_degrees(element.base.rotation(), "shape", &element.base.id)?;
    if !element.base.opacity.is_finite() {
        return Err(format!(
            "shape layer {} opacity must be finite",
            element.base.id
        ));
    }
    blend_mode(&element.base.blend_mode, "shape", &element.base.id)?;
    if !matches!(
        element.shape.as_str(),
        "rectangle" | "ellipse" | "triangle" | "diamond" | "star"
    ) {
        return Err(format!(
            "shape layer {} has unsupported shape kind {}",
            element.base.id, element.shape
        ));
    }
    if element.style.has_drop_shadow() {
        return Err(format!(
            "shape layer {} uses unsupported drop shadow",
            element.base.id
        ));
    }
    let stroke_width = element.style.stroke_width as f32;
    if !element.style.stroke_width.is_finite()
        || !stroke_width.is_finite()
        || element.style.stroke_width < 0.
        || (element.style.has_stroke() && element.style.stroke_width == 0.)
    {
        return Err(format!(
            "shape layer {} stroke width must be finite and positive when enabled",
            element.base.id
        ));
    }
    parse_color(
        &element.style.color,
        &format!("shape layer {} stroke", element.base.id),
    )?;
    if let Some(fill) = element.style.fill.as_deref() {
        parse_color(fill, &format!("shape layer {} fill", element.base.id))?;
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

fn finite_shape_f32(value: f64, name: &str, id: &str) -> Result<f32, String> {
    let converted = value as f32;
    if !value.is_finite() || !converted.is_finite() {
        return Err(format!("shape layer {id} {name} must be finite"));
    }
    Ok(converted)
}

fn positive_shape_f32(value: f64, name: &str, id: &str) -> Result<f32, String> {
    let converted = finite_shape_f32(value, name, id)?;
    if converted <= 0. {
        return Err(format!("shape layer {id} {name} must be positive"));
    }
    Ok(converted)
}

fn radians_to_degrees(radians: f64, kind: &str, id: &str) -> Result<f32, String> {
    let degrees = radians.to_degrees() as f32;
    if !radians.is_finite() || !degrees.is_finite() {
        return Err(format!("{kind} layer {id} rotation must be finite"));
    }
    Ok(degrees)
}

fn blend_mode(value: &str, kind: &str, id: &str) -> Result<BlendMode, String> {
    match value {
        "source-over" => Ok(BlendMode::Normal),
        "multiply" => Ok(BlendMode::Multiply),
        "screen" => Ok(BlendMode::Screen),
        "overlay" => Ok(BlendMode::Overlay),
        "darken" => Ok(BlendMode::Darken),
        "lighten" => Ok(BlendMode::Lighten),
        _ => Err(format!(
            "{kind} layer {id} has unsupported blend mode {value}"
        )),
    }
}

fn unsupported_layer(kind: &str, id: &str) -> String {
    format!("visible {kind} layer {id} is not supported by the image renderer")
}

fn parse_color(value: &str, description: &str) -> Result<Rgba<u8>, String> {
    let error = || format!("unsupported {description} color {value}");
    let hex = value.trim().strip_prefix('#').ok_or_else(&error)?;
    if !hex.is_ascii() {
        return Err(error());
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
        _ => return Err(error()),
    };
    let channel = |start| u8::from_str_radix(&hex[start..start + 2], 16).map_err(|_| error());
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

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;
    use crate::editor::{Point as EditorPoint, Rect};

    #[derive(Deserialize)]
    struct FixtureCase {
        element: ShapeElement,
        expected: ExpectedGeometry,
    }

    #[derive(Deserialize)]
    struct ExpectedGeometry {
        variant: String,
        rect: Rect,
        radius: Option<f64>,
        points: Vec<EditorPoint>,
    }

    fn close(actual: f32, expected: f64) {
        assert!(
            (f64::from(actual) - expected).abs() < 0.000_02,
            "{actual} != {expected}"
        );
    }

    #[test]
    fn closed_shape_geometry_matches_typescript_fixture() {
        let cases: Vec<FixtureCase> =
            serde_json::from_str(include_str!("../tests/editor-shape-golden.json")).unwrap();
        for (id, case) in cases.iter().enumerate() {
            let layer = shape_layer(id as u64, &case.element).unwrap();
            assert_eq!(layer.color, [18, 86, 170, 137]);
            assert_eq!(layer.fill, Some([239, 113, 57, 120]));
            assert_eq!(layer.stroke_width, 3.25);
            assert_eq!(layer.blend_mode, BlendMode::Screen);
            let center = layer.rotation_origin.unwrap();
            close(
                center.x,
                case.expected.rect.x + case.expected.rect.width / 2.,
            );
            close(
                center.y,
                case.expected.rect.y + case.expected.rect.height / 2.,
            );
            match (&layer.shape, case.expected.variant.as_str()) {
                (
                    Shape::RoundedRectangle {
                        origin,
                        width,
                        height,
                        radius,
                    },
                    "rounded-rectangle",
                ) => {
                    close(origin.x, case.expected.rect.x);
                    close(origin.y, case.expected.rect.y);
                    close(*width, case.expected.rect.width);
                    close(*height, case.expected.rect.height);
                    close(*radius, case.expected.radius.unwrap());
                }
                (
                    Shape::Ellipse {
                        origin,
                        width,
                        height,
                    },
                    "ellipse",
                ) => {
                    close(origin.x, case.expected.rect.x);
                    close(origin.y, case.expected.rect.y);
                    close(*width, case.expected.rect.width);
                    close(*height, case.expected.rect.height);
                }
                (Shape::Polygon(actual), "polygon") => {
                    assert_eq!(actual.len(), case.expected.points.len());
                    for (actual, expected) in actual.iter().zip(&case.expected.points) {
                        close(actual.x, expected.x);
                        close(actual.y, expected.y);
                    }
                }
                (actual, expected) => panic!("unexpected {actual:?} for {expected}"),
            }
        }
    }
}
