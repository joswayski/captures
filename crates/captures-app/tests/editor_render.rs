use std::{collections::BTreeMap, f64::consts::FRAC_PI_4, sync::Arc};

use captures_app::{
    editor::{
        Document, Element, ElementBase, ImageElement, ImageOrientation, OptionalNullable, Rect,
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
                "shape": "rectangle", "endX": 10, "endY": 10, "controls": [],
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
    for kind in ["text", "shape", "path"] {
        let error = render(
            &document(1., 1., vec![unsupported_element(kind, true)]),
            &BTreeMap::new(),
        )
        .unwrap_err();
        assert!(error.contains(&format!("visible {kind} layer")), "{error}");
    }

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
