use std::{collections::BTreeMap, f64::consts::FRAC_PI_4, sync::Arc};

use captures_app::{
    editor::{
        Document, DropShadowStyle, Element, ElementBase, ElementStyle, ImageElement,
        ImageOrientation, OptionalNullable, PathElement, Point, Rect, ShapeElement,
    },
    editor_render::render,
};
use image::{Rgba, RgbaImage};
use serde_json::{Map, json};

fn base(id: &str) -> ElementBase {
    ElementBase {
        id: id.into(),
        x: 0.,
        y: 0.,
        rotation: None,
        locked: false,
        visible: true,
        opacity: 100.,
        blend_mode: "source-over".into(),
    }
}

fn image(id: &str, src: &str, width: f64, height: f64) -> ImageElement {
    ImageElement {
        base: base(id),
        source: "capture".into(),
        src: src.into(),
        original_src: OptionalNullable::Missing,
        name: id.into(),
        source_artifact_id: None,
        width,
        height,
        natural_width: width,
        natural_height: height,
        orientation: None,
        extra: Map::new(),
    }
}

fn shape(id: &str, kind: &str, start: (f64, f64), end: (f64, f64)) -> ShapeElement {
    let mut base = base(id);
    base.x = start.0;
    base.y = start.1;
    ShapeElement {
        base,
        shape: kind.into(),
        end_x: end.0,
        end_y: end.1,
        controls: vec![],
        style: ElementStyle {
            color: "#1464dc".into(),
            fill: Some("#e63c28".into()),
            stroke_width: 4.,
            stroke_enabled: None,
            drop_shadow: None,
            drop_shadow_style: None,
            extra: Map::new(),
        },
        extra: Map::new(),
    }
}

fn path(id: &str, points: &[(f64, f64)]) -> PathElement {
    PathElement {
        base: base(id),
        points: points.iter().map(|&(x, y)| Point { x, y }).collect(),
        style: ElementStyle {
            color: "#14b45a".into(),
            fill: None,
            stroke_width: 4.,
            stroke_enabled: None,
            drop_shadow: None,
            drop_shadow_style: None,
            extra: Map::new(),
        },
        extra: Map::new(),
    }
}

fn document(width: f64, height: f64, elements: Vec<Element>) -> Document {
    Document {
        width,
        height,
        background: None,
        elements,
        extra: Map::new(),
    }
}

fn assets(entries: &[(&str, RgbaImage)]) -> BTreeMap<String, Arc<RgbaImage>> {
    entries
        .iter()
        .map(|(key, pixels)| ((*key).into(), Arc::new(pixels.clone())))
        .collect()
}

fn pixel_image(width: u32, height: u32, pixels: &[[u8; 4]]) -> RgbaImage {
    assert_eq!(pixels.len(), (width * height) as usize);
    RgbaImage::from_raw(width, height, pixels.iter().flatten().copied().collect()).unwrap()
}

fn assert_pixel_near(actual: [u8; 4], expected: [u8; 4], tolerance: i16) {
    for (actual, expected) in actual.into_iter().zip(expected) {
        assert!(
            (i16::from(actual) - i16::from(expected)).abs() <= tolerance,
            "pixel channel {actual} differs from {expected} by more than {tolerance}"
        );
    }
}

#[test]
fn all_eight_orientations_place_asymmetric_rgba_pixels_losslessly() {
    const A: [u8; 4] = [255, 0, 0, 255];
    const B: [u8; 4] = [0, 255, 0, 128];
    const C: [u8; 4] = [0, 0, 255, 255];
    const D: [u8; 4] = [255, 255, 0, 64];
    const E: [u8; 4] = [255, 0, 255, 192];
    const F: [u8; 4] = [0, 255, 255, 255];
    let source = pixel_image(3, 2, &[A, B, C, D, E, F]);
    let cases = [
        (ImageOrientation::Normal, 3, 2, vec![A, B, C, D, E, F]),
        (ImageOrientation::Rotate90, 2, 3, vec![D, A, E, B, F, C]),
        (ImageOrientation::Rotate180, 3, 2, vec![F, E, D, C, B, A]),
        (ImageOrientation::Rotate270, 2, 3, vec![C, F, B, E, A, D]),
        (
            ImageOrientation::FlipHorizontal,
            3,
            2,
            vec![C, B, A, F, E, D],
        ),
        (ImageOrientation::FlipVertical, 3, 2, vec![D, E, F, A, B, C]),
        (ImageOrientation::Transpose, 2, 3, vec![A, D, B, E, C, F]),
        (ImageOrientation::Transverse, 2, 3, vec![F, C, E, B, D, A]),
    ];
    let assets = assets(&[("asset://asymmetric", source)]);

    for (orientation, width, height, expected) in cases {
        let mut layer = image(
            "oriented",
            "asset://asymmetric",
            f64::from(width),
            f64::from(height),
        );
        layer.orientation = Some(orientation);
        let rendered = render(
            &document(
                f64::from(width),
                f64::from(height),
                vec![Element::Image(layer)],
            ),
            &assets,
        )
        .unwrap();
        assert_eq!(rendered.dimensions(), (width, height), "{orientation:?}");
        for (actual, expected) in rendered.pixels().zip(expected) {
            // Premultiply/demultiply can round a straight-alpha channel by one.
            assert_pixel_near(actual.0, expected, 1);
        }
    }
}

#[test]
fn crop_geometry_clips_off_canvas_layers_and_preserves_inputs() {
    let source = pixel_image(
        4,
        2,
        &[
            [10, 0, 0, 255],
            [20, 0, 0, 255],
            [30, 0, 0, 255],
            [40, 0, 0, 255],
            [50, 0, 0, 255],
            [60, 0, 0, 255],
            [70, 0, 0, 255],
            [80, 0, 0, 255],
        ],
    );
    let assets = assets(&[("exact src with spaces", source)]);
    let mut layer = image("cropped", "exact src with spaces", 4., 2.);
    layer.base.x = 1.;
    layer.base.y = 1.;
    let mut doc = document(6., 4., vec![Element::Image(layer)]);
    doc.crop(Rect {
        x: 2.,
        y: 1.,
        width: 3.,
        height: 2.,
    });
    let original_document = doc.clone();
    let original_asset = assets["exact src with spaces"].clone();
    let original_pixels = original_asset.as_raw().clone();

    let rendered = render(&doc, &assets).unwrap();

    assert_eq!(rendered.dimensions(), (3, 2));
    assert_eq!(rendered.get_pixel(0, 0).0, [20, 0, 0, 255]);
    assert_eq!(rendered.get_pixel(1, 0).0, [30, 0, 0, 255]);
    assert_eq!(rendered.get_pixel(2, 0).0, [40, 0, 0, 255]);
    assert_eq!(rendered.get_pixel(0, 1).0, [60, 0, 0, 255]);
    assert_eq!(doc, original_document);
    assert_eq!(assets["exact src with spaces"].as_raw(), &original_pixels);
    assert!(Arc::ptr_eq(
        &assets["exact src with spaces"],
        &original_asset
    ));
}

#[test]
fn layer_order_visibility_locking_opacity_and_background_are_retained() {
    let red = RgbaImage::from_pixel(1, 1, Rgba([220, 20, 40, 255]));
    let blue = RgbaImage::from_pixel(1, 1, Rgba([20, 40, 220, 255]));
    let hidden = {
        let mut layer = image("hidden", "missing-hidden", 1., 1.);
        layer.base.visible = false;
        Element::Image(layer)
    };
    let mut bottom = image("bottom", "red", 1., 1.);
    bottom.base.locked = true;
    let mut top = image("top", "blue", 1., 1.);
    top.base.opacity = 50.;
    let mut doc = document(
        1.,
        1.,
        vec![hidden, Element::Image(bottom), Element::Image(top)],
    );
    doc.background = Some("#1234".into());
    let assets = assets(&[("red", red), ("blue", blue)]);

    let rendered = render(&doc, &assets).unwrap();

    // 50% blue over opaque red. The shorthand alpha background is obscured,
    // while the locked bottom layer still participates normally.
    assert_pixel_near(rendered.get_pixel(0, 0).0, [120, 30, 130, 255], 1);

    doc.elements.swap(1, 2);
    let reversed = render(&doc, &assets).unwrap();
    assert_eq!(reversed.get_pixel(0, 0).0, [220, 20, 40, 255]);

    let mut background_only = document(1., 1., vec![]);
    background_only.background = Some("#1234".into());
    assert_eq!(
        render(&background_only, &BTreeMap::new())
            .unwrap()
            .get_pixel(0, 0)
            .0,
        [17, 34, 51, 68]
    );
}

#[test]
fn all_shipping_blend_modes_match_independent_channel_equations() {
    let foreground = [192, 96, 32, 255];
    let source = RgbaImage::from_pixel(1, 1, Rgba(foreground));
    let assets = assets(&[("blend", source)]);
    // Values are independently calculated from the W3C separable blend
    // equations using backdrop B=(64,160,224) and source S=(192,96,32).
    let cases = [
        ("source-over", [192, 96, 32, 255]),
        ("multiply", [48, 60, 28, 255]),
        ("screen", [208, 196, 228, 255]),
        ("overlay", [96, 137, 201, 255]),
        ("darken", [64, 96, 32, 255]),
        ("lighten", [192, 160, 224, 255]),
    ];

    for (mode, expected) in cases {
        let mut layer = image(mode, "blend", 1., 1.);
        layer.base.blend_mode = mode.into();
        let mut doc = document(1., 1., vec![Element::Image(layer)]);
        doc.background = Some("#40a0e0".into());
        let actual = render(&doc, &assets).unwrap().get_pixel(0, 0).0;
        assert_pixel_near(actual, expected, 2);
    }
}

#[test]
fn radians_are_converted_for_arbitrary_existing_renderer_rotation() {
    let source = RgbaImage::from_pixel(10, 4, Rgba([35, 145, 225, 255]));
    let mut layer = image("rotated", "rotation", 10., 4.);
    layer.base.x = 5.;
    layer.base.y = 8.;
    layer.base.rotation = Some(FRAC_PI_4);
    let rendered = render(
        &document(20., 20., vec![Element::Image(layer)]),
        &assets(&[("rotation", source)]),
    )
    .unwrap();

    // The 45° bounds reach y≈5; this pixel is well inside the rotated shape
    // but outside the unrotated y=8..12 rectangle.
    assert_eq!(rendered.get_pixel(8, 6).0, [35, 145, 225, 255]);
    assert_eq!(rendered.get_pixel(5, 5).0, [0, 0, 0, 0]);
}

#[test]
fn five_closed_shapes_use_reversed_drag_bounds_and_shipping_silhouettes() {
    let cases = [
        ("rectangle", (35, 30), (10, 10)),
        ("ellipse", (35, 30), (12, 12)),
        ("triangle", (35, 27), (15, 20)),
        ("diamond", (35, 30), (15, 15)),
        ("star", (35, 30), (25, 16)),
    ];
    for (kind, interior, exterior) in cases {
        let mut element = shape(kind, kind, (60., 50.), (10., 10.));
        element.style.stroke_enabled = Some(false);
        let rendered = render(
            &document(70., 60., vec![Element::Shape(element)]),
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(
            rendered.get_pixel(interior.0, interior.1).0,
            [230, 60, 40, 255],
            "{kind} interior"
        );
        assert_eq!(
            rendered.get_pixel(exterior.0, exterior.1).0,
            [0, 0, 0, 0],
            "{kind} exterior"
        );
    }
}

#[test]
fn shape_stroke_fill_rotation_clipping_opacity_and_blend_match_shipping() {
    let mut stroke_only = shape("stroke", "ellipse", (5., 5.), (35., 25.));
    stroke_only.style.fill = None;
    let stroke_render = render(
        &document(40., 30., vec![Element::Shape(stroke_only)]),
        &BTreeMap::new(),
    )
    .unwrap();
    assert_eq!(stroke_render.get_pixel(20, 15).0, [0, 0, 0, 0]);
    assert!(stroke_render.get_pixel(20, 4)[3] > 0);

    let mut fill_only = shape("fill", "diamond", (-12., -8.), (28., 32.));
    fill_only.style.stroke_enabled = Some(false);
    fill_only.base.opacity = 50.;
    fill_only.base.blend_mode = "multiply".into();
    fill_only.base.locked = true;
    let mut doc = document(30., 35., vec![Element::Shape(fill_only)]);
    doc.background = Some("#80c060".into());
    let fill_render = render(&doc, &BTreeMap::new()).unwrap();
    // 50% multiply of (230,60,40) over (128,192,96), independently rounded.
    assert_pixel_near(fill_render.get_pixel(8, 12).0, [122, 119, 56, 255], 2);
    assert_eq!(fill_render.get_pixel(29, 0).0, [128, 192, 96, 255]);

    let mut rotated = shape("rotated-star", "star", (10., 10.), (50., 50.));
    rotated.style.stroke_enabled = Some(false);
    rotated.base.rotation = Some(std::f64::consts::PI);
    let rotated = render(
        &document(60., 60., vec![Element::Shape(rotated)]),
        &BTreeMap::new(),
    )
    .unwrap();
    // The authored drag-box center is (30,30), so the top tip rotates to the
    // bottom edge even though the unrotated star's painted bounds are asymmetric.
    assert_eq!(rotated.get_pixel(30, 46).0, [230, 60, 40, 255]);
    assert_eq!(rotated.get_pixel(30, 11).0, [0, 0, 0, 0]);
}

#[test]
fn image_and_shape_layers_keep_document_order() {
    let bottom = RgbaImage::from_pixel(20, 20, Rgba([210, 30, 50, 255]));
    let top = RgbaImage::from_pixel(4, 4, Rgba([20, 210, 80, 255]));
    let mut middle = shape("middle", "rectangle", (3., 3.), (17., 17.));
    middle.style.stroke_enabled = Some(false);
    middle.style.fill = Some("#2846dc".into());
    let mut top_image = image("top", "top", 4., 4.);
    top_image.base.x = 8.;
    top_image.base.y = 8.;
    let rendered = render(
        &document(
            20.,
            20.,
            vec![
                Element::Image(image("bottom", "bottom", 20., 20.)),
                Element::Shape(middle),
                Element::Image(top_image),
            ],
        ),
        &assets(&[("bottom", bottom), ("top", top)]),
    )
    .unwrap();
    assert_eq!(rendered.get_pixel(1, 1).0, [210, 30, 50, 255]);
    assert_eq!(rendered.get_pixel(6, 6).0, [40, 70, 220, 255]);
    assert_eq!(rendered.get_pixel(9, 9).0, [20, 210, 80, 255]);
}

#[test]
fn line_controls_and_freehand_paths_use_distinct_shipping_curves() {
    let mut line = shape("quadratic-line", "line", (5., 50.), (65., 50.));
    line.controls = vec![Point { x: 35., y: 10. }];
    line.style.color = "#286ee6".into();
    line.style.fill = None;
    let rendered = render(
        &document(75., 65., vec![Element::Shape(line)]),
        &BTreeMap::new(),
    )
    .unwrap();
    // Quadratic apex is (35,30), not the off-path control or straight chord.
    assert!(rendered.get_pixel(35, 30)[3] > 220);
    assert_eq!(rendered.get_pixel(35, 10)[3], 0);
    assert_eq!(rendered.get_pixel(35, 50)[3], 0);

    let freehand = path("freehand", &[(5., 5.), (25., 35.), (45., 5.)]);
    let rendered = render(
        &document(55., 45., vec![Element::Path(freehand)]),
        &BTreeMap::new(),
    )
    .unwrap();
    // Shipping freehand first curves to midpoint (35,20), then lines to the end.
    assert!(rendered.get_pixel(23, 24)[3] > 180);
    assert_eq!(rendered.get_pixel(25, 35)[3], 0);
    assert!(rendered.get_pixel(43, 7)[3] > 180);

    let dot = path("dot", &[(12., 14.)]);
    let rendered = render(
        &document(25., 25., vec![Element::Path(dot)]),
        &BTreeMap::new(),
    )
    .unwrap();
    assert!(rendered.get_pixel(12, 14)[3] > 200);
}

#[test]
fn tapered_arrows_grow_from_a_thin_tail_and_follow_curved_controls() {
    let mut arrow = shape("arrow", "arrow", (10., 25.), (70., 25.));
    arrow.style.color = "#dc3c28".into();
    arrow.style.stroke_width = 8.;
    arrow.style.fill = None;
    let rendered = render(
        &document(80., 50., vec![Element::Shape(arrow.clone())]),
        &BTreeMap::new(),
    )
    .unwrap();
    assert_eq!(rendered.get_pixel(20, 25).0, [220, 60, 40, 255]);
    assert_eq!(rendered.get_pixel(20, 28)[3], 0, "tail stays narrow");
    assert!(rendered.get_pixel(52, 34)[3] > 200, "head shoulder widens");
    assert!(rendered.get_pixel(69, 25)[3] > 0, "tip is retained");

    arrow.controls = vec![Point { x: 40., y: 5. }];
    let curved = render(
        &document(80., 50., vec![Element::Shape(arrow)]),
        &BTreeMap::new(),
    )
    .unwrap();
    assert!(curved.get_pixel(40, 15)[3] > 150);
    assert_eq!(curved.get_pixel(40, 25)[3], 0);

    let tiny = shape("tiny", "arrow", (5., 5.), (5.5, 5.5));
    assert!(
        render(
            &document(10., 10., vec![Element::Shape(tiny)]),
            &BTreeMap::new()
        )
        .unwrap()
        .pixels()
        .all(|pixel| pixel[3] == 0)
    );
}

#[test]
fn open_strokes_preserve_rotation_opacity_blend_clipping_lock_and_stack_order() {
    let bottom = RgbaImage::from_pixel(40, 40, Rgba([120, 180, 220, 255]));
    let top = RgbaImage::from_pixel(4, 4, Rgba([245, 210, 20, 255]));
    let mut line = shape("line", "line", (-10., 20.), (35., 20.));
    line.style.color = "#c85028".into();
    line.style.fill = None;
    line.style.stroke_width = 6.;
    line.base.opacity = 50.;
    line.base.blend_mode = "multiply".into();
    line.base.rotation = Some(std::f64::consts::FRAC_PI_2);
    line.base.locked = true;
    let mut top_image = image("top", "top", 4., 4.);
    top_image.base.x = 11.;
    top_image.base.y = 18.;
    let rendered = render(
        &document(
            40.,
            40.,
            vec![
                Element::Image(image("bottom", "bottom", 40., 40.)),
                Element::Shape(line),
                Element::Image(top_image),
            ],
        ),
        &assets(&[("bottom", bottom), ("top", top)]),
    )
    .unwrap();
    // 50% multiply of (200,80,40) over (120,180,220).
    assert_pixel_near(rendered.get_pixel(12, 10).0, [107, 118, 127, 255], 2);
    assert_eq!(rendered.get_pixel(12, 19).0, [245, 210, 20, 255]);
    assert_eq!(rendered.get_pixel(30, 20).0, [120, 180, 220, 255]);
}

fn unsupported_element(kind: &str, visible: bool) -> Element {
    let base = json!({
        "kind": kind,
        "id": format!("unsupported-{kind}"),
        "x": 0,
        "y": 0,
        "locked": false,
        "visible": visible,
        "opacity": 100,
        "blendMode": "source-over"
    });
    let mut value = base.as_object().unwrap().clone();
    match kind {
        "text" => value.extend(
            json!({
                "text": "hello", "fontSize": 12, "width": 40,
                "fontFamily": "sans", "bold": false, "italic": false,
                "align": "left", "color": "#000", "background": null,
                "outlined": false, "roundedBackground": false
            })
            .as_object()
            .unwrap()
            .clone(),
        ),
        "shape" => value.extend(
            json!({
                "shape": "line", "endX": 10, "endY": 10, "controls": [],
                "style": { "color": "#000", "fill": null, "strokeWidth": 1 }
            })
            .as_object()
            .unwrap()
            .clone(),
        ),
        "path" => value.extend(
            json!({
                "points": [{ "x": 0, "y": 0 }],
                "style": { "color": "#000", "fill": null, "strokeWidth": 1 }
            })
            .as_object()
            .unwrap()
            .clone(),
        ),
        _ => unreachable!(),
    }
    serde_json::from_value(value.into()).unwrap()
}

#[test]
fn unsupported_visible_content_and_values_fail_instead_of_disappearing() {
    let error = render(
        &document(1., 1., vec![unsupported_element("text", true)]),
        &BTreeMap::new(),
    )
    .unwrap_err();
    assert!(error.contains("visible text layer"), "{error}");

    let hidden = document(
        1.,
        1.,
        vec![
            unsupported_element("text", false),
            unsupported_element("shape", false),
            unsupported_element("path", false),
        ],
    );
    assert_eq!(
        render(&hidden, &BTreeMap::new()).unwrap().get_pixel(0, 0).0,
        [0, 0, 0, 0]
    );

    let mut missing = image("missing", "exact-missing-src", 1., 1.);
    assert!(
        render(
            &document(1., 1., vec![Element::Image(missing.clone())]),
            &BTreeMap::new()
        )
        .unwrap_err()
        .contains("exact-missing-src")
    );
    missing.base.blend_mode = "color-burn".into();
    assert!(
        render(
            &document(1., 1., vec![Element::Image(missing)]),
            &BTreeMap::new()
        )
        .unwrap_err()
        .contains("unsupported blend mode")
    );

    let mut invalid_background = document(1., 1., vec![]);
    invalid_background.background = Some("rgb(1, 2, 3)".into());
    assert!(
        render(&invalid_background, &BTreeMap::new())
            .unwrap_err()
            .contains("unsupported editor background")
    );
}

#[test]
fn unsupported_shape_kinds_and_invalid_style_values_are_explicit() {
    let error = render(
        &document(
            20.,
            20.,
            vec![Element::Shape(shape(
                "hexagon",
                "hexagon",
                (1., 1.),
                (10., 10.),
            ))],
        ),
        &BTreeMap::new(),
    )
    .unwrap_err();
    assert!(error.contains("unsupported shape kind"), "{error}");

    for mutate in [
        |shape: &mut ShapeElement| shape.end_x = f64::NAN,
        |shape: &mut ShapeElement| shape.end_y = shape.base.y,
        |shape: &mut ShapeElement| shape.style.stroke_width = f64::INFINITY,
        |shape: &mut ShapeElement| shape.style.color = "rgb(1, 2, 3)".into(),
        |shape: &mut ShapeElement| shape.style.fill = Some("not-a-color".into()),
        |shape: &mut ShapeElement| shape.base.opacity = f64::NAN,
    ] {
        let mut invalid = shape("invalid", "triangle", (1., 1.), (10., 10.));
        mutate(&mut invalid);
        assert!(
            render(
                &document(20., 20., vec![Element::Shape(invalid)]),
                &BTreeMap::new()
            )
            .is_err()
        );
    }

    let mut hidden = shape("hidden-shadow", "rectangle", (1., 1.), (10., 10.));
    hidden.base.visible = false;
    hidden.style.drop_shadow = Some(true);
    assert!(
        render(
            &document(20., 20., vec![Element::Shape(hidden)]),
            &BTreeMap::new()
        )
        .is_ok()
    );

    for mutate in [
        |path: &mut PathElement| path.points[0].x = f64::NAN,
        |path: &mut PathElement| path.style.stroke_width = 0.,
        |path: &mut PathElement| path.style.color = "hsl(0 0% 0%)".into(),
        |path: &mut PathElement| path.base.opacity = f64::INFINITY,
        |path: &mut PathElement| path.base.blend_mode = "difference".into(),
        |path: &mut PathElement| path.base.rotation = Some(f64::MAX),
    ] {
        let mut invalid = path("invalid-path", &[(1., 1.), (10., 10.)]);
        mutate(&mut invalid);
        assert!(
            render(
                &document(20., 20., vec![Element::Path(invalid)]),
                &BTreeMap::new()
            )
            .is_err()
        );
    }
}

#[test]
fn enabled_shape_and_path_shadows_use_custom_and_shipping_default_metrics() {
    let mut shadow = shape("shadow", "rectangle", (5., 7.), (15., 17.));
    shadow.base.opacity = 50.;
    shadow.style.stroke_enabled = Some(false);
    shadow.style.drop_shadow = Some(true);
    shadow.style.drop_shadow_style = Some(DropShadowStyle {
        color: "#20c060".into(),
        opacity: 80.,
        blur: 0.,
        offset_x: 12.,
        offset_y: -3.,
        extra: Map::new(),
    });
    let rendered = render(
        &document(36., 24., vec![Element::Shape(shadow)]),
        &BTreeMap::new(),
    )
    .unwrap();
    let shadow_only = rendered.get_pixel(21, 9).0;
    for (actual, expected) in shadow_only[..3].iter().zip([32_u8, 192, 96]) {
        assert!(actual.abs_diff(expected) <= 1, "{shadow_only:?}");
    }
    assert!((100..=103).contains(&shadow_only[3]), "{shadow_only:?}");
    assert_eq!(&rendered.get_pixel(10, 11).0[..3], &[230, 60, 40]);

    let mut path_shadow = path("path-shadow", &[(4., 12.), (13., 12.)]);
    path_shadow.style.color = "#e04090".into();
    path_shadow.style.stroke_width = 8.;
    path_shadow.style.drop_shadow = Some(true);
    let rendered = render(
        &document(28., 28., vec![Element::Path(path_shadow)]),
        &BTreeMap::new(),
    )
    .unwrap();
    assert_eq!(&rendered.get_pixel(8, 12).0[..3], &[224, 64, 144]);
    assert!(rendered.get_pixel(8, 21).0[3] > 0);
}

#[test]
fn invalid_and_oversized_inputs_are_rejected_before_canvas_allocation() {
    for (width, height) in [
        (f64::NAN, 1.),
        (1., f64::INFINITY),
        (0., 1.),
        (16_385., 1.),
        (10_001., 10_001.),
    ] {
        assert!(render(&document(width, height, vec![]), &BTreeMap::new()).is_err());
    }

    let valid_asset = RgbaImage::from_pixel(1, 1, Rgba([1, 2, 3, 4]));
    let invalid = image("invalid", "asset", 1., 1.);
    for mutate in [
        |image: &mut ImageElement| image.base.x = f64::NAN,
        |image: &mut ImageElement| image.base.y = f64::MAX,
        |image: &mut ImageElement| image.width = -1.,
        |image: &mut ImageElement| image.height = f64::INFINITY,
        |image: &mut ImageElement| image.natural_width = 0.,
        |image: &mut ImageElement| image.natural_height = f64::NAN,
        |image: &mut ImageElement| image.base.rotation = Some(f64::MAX),
        |image: &mut ImageElement| image.base.opacity = f64::NAN,
    ] {
        let mut case = invalid.clone();
        mutate(&mut case);
        assert!(
            render(
                &document(1., 1., vec![Element::Image(case)]),
                &assets(&[("asset", valid_asset.clone())])
            )
            .is_err()
        );
    }

    assert!(
        render(
            &document(1., 1., vec![Element::Image(invalid.clone())]),
            &assets(&[("asset", RgbaImage::new(0, 1))])
        )
        .unwrap_err()
        .contains("asset is empty")
    );
    assert!(
        render(
            &document(1., 1., vec![Element::Image(invalid)]),
            &assets(&[("asset", RgbaImage::new(16_385, 1))])
        )
        .unwrap_err()
        .contains("asset exceeds renderer limits")
    );
}
