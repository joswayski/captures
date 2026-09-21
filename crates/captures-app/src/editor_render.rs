//! Pixel rendering for the shared screenshot-editor document.
//!
//! Image assets are supplied by exact document `src`, so rendering performs no
//! filesystem, network, host-font, or UI access. The five closed annotation
//! shapes, curved lines, tapered arrows, and freehand paths are rendered. Filled
//! text and plates additionally require explicit fonts and family mapping.

use std::{borrow::Cow, collections::BTreeMap, sync::Arc};

use captures_image::{
    BlendMode, DROP_SHADOW_BLUR_MAX, DROP_SHADOW_OFFSET_MAX, DropShadow, Layer, Point, Shape,
    text::{TextRenderer, TextStyle},
};
use image::{Rgba, RgbaImage};

use crate::editor::{
    Document, DropShadowStyle, Element, ElementStyle, ImageElement, ImageOrientation, PathElement,
    Point as EditorPoint, ShapeElement, TextElement, arrow_fill_polygon, sample_controlled_path,
};

pub const MAX_RENDER_DIMENSION: u32 = 16_384;
pub const MAX_RENDER_PIXELS: u64 = 100_000_000;

/// Render supported visible layers in document order through `captures-image`.
///
/// The input document and shared assets are borrowed and never mutated. Text
/// is rejected rather than silently omitted.
pub fn render(
    document: &Document,
    assets: &BTreeMap<String, Arc<RgbaImage>>,
) -> Result<RgbaImage, String> {
    render_inner(document, assets, None)
}

/// Opt in to filled paragraph text using caller-owned fonts. `families` maps
/// document family keys (such as `sans`) to names embedded in supplied font bytes.
/// No installed fonts are scanned. Outlines remain explicit errors. Text and
/// plates rotate together around the shipping selection pivot; shadow offsets
/// stay in canvas space. Plates shadow once, otherwise glyph lines shadow first
/// and all crisp glyph passes follow, matching shipping paint order.
/// Retained text bitmaps are limited to 16M pixels across the visible document.
pub fn render_with_text(
    document: &Document,
    assets: &BTreeMap<String, Arc<RgbaImage>>,
    renderer: &mut TextRenderer,
    families: &BTreeMap<String, String>,
) -> Result<RgbaImage, String> {
    render_inner(document, assets, Some((renderer, families)))
}

fn render_inner(
    document: &Document,
    assets: &BTreeMap<String, Arc<RgbaImage>>,
    mut text: Option<(&mut TextRenderer, &BTreeMap<String, String>)>,
) -> Result<RgbaImage, String> {
    let (width, height) = validate_canvas(document)?;
    let background = document
        .background
        .as_deref()
        .map(|color| parse_color(color, "editor background"))
        .transpose()?
        .unwrap_or(Rgba([0, 0, 0, 0]));

    // Reject unsupported visible styles/assets before orientation or canvas
    // allocation. Shaping/rasterization can still fail on missing glyphs/budgets.
    for element in document
        .elements
        .iter()
        .filter(|element| element.base().visible)
    {
        match element {
            Element::Image(image) => validate_image(image, assets)?,
            Element::Text(element) => {
                let Some((_, families)) = text.as_ref() else {
                    return Err(unsupported_layer("text", &element.base.id));
                };
                validate_text(element, families)?;
            }
            Element::Shape(shape) => validate_shape(shape)?,
            Element::Path(path) => validate_path(path)?,
        }
    }

    let mut layers = Vec::new();
    let mut shadows = BTreeMap::new();
    let mut text_pixels_remaining = 16_777_216;
    let mut next_text_id = document.elements.len() as u64;
    for (index, element) in document.elements.iter().enumerate() {
        if !element.base().visible {
            continue;
        }
        let id = u64::try_from(index).map_err(|_| "too many editor layers to render".to_owned())?;
        match element {
            Element::Image(image) => layers.push(image_layer(id, image, assets)?),
            Element::Shape(shape) => {
                layers.push(shape_layer(id, shape)?);
                if shape.style.has_drop_shadow() {
                    shadows.insert(id, drop_shadow(&shape.style));
                }
            }
            Element::Path(path) => {
                layers.push(path_layer(id, path)?);
                if path.style.has_drop_shadow() {
                    shadows.insert(id, drop_shadow(&path.style));
                }
            }
            Element::Text(element) => {
                let (renderer, families) = text.as_mut().expect("validated text context");
                let paints =
                    text_layers(id, element, renderer, families, &mut text_pixels_remaining)?;
                let plate = matches!(
                    paints.first().map(|layer| &layer.shape),
                    Some(Shape::RoundedRectangle { .. })
                );
                let shadow = element.has_drop_shadow().then(|| {
                    drop_shadow(&crate::editor_text::shadow_style(
                        element,
                        element.font_size,
                    ))
                });
                let mut crisp = Vec::new();
                for (index, mut layer) in paints.into_iter().enumerate() {
                    // Preserve document-index IDs and keep the extra text paints
                    // disjoint from every shape/path shadow key.
                    if index > 0 {
                        layer.id = next_text_id;
                        next_text_id += 1;
                    }
                    if let Some(shadow) = shadow.filter(|_| !plate || index == 0) {
                        shadows.insert(layer.id, shadow);
                        if !plate {
                            let mut copy = layer.clone(); // Shared immutable bitmap, no second raster allocation.
                            copy.id = next_text_id;
                            next_text_id += 1;
                            crisp.push(copy);
                        }
                    }
                    layers.push(layer);
                }
                layers.extend(crisp);
            }
        }
    }

    captures_image::render_with_shadows(
        &captures_image::Document {
            source: Arc::new(RgbaImage::from_pixel(width, height, background)),
            crop: None,
            layers,
        },
        &shadows,
    )
}

fn validate_text(element: &TextElement, families: &BTreeMap<String, String>) -> Result<(), String> {
    let id = &element.base.id;
    if element.outlined {
        return Err(format!("text layer {id} outlines are not supported yet"));
    }
    for (axis, value) in [
        ("x", element.base.x),
        ("y", element.base.y),
        ("width", element.width),
    ] {
        finite_layer_f32(value, "text", axis, id)?;
    }
    if !element.base.opacity.is_finite() {
        return Err(format!("text layer {id} opacity must be finite"));
    }
    blend_mode(&element.base.blend_mode, "text", id)?;
    parse_color(&element.color, "text")?;
    if let Some(background) = element.background.as_deref().filter(|s| !s.is_empty()) {
        parse_color(background, "text background")?;
    }
    if !families.contains_key(&element.font_family) {
        return Err(format!(
            "text layer {id} has no explicit font family mapping"
        ));
    }
    Ok(())
}

// Canvas's text preparation replaces ASCII whitespace with spaces. Paragraph
// breaks are handled before this; normalize both measurement and rasterization.
// Other line-control characters remain explicit errors from the single-line shaper.
fn canvas_text_line(line: &str) -> Cow<'_, str> {
    if line.is_empty() {
        Cow::Borrowed(" ")
    } else if line.contains(['\t', '\n', '\r', '\u{000c}']) {
        Cow::Owned(line.replace(['\t', '\n', '\r', '\u{000c}'], " "))
    } else {
        Cow::Borrowed(line)
    }
}

/// Validate property edits even on hidden text, using the same fonts and Canvas
/// normalization as paint. Content/type edits refit; color/alignment edits do not.
pub(crate) fn prepare_text_edit(
    element: &TextElement,
    refit: bool,
    renderer: &mut TextRenderer,
    families: &BTreeMap<String, String>,
) -> Result<TextElement, String> {
    validate_text(element, families)?;
    let style = TextStyle {
        family: &families[&element.font_family],
        size: element.font_size as f32,
        bold: element.bold,
        italic: element.italic,
        color: parse_color(&element.color, "text")?.0,
    };
    let mut measure = |line: &str| {
        renderer
            .measure_line(&canvas_text_line(line), &style)
            .map(f64::from)
    };
    let fitted = if refit {
        crate::editor_text::fit_auto_width(element, true, &mut measure)?
    } else {
        element.clone()
    };
    crate::editor_text::layout(&fitted, measure)?;
    Ok(fitted)
}

fn text_layers(
    id: u64,
    element: &TextElement,
    renderer: &mut TextRenderer,
    families: &BTreeMap<String, String>,
    pixels_remaining: &mut u64,
) -> Result<Vec<Layer>, String> {
    let color = |value: &str| -> Result<[u8; 4], String> {
        let mut rgba = parse_color(value, "text")?.0;
        rgba[3] = (f64::from(rgba[3]) * element.base.opacity.clamp(0., 100.) / 100.).round() as u8;
        Ok(rgba)
    };
    let style = TextStyle {
        family: &families[&element.font_family],
        size: element.font_size as f32,
        bold: element.bold,
        italic: element.italic,
        color: color(&element.color)?,
    };
    let layout = crate::editor_text::layout(element, |line| {
        renderer
            .measure_line(&canvas_text_line(line), &style)
            .map(f64::from)
    })?;
    let mode = blend_mode(&element.base.blend_mode, "text", &element.base.id)?;
    let number = |value, axis| finite_layer_f32(value, "text", axis, &element.base.id);
    let bounds = crate::editor_text::selection_bounds(element)?;
    let rotation = radians_to_degrees(element.base.rotation(), "text", &element.base.id)?;
    let origin = Point {
        x: number(bounds.x + bounds.width / 2., "rotation x")?,
        y: number(bounds.y + bounds.height / 2., "rotation y")?,
    };
    let layer = |shape, color, fill| Layer {
        id,
        shape,
        color,
        fill,
        stroke_width: 0.,
        rotation_degrees: rotation,
        rotation_origin: Some(origin),
        blend_mode: mode,
    };
    let mut layers = Vec::new();
    if let Some(plate) = layout.plate {
        let fill = color(element.background.as_deref().expect("layout plate color"))?;
        layers.push(layer(
            Shape::RoundedRectangle {
                origin: Point {
                    x: number(plate.bounds.x, "plate x")?,
                    y: number(plate.bounds.y, "plate y")?,
                },
                width: number(plate.bounds.width, "plate width")?,
                height: number(plate.bounds.height, "plate height")?,
                radius: number(plate.radius, "plate radius")?,
            },
            fill,
            Some(fill),
        ));
    }
    for row in layout.rows {
        let line = renderer.render_line(&canvas_text_line(&row.text), &style)?;
        if line.pixels.width() == 0 || line.pixels.height() == 0 {
            continue;
        }
        *pixels_remaining = pixels_remaining
            .checked_sub(u64::from(line.pixels.width()) * u64::from(line.pixels.height()))
            .ok_or("Text exceeds the document's 16M raster pixel budget.")?;
        // Center actual raster ink vertically, preserving horizontal bearings.
        // The integer ink box can differ subpixel-wise from Canvas outline metrics.
        let x = row.x + f64::from(line.bounds.x);
        let y = row.y + (element.font_size * 1.25 - f64::from(line.bounds.height)) / 2.;
        layers.push(layer(
            Shape::Image {
                origin: Point {
                    x: number(x, "ink x")?,
                    y: number(y, "ink y")?,
                },
                width: line.pixels.width() as f32,
                height: line.pixels.height() as f32,
                pixels: Arc::new(line.pixels),
            },
            [255; 4],
            None,
        ));
    }
    Ok(layers)
}

fn drop_shadow(style: &ElementStyle) -> DropShadow {
    const DEFAULT_OPACITY: f64 = 45.0;

    let width = style.stroke_width.max(1.0);
    let fallback = DropShadowStyle {
        color: "#000000".into(),
        opacity: DEFAULT_OPACITY,
        blur: (width * 0.85).max(6.0),
        offset_x: 0.0,
        offset_y: (width * 0.32).round().max(2.0),
        extra: Default::default(),
    };
    let custom = style.drop_shadow_style.as_ref().unwrap_or(&fallback);
    let number = |value: f64, min: f64, max: f64, fallback: f64| {
        if value.is_finite() {
            value.clamp(min, max)
        } else {
            fallback
        }
    };
    let [red, green, blue] = parse_shadow_color(&custom.color).unwrap_or([0, 0, 0]);
    DropShadow {
        color: [
            red,
            green,
            blue,
            (number(custom.opacity, 0.0, 100.0, fallback.opacity) * 2.55).round() as u8,
        ],
        blur: number(
            custom.blur,
            0.0,
            f64::from(DROP_SHADOW_BLUR_MAX),
            fallback.blur,
        ) as f32,
        offset_x: number(
            custom.offset_x,
            -f64::from(DROP_SHADOW_OFFSET_MAX),
            f64::from(DROP_SHADOW_OFFSET_MAX),
            fallback.offset_x,
        ) as f32,
        offset_y: number(
            custom.offset_y,
            -f64::from(DROP_SHADOW_OFFSET_MAX),
            f64::from(DROP_SHADOW_OFFSET_MAX),
            fallback.offset_y,
        ) as f32,
    }
}

fn parse_shadow_color(value: &str) -> Option<[u8; 3]> {
    let raw = value.trim().strip_prefix('#').unwrap_or(value.trim());
    if !raw.is_ascii() {
        return None;
    }
    let expanded;
    let hex = if raw.len() == 3 {
        expanded = raw
            .chars()
            .flat_map(|character| [character, character])
            .collect::<String>();
        expanded.as_str()
    } else {
        raw.get(..6)?
    };
    let channel = |start| u8::from_str_radix(&hex[start..start + 2], 16).ok();
    Some([channel(0)?, channel(2)?, channel(4)?])
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
    if matches!(element.shape.as_str(), "line" | "arrow") {
        return open_shape_layer(id, element);
    }
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

fn open_shape_layer(id: u64, element: &ShapeElement) -> Result<Layer, String> {
    let vertices = std::iter::once(EditorPoint {
        x: element.base.x,
        y: element.base.y,
    })
    .chain(element.controls.iter().copied())
    .chain(std::iter::once(EditorPoint {
        x: element.end_x,
        y: element.end_y,
    }))
    .collect::<Vec<_>>();
    let opacity = element.base.opacity.clamp(0., 100.);
    let color = apply_opacity(
        parse_color(
            &element.style.color,
            &format!("shape layer {} stroke", element.base.id),
        )?,
        opacity,
    );
    let (shape, stroke_width, fill, rotation_origin) = if element.shape == "line" {
        let samples = sample_controlled_path(&vertices, 48);
        (
            Shape::ControlledPath(capture_points(&vertices, "shape", &element.base.id)?),
            element.style.stroke_width as f32,
            None,
            center_of_points(&samples, element.base.x, element.base.y),
        )
    } else {
        let polygon = arrow_fill_polygon(element);
        if polygon.len() < 3 {
            let samples = sample_controlled_path(&vertices, 48);
            (
                Shape::SmoothPath(Vec::new()),
                0.,
                None,
                center_of_points(&samples, element.base.x, element.base.y),
            )
        } else {
            let center = center_of_points(&polygon, element.base.x, element.base.y);
            (
                Shape::TaperedArrow(capture_points(&polygon, "shape", &element.base.id)?),
                (element.style.stroke_width * 0.06).max(0.6) as f32,
                Some(color),
                center,
            )
        }
    };
    Ok(Layer {
        id,
        shape,
        color,
        stroke_width,
        fill,
        rotation_degrees: radians_to_degrees(element.base.rotation(), "shape", &element.base.id)?,
        rotation_origin: Some(editor_point_to_capture(
            rotation_origin,
            "shape rotation origin",
            &element.base.id,
        )?),
        blend_mode: blend_mode(&element.base.blend_mode, "shape", &element.base.id)?,
    })
}

fn path_layer(id: u64, element: &PathElement) -> Result<Layer, String> {
    let opacity = element.base.opacity.clamp(0., 100.);
    let color = apply_opacity(
        parse_color(
            &element.style.color,
            &format!("path layer {} stroke", element.base.id),
        )?,
        opacity,
    );
    let center = center_of_points(&element.points, element.base.x, element.base.y);
    Ok(Layer {
        id,
        shape: Shape::SmoothPath(capture_points(&element.points, "path", &element.base.id)?),
        color,
        stroke_width: element.style.stroke_width as f32,
        fill: None,
        rotation_degrees: radians_to_degrees(element.base.rotation(), "path", &element.base.id)?,
        rotation_origin: Some(editor_point_to_capture(
            center,
            "path rotation origin",
            &element.base.id,
        )?),
        blend_mode: blend_mode(&element.base.blend_mode, "path", &element.base.id)?,
    })
}

fn editor_point_to_capture(point: EditorPoint, name: &str, id: &str) -> Result<Point, String> {
    Ok(Point {
        x: finite_layer_f32(point.x, name, "x", id)?,
        y: finite_layer_f32(point.y, name, "y", id)?,
    })
}

fn capture_points(points: &[EditorPoint], kind: &str, id: &str) -> Result<Vec<Point>, String> {
    points
        .iter()
        .copied()
        .map(|point| editor_point_to_capture(point, kind, id))
        .collect()
}

fn center_of_points(points: &[EditorPoint], fallback_x: f64, fallback_y: f64) -> EditorPoint {
    let Some(first) = points.first() else {
        return EditorPoint {
            x: fallback_x + 0.5,
            y: fallback_y + 0.5,
        };
    };
    let (mut left, mut top, mut right, mut bottom) = (first.x, first.y, first.x, first.y);
    for point in &points[1..] {
        left = left.min(point.x);
        top = top.min(point.y);
        right = right.max(point.x);
        bottom = bottom.max(point.y);
    }
    EditorPoint {
        x: left + (right - left).max(1.) / 2.,
        y: top + (bottom - top).max(1.) / 2.,
    }
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
    for point in &element.controls {
        finite_shape_f32(point.x, "control x", &element.base.id)?;
        finite_shape_f32(point.y, "control y", &element.base.id)?;
    }
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
        "rectangle" | "ellipse" | "triangle" | "diamond" | "star" | "line" | "arrow"
    ) {
        return Err(format!(
            "shape layer {} has unsupported shape kind {}",
            element.base.id, element.shape
        ));
    }
    let closed = matches!(
        element.shape.as_str(),
        "rectangle" | "ellipse" | "triangle" | "diamond" | "star"
    );
    if closed {
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
    }
    let stroke_width = element.style.stroke_width as f32;
    if !element.style.stroke_width.is_finite()
        || !stroke_width.is_finite()
        || element.style.stroke_width < 0.
        || ((!closed || element.style.has_stroke()) && element.style.stroke_width == 0.)
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

fn validate_path(element: &PathElement) -> Result<(), String> {
    finite_layer_f32(element.base.x, "path", "x", &element.base.id)?;
    finite_layer_f32(element.base.y, "path", "y", &element.base.id)?;
    for point in &element.points {
        finite_layer_f32(point.x, "path point", "x", &element.base.id)?;
        finite_layer_f32(point.y, "path point", "y", &element.base.id)?;
    }
    radians_to_degrees(element.base.rotation(), "path", &element.base.id)?;
    if !element.base.opacity.is_finite() {
        return Err(format!(
            "path layer {} opacity must be finite",
            element.base.id
        ));
    }
    blend_mode(&element.base.blend_mode, "path", &element.base.id)?;
    let stroke_width = element.style.stroke_width as f32;
    if !element.style.stroke_width.is_finite()
        || !stroke_width.is_finite()
        || element.style.stroke_width <= 0.
    {
        return Err(format!(
            "path layer {} stroke width must be finite and positive",
            element.base.id
        ));
    }
    parse_color(
        &element.style.color,
        &format!("path layer {} stroke", element.base.id),
    )?;
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

fn finite_layer_f32(value: f64, description: &str, axis: &str, id: &str) -> Result<f32, String> {
    let converted = value as f32;
    if !value.is_finite() || !converted.is_finite() {
        return Err(format!("{description} layer {id} {axis} must be finite"));
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

    #[derive(Deserialize)]
    struct StrokeFixtureCase {
        element: ShapeElement,
        expected: ExpectedStrokeGeometry,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct ExpectedStrokeGeometry {
        samples48: PointSummary,
        polygon: PointSummary,
        rotation_origin: EditorPoint,
    }

    #[derive(Deserialize)]
    struct PointSummary {
        count: usize,
        checkpoints: Vec<PointCheckpoint>,
        bounds: Option<Rect>,
    }

    #[derive(Deserialize)]
    struct PointCheckpoint {
        index: usize,
        point: EditorPoint,
    }

    fn close(actual: f32, expected: f64) {
        assert!(
            (f64::from(actual) - expected).abs() < 0.000_02,
            "{actual} != {expected}"
        );
    }

    fn assert_summary(actual: &[EditorPoint], expected: &PointSummary) {
        assert_eq!(actual.len(), expected.count);
        for checkpoint in &expected.checkpoints {
            let actual = actual[checkpoint.index];
            assert!((actual.x - checkpoint.point.x).abs() < 1e-9);
            assert!((actual.y - checkpoint.point.y).abs() < 1e-9);
        }
        match expected.bounds {
            Some(expected) => {
                let first = actual[0];
                let (mut left, mut top, mut right, mut bottom) =
                    (first.x, first.y, first.x, first.y);
                for point in &actual[1..] {
                    left = left.min(point.x);
                    top = top.min(point.y);
                    right = right.max(point.x);
                    bottom = bottom.max(point.y);
                }
                assert!((left - expected.x).abs() < 1e-9);
                assert!((top - expected.y).abs() < 1e-9);
                assert!((right - left - expected.width).abs() < 1e-9);
                assert!((bottom - top - expected.height).abs() < 1e-9);
            }
            None => assert!(actual.is_empty()),
        }
    }

    fn assert_capture_summary(actual: &[Point], expected: &PointSummary) {
        assert_eq!(actual.len(), expected.count);
        for checkpoint in &expected.checkpoints {
            close(actual[checkpoint.index].x, checkpoint.point.x);
            close(actual[checkpoint.index].y, checkpoint.point.y);
        }
        match expected.bounds {
            Some(expected) => {
                let first = actual[0];
                let (mut left, mut top, mut right, mut bottom) =
                    (first.x, first.y, first.x, first.y);
                for point in &actual[1..] {
                    left = left.min(point.x);
                    top = top.min(point.y);
                    right = right.max(point.x);
                    bottom = bottom.max(point.y);
                }
                close(left, expected.x);
                close(top, expected.y);
                close(right - left, expected.width);
                close(bottom - top, expected.height);
            }
            None => assert!(actual.is_empty()),
        }
    }

    #[test]
    fn drop_shadow_metrics_match_shipping_defaults_and_custom_clamping() {
        let mut style = ElementStyle {
            color: "#fff".into(),
            fill: None,
            stroke_width: 8.0,
            stroke_enabled: None,
            drop_shadow: Some(true),
            drop_shadow_style: None,
            extra: Default::default(),
        };
        assert_eq!(
            drop_shadow(&style),
            DropShadow {
                color: [0, 0, 0, 115],
                blur: 6.8,
                offset_x: 0.0,
                offset_y: 3.0,
            }
        );

        style.drop_shadow_style = Some(DropShadowStyle {
            color: "#1aB2c3ff".into(),
            opacity: 120.0,
            blur: f64::NAN,
            offset_x: -700.0,
            offset_y: f64::INFINITY,
            extra: Default::default(),
        });
        assert_eq!(
            drop_shadow(&style),
            DropShadow {
                color: [0x1a, 0xb2, 0xc3, 255],
                blur: 6.8,
                offset_x: -500.0,
                offset_y: 3.0,
            }
        );

        style.drop_shadow_style.as_mut().unwrap().color = "invalid".into();
        assert_eq!(drop_shadow(&style).color[..3], [0, 0, 0]);
        style.drop_shadow_style.as_mut().unwrap().color = "#AéBCD".into();
        assert_eq!(drop_shadow(&style).color[..3], [0, 0, 0]);
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

    #[test]
    fn open_stroke_geometry_matches_typescript_fixture() {
        let cases: Vec<StrokeFixtureCase> =
            serde_json::from_str(include_str!("../tests/editor-stroke-golden.json")).unwrap();
        for (id, case) in cases.iter().enumerate() {
            let vertices = std::iter::once(EditorPoint {
                x: case.element.base.x,
                y: case.element.base.y,
            })
            .chain(case.element.controls.iter().copied())
            .chain(std::iter::once(EditorPoint {
                x: case.element.end_x,
                y: case.element.end_y,
            }))
            .collect::<Vec<_>>();
            let samples = sample_controlled_path(&vertices, 48);
            assert_summary(&samples, &case.expected.samples48);

            let layer = open_shape_layer(id as u64, &case.element).unwrap();
            assert_eq!(layer.color, [43, 113, 201, 132]);
            assert_eq!(layer.blend_mode, BlendMode::Overlay);
            close(
                layer.rotation_origin.unwrap().x,
                case.expected.rotation_origin.x,
            );
            close(
                layer.rotation_origin.unwrap().y,
                case.expected.rotation_origin.y,
            );
            match &layer.shape {
                Shape::ControlledPath(actual) => {
                    assert_eq!(case.element.shape, "line");
                    assert_eq!(layer.stroke_width, 7.25);
                    assert!(layer.fill.is_none());
                    assert_eq!(actual.len(), vertices.len());
                    for (actual, expected) in actual.iter().zip(&vertices) {
                        close(actual.x, expected.x);
                        close(actual.y, expected.y);
                    }
                }
                Shape::TaperedArrow(actual) => {
                    assert_eq!(case.element.shape, "arrow");
                    assert_eq!(layer.stroke_width, 0.6);
                    assert_eq!(layer.fill, Some(layer.color));
                    assert_capture_summary(actual, &case.expected.polygon);
                }
                actual => panic!("unexpected stroke shape {actual:?}"),
            }
        }
    }
}
