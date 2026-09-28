use std::{collections::BTreeMap, sync::Arc};

use captures_app::{
    editor::{Element, Rect, TextElement},
    editor_text::{
        FONT_FAMILY_LABELS, TEXT_STYLE_PRESETS, fit_auto_width, font_family_options,
        inline_editor_layout, layout, resize, selection_bounds, text_style_preset_id,
    },
};
use captures_image::text::{TextRenderer, TextStyle};
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
struct Case {
    element: TextElement,
    measurements: BTreeMap<String, f64>,
    expected: Value,
    fitted: TextElement,
    editing: TextElement,
    selection: Rect,
    resizes: Vec<ResizeCase>,
}

#[derive(Deserialize)]
struct ResizeCase {
    next: Rect,
    expected: Value,
}

fn cases() -> Vec<Case> {
    serde_json::from_str(include_str!("editor-text-golden.json")).unwrap()
}

#[test]
fn named_style_catalog_matches_shipping_preset_flags_fonts_and_plate_defaults() {
    let expected: Value =
        serde_json::from_str(include_str!("editor-text-presets-golden.json")).unwrap();
    let mut actual = serde_json::to_value(TEXT_STYLE_PRESETS).unwrap();
    for preset in actual.as_array_mut().unwrap() {
        preset.as_object_mut().unwrap().remove("label");
    }
    assert_eq!(actual, expected);
}

#[test]
fn named_style_labels_match_shipping_menu_copy() {
    // `TEXT_STYLE_ITEMS` in ScreenshotEditor.tsx, title-cased as shipped.
    let labels: Vec<_> = TEXT_STYLE_PRESETS
        .iter()
        .map(|preset| preset.label)
        .collect();
    assert_eq!(
        labels,
        [
            "Standard",
            "Rounded",
            "Outlined",
            "Mono",
            "Box",
            "Mono Box",
            "Rounded Box"
        ]
    );
}

#[test]
fn text_style_picker_value_matches_shipping_text_style_preset() {
    // Each preset's own treatment reads back as that preset.
    for preset in TEXT_STYLE_PRESETS {
        assert_eq!(
            text_style_preset_id(
                preset.font_family,
                preset.background.is_some(),
                preset.outlined,
                preset.rounded_background,
            ),
            preset.id
        );
    }
    // Shipping precedence for mixed treatments.
    assert_eq!(
        text_style_preset_id("serif", false, false, false),
        "standard"
    );
    assert_eq!(text_style_preset_id("mono", false, true, false), "outlined");
    assert_eq!(text_style_preset_id("rounded", true, true, false), "box");
    assert_eq!(text_style_preset_id("serif", true, false, true), "box");
}

#[test]
fn font_menu_uses_shipping_labels_and_order_not_asset_names() {
    let bundled = captures_app::editor_fonts::bundled().families;
    let options = font_family_options(&bundled);
    let expected: Vec<(String, String)> = FONT_FAMILY_LABELS
        .iter()
        .map(|(key, label)| ((*key).into(), (*label).into()))
        .collect();
    assert_eq!(options, expected);
    assert_eq!(
        options
            .iter()
            .map(|(_, label)| label.as_str())
            .collect::<Vec<_>>(),
        ["Sans serif", "Serif", "Monospace", "Rounded"]
    );
    // A legacy draft without Rounded, plus an unknown pinned key.
    let mut legacy = bundled.clone();
    legacy.remove("rounded");
    legacy.insert("display".into(), "Display Face".into());
    assert_eq!(
        font_family_options(&legacy),
        [
            ("sans".to_owned(), "Sans serif".to_owned()),
            ("serif".into(), "Serif".into()),
            ("mono".into(), "Monospace".into()),
            ("display".into(), "Display Face".into()),
        ]
    );
    assert!(font_family_options(&BTreeMap::new()).is_empty());
}

fn compare(actual: &Value, expected: &Value) {
    match (actual, expected) {
        (Value::Number(a), Value::Number(b)) => {
            assert!(
                (a.as_f64().unwrap() - b.as_f64().unwrap()).abs() < 1e-9,
                "{a} != {b}"
            );
        }
        (Value::Array(a), Value::Array(b)) => {
            assert_eq!(a.len(), b.len());
            for (a, b) in a.iter().zip(b) {
                compare(a, b);
            }
        }
        (Value::Object(a), Value::Object(b)) => {
            assert_eq!(a.len(), b.len());
            for (key, value) in a {
                compare(value, &b[key]);
            }
        }
        _ => assert_eq!(actual, expected),
    }
}

#[test]
fn matches_shipping_wrap_alignment_plates_and_auto_width_without_losing_metadata() {
    for case in cases() {
        let measure = |text: &str| {
            case.measurements.get(text).copied().ok_or_else(|| {
                format!(
                    "unexpected measurement {text:?} in {}",
                    case.element.base.id
                )
            })
        };
        compare(
            &serde_json::to_value(layout(&case.element, measure).unwrap()).unwrap(),
            &case.expected,
        );
        assert_eq!(
            fit_auto_width(&case.element, false, measure).unwrap(),
            case.fitted
        );
        assert_eq!(
            fit_auto_width(&case.element, true, measure).unwrap(),
            case.editing
        );
    }
}

#[test]
fn selection_and_resize_match_shipping_utf16_wrap_plate_shadow_and_type_scaling() {
    for case in cases() {
        compare(
            &serde_json::to_value(selection_bounds(&case.element).unwrap()).unwrap(),
            &serde_json::to_value(case.selection).unwrap(),
        );
        assert_eq!(
            Element::Text(case.element.clone())
                .selection_bounds()
                .unwrap(),
            selection_bounds(&case.element).unwrap()
        );
        for resized in case.resizes {
            compare(
                &serde_json::to_value(resize(&case.element, case.selection, resized.next).unwrap())
                    .unwrap(),
                &resized.expected,
            );
        }
    }
}

#[test]
fn real_shaping_controls_wrap_ligatures_and_auto_width() {
    let mut renderer = TextRenderer::new([Arc::from(
        include_bytes!("../../captures-image/tests/shaping-regular.ttf").as_slice(),
    )])
    .unwrap();
    let style = TextStyle {
        family: "Captures Shaping Test",
        size: 100.,
        bold: false,
        italic: false,
        color: [0, 0, 0, 255],
    };
    let mut element = cases().remove(0).element;
    element.font_size = 100.;
    element.width = 145.;
    element.align = "right".into();
    element.text = "fi L\nA\u{301}".into();
    let measured = layout(&element, |s| {
        renderer.measure_line(s, &style).map(f64::from)
    })
    .unwrap();
    // GSUB fi = 45, space = 30, L = 70; the combining acute has zero advance.
    assert_eq!(
        measured
            .rows
            .iter()
            .map(|row| row.text.as_str())
            .collect::<Vec<_>>(),
        ["fi L", "A\u{301}"]
    );
    assert_eq!(measured.rows[0].advance, 145.);
    assert_eq!(measured.rows[1].advance, 70.);
    assert_eq!(measured.rows[1].x, element.base.x + 75.);
    assert_eq!(measured.rows[1].y, element.base.y + 125.);
    element.width = 144.;
    assert_eq!(
        layout(&element, |s| renderer
            .measure_line(s, &style)
            .map(f64::from))
        .unwrap()
        .rows
        .iter()
        .map(|row| row.text.as_str())
        .collect::<Vec<_>>(),
        ["fi", "L", "A\u{301}"]
    );
    element.auto_width = Some(true);
    let fitted = fit_auto_width(&element, false, |s| {
        renderer.measure_line(s, &style).map(f64::from)
    })
    .unwrap();
    assert_eq!(fitted.width, 180.); // longest line 145 + 35 caret padding
    assert_eq!(fitted.base.x, element.base.x - 36.);

    // A token can exceed the raster extent, yet every wrapped row fits it.
    element.text = "L".repeat(240);
    element.width = 140.;
    let wrapped = layout(&element, |s| {
        renderer.measure_line(s, &style).map(f64::from)
    })
    .unwrap();
    assert_eq!(wrapped.rows.len(), 120);
    assert!(
        wrapped
            .rows
            .iter()
            .all(|row| row.text == "LL" && row.advance == 140.)
    );
}

#[test]
fn invalid_input_and_measurement_fail_without_mutating_the_element() {
    let original = cases().remove(0).element;
    for invalid in [f64::NAN, f64::INFINITY, -1.] {
        assert!(layout(&original, |_| Ok(invalid)).is_err());
    }
    assert_eq!(
        layout(&original, |_| Err("missing font".into())).unwrap_err(),
        "missing font"
    );
    for field in ["text", "size", "width", "align"] {
        let mut element = original.clone();
        match field {
            "text" => element.text = "é".repeat(2049),
            "size" => element.font_size = 513.,
            "width" => element.width = f64::INFINITY,
            _ => element.align = "justify".into(),
        }
        assert!(layout(&element, |_| panic!("validate before measurement")).is_err());
    }
    assert_eq!(
        fit_auto_width(&original, false, |_| panic!(
            "fixed width needs no measurement"
        ))
        .unwrap(),
        original
    );
    let mut boundary = original.clone();
    boundary.text = "é".repeat(2048);
    boundary.font_size = 512.;
    assert_eq!(
        layout(&boundary, |_| Ok(10.)).unwrap().rows[0].text,
        boundary.text
    );
    boundary.base.x = f64::MAX;
    boundary.width = f64::MAX;
    boundary.align = "right".into();
    assert!(layout(&boundary, |_| Ok(10.)).is_err());

    let mut blank = original;
    blank.auto_width = Some(true);
    blank.text = "\u{feff} \n".into();
    assert_eq!(
        fit_auto_width(&blank, true, |_| panic!(
            "blank composing box needs no measurement"
        ))
        .unwrap()
        .width,
        188.
    );
}

#[test]
fn inline_editor_layout_matches_shipping_frame_padding_radius_and_rotation() {
    let mut element: TextElement = serde_json::from_value(serde_json::json!({
        "id": "label", "kind": "text", "x": 40.0, "y": 30.0, "visible": true, "locked": false,
        "opacity": 100, "blendMode": "source-over", "text": "Hello\nworld", "fontSize": 20.0,
        "width": 120.0, "fontFamily": "sans", "bold": false, "italic": false, "align": "left",
        "color": "#ff3b5c", "background": null, "outlined": false, "roundedBackground": false
    }))
    .unwrap();
    let plain = inline_editor_layout(&element).unwrap();
    // No plate: the glyph box, two 1.25-em rows, and only the optical nudge.
    assert_eq!(
        plain.frame,
        Rect {
            x: 40.,
            y: 30.,
            width: 120.,
            height: 50.
        }
    );
    assert!((plain.padding[0] - 1.4).abs() < 1e-9);
    assert_eq!(plain.padding[1..], [0., 0., 0.]);
    assert_eq!(
        (plain.plate_radius, plain.outline_width, plain.rotation),
        (0., 0., 0.)
    );
    assert!(!plain.auto_width);

    element.background = Some("#111318".into());
    element.rounded_background = true;
    element.outlined = true;
    element.auto_width = Some(true);
    element.base.rotation = Some(0.5);
    let plate = inline_editor_layout(&element).unwrap();
    // textBackgroundPad: 0.36 em sideways, 0.22 em vertically, around the box.
    assert!((plate.frame.x - 32.8).abs() < 1e-9 && (plate.frame.y - 25.6).abs() < 1e-9);
    assert!((plate.frame.width - 134.4).abs() < 1e-9 && (plate.frame.height - 58.8).abs() < 1e-9);
    let [top, right, bottom, left] = plate.padding;
    assert!((top - 5.8).abs() < 1e-9 && (bottom - 3.0).abs() < 1e-9);
    assert!((right - 7.2).abs() < 1e-9 && (left - 7.2).abs() < 1e-9);
    // min(shortest × 0.28, size × 0.34, shortest / 2).
    assert!((plate.plate_radius - 6.8).abs() < 1e-9);
    assert_eq!(
        (plate.outline_width, plate.rotation, plate.auto_width),
        (1.6, 0.5, true)
    );
}
