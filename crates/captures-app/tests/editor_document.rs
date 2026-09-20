use captures_app::editor::{
    Document, DocumentHistory, Element, ImageTransform, LayerEdit, Point, Rect, bounded_crop_rect,
};
use captures_history::editor_draft::{self, SaveRequest};
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fixture {
    initialization: InitializationCase,
    document: Value,
    crops: Vec<CropCase>,
    translations: Vec<TranslationCase>,
    crop_rects: Vec<DocumentCropCase>,
    canvas_sizes: Vec<CanvasSizeCase>,
    orientations: Vec<OrientationCase>,
    layers: Value,
    history: HistoryCase,
}

#[derive(Deserialize)]
struct InitializationCase {
    input: InitializationInput,
    expected: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InitializationInput {
    src: String,
    width: f64,
    height: f64,
    source_artifact_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CropCase {
    start: Point,
    end: Point,
    bounds: Bounds,
    aspect_ratio: Option<f64>,
    expected: Rect,
}

#[derive(Deserialize)]
struct Bounds {
    width: f64,
    height: f64,
}

impl Bounds {
    fn as_rect(&self) -> Rect {
        Rect {
            x: 0.,
            y: 0.,
            width: self.width,
            height: self.height,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TranslationCase {
    delta_x: f64,
    delta_y: f64,
    expected: Value,
}

#[derive(Deserialize)]
struct DocumentCropCase {
    rect: Rect,
    expected: Value,
}

#[derive(Deserialize)]
struct CanvasSizeCase {
    width: f64,
    height: f64,
    expected: Value,
}

#[derive(Deserialize)]
struct OrientationCase {
    input: Value,
    action: ImageTransform,
    expected: Value,
}

#[derive(Deserialize)]
struct HistoryCase {
    initial: Document,
    operations: Vec<HistoryOperation>,
    expected: Vec<HistoryExpected>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum HistoryOperation {
    Commit {
        width: f64,
        marker: Option<u64>,
        #[serde(default)]
        branch: bool,
    },
    Undo,
    Redo,
}

#[derive(Deserialize)]
struct HistoryExpected {
    changed: bool,
    current: HistoryCurrent,
    undo: usize,
    redo: usize,
}

#[derive(Deserialize)]
struct HistoryCurrent {
    width: f64,
    marker: Option<u64>,
    branch: bool,
}

fn fixture() -> Fixture {
    serde_json::from_str(include_str!("editor-document-golden.json")).unwrap()
}

fn assert_json_equivalent(actual: Value, expected: Value) {
    match (actual, expected) {
        (Value::Number(actual), Value::Number(expected)) => assert_eq!(
            actual.as_f64(),
            expected.as_f64(),
            "JSON numbers differ: {actual} != {expected}"
        ),
        (Value::Array(actual), Value::Array(expected)) => {
            assert_eq!(actual.len(), expected.len());
            for (actual, expected) in actual.into_iter().zip(expected) {
                assert_json_equivalent(actual, expected);
            }
        }
        (Value::Object(actual), Value::Object(expected)) => {
            assert_eq!(actual.len(), expected.len());
            for (key, expected) in expected {
                let actual = actual
                    .get(&key)
                    .unwrap_or_else(|| panic!("missing JSON field {key}"));
                assert_json_equivalent(actual.clone(), expected);
            }
        }
        (actual, expected) => assert_eq!(actual, expected),
    }
}

#[test]
fn initialization_crop_translation_and_canvas_size_match_typescript() {
    let fixture = fixture();
    let input = fixture.initialization.input;
    let initialized = Document::new_capture(
        input.src,
        input.width,
        input.height,
        input.source_artifact_id,
    );
    assert_json_equivalent(
        serde_json::to_value(initialized).unwrap(),
        fixture.initialization.expected,
    );

    for case in fixture.crops {
        assert_eq!(
            bounded_crop_rect(
                case.start,
                case.end,
                case.bounds.as_rect(),
                case.aspect_ratio
            ),
            case.expected
        );
    }

    for case in fixture.translations {
        let mut document: Document = serde_json::from_value(fixture.document.clone()).unwrap();
        document.translate(case.delta_x, case.delta_y);
        assert_json_equivalent(serde_json::to_value(document).unwrap(), case.expected);
    }

    for case in fixture.crop_rects {
        let mut document: Document = serde_json::from_value(fixture.document.clone()).unwrap();
        document.crop(case.rect);
        assert_json_equivalent(serde_json::to_value(document).unwrap(), case.expected);
    }

    for case in fixture.canvas_sizes {
        let mut document: Document = serde_json::from_value(fixture.document.clone()).unwrap();
        document.resize_canvas(case.width, case.height);
        assert_json_equivalent(serde_json::to_value(document).unwrap(), case.expected);
    }
}

#[test]
fn all_d4_orientations_left_compose_and_preserve_center_like_typescript() {
    for case in fixture().orientations {
        let element: Element = serde_json::from_value(case.input).unwrap();
        let Element::Image(mut image) = element else {
            panic!("orientation fixture must contain an image")
        };
        image.transform(case.action);
        assert_json_equivalent(
            serde_json::to_value(Element::Image(image)).unwrap(),
            case.expected,
        );
    }
}

#[test]
fn unknown_and_legacy_optional_data_round_trip_without_loss() {
    let value = fixture().document;
    let document: Document = serde_json::from_value(value.clone()).unwrap();
    assert_json_equivalent(serde_json::to_value(&document).unwrap(), value);

    let Element::Text(text) = &document.elements[1] else {
        panic!("second fixture layer must be text")
    };
    assert_eq!(text.base.rotation(), 0.375);
    assert!(!text.uses_auto_width());
    assert!(!text.has_drop_shadow());

    let Element::Shape(shape) = &document.elements[2] else {
        panic!("third fixture layer must be a shape")
    };
    assert!(shape.style.has_stroke());
    assert!(!shape.style.has_drop_shadow());
}

#[test]
fn layer_order_and_duplication_match_shipping_including_locked_boundaries_and_unknowns() {
    let fixture = fixture();
    let mut initial: Document = serde_json::from_value(fixture.document).unwrap();
    initial.elements = serde_json::from_value(fixture.layers["elements"].clone()).unwrap();
    for case in fixture.layers["reorders"].as_array().unwrap() {
        let mut document = initial.clone();
        document
            .edit_layer(
                case["moved"].as_str().unwrap(),
                LayerEdit::Reorder {
                    target_id: case["target"].as_str().unwrap().into(),
                    placement: serde_json::from_value(case["placement"].clone()).unwrap(),
                },
            )
            .unwrap();
        let actual: Vec<_> = document
            .elements
            .iter()
            .map(|element| element.base().id.as_str())
            .collect();
        assert_eq!(
            serde_json::to_value(actual).unwrap(),
            case["expected"],
            "{case}"
        );
        for before in &initial.elements {
            assert_eq!(
                document
                    .elements
                    .iter()
                    .find(|element| element.base().id == before.base().id),
                Some(before)
            );
        }
    }
    for case in fixture.layers["duplicates"].as_array().unwrap() {
        let mut document = initial.clone();
        let input: Element = serde_json::from_value(case["input"].clone()).unwrap();
        document.elements = vec![input.clone()];
        document
            .edit_layer(
                &input.base().id,
                LayerEdit::Duplicate {
                    new_id: case["expected"]["id"].as_str().unwrap().into(),
                },
            )
            .unwrap();
        assert_eq!(document.elements[0], input);
        assert_json_equivalent(
            serde_json::to_value(&document.elements[1]).unwrap(),
            case["expected"].clone(),
        );
    }
}

#[test]
fn layer_flags_hidden_moves_and_rejections_preserve_other_document_data() {
    let mut document: Document = serde_json::from_value(fixture().document).unwrap();
    let before = document.clone();
    for edit in [
        LayerEdit::Delete,
        LayerEdit::Translate {
            delta_x: 13.5,
            delta_y: -8.25,
        },
    ] {
        document.edit_layer("locked-text", edit).unwrap();
        assert_eq!(document, before);
    }
    document
        .edit_layer(
            "hidden-image",
            LayerEdit::Translate {
                delta_x: 13.5,
                delta_y: -8.25,
            },
        )
        .unwrap();
    let image = document.elements[0].base();
    assert_eq!((image.x, image.y, image.visible), (-13.75, 6.5, false));
    document
        .edit_layer("locked-text", LayerEdit::Opacity { opacity: 42.5 })
        .unwrap();
    assert_eq!(document.elements[1].base().opacity, 42.5);
    document
        .edit_layer("locked-text", LayerEdit::Visibility { visible: false })
        .unwrap();
    assert!(!document.elements[1].base().visible);
    document
        .edit_layer(
            "hidden-image",
            LayerEdit::Rename {
                name: "  Better name  ".into(),
            },
        )
        .unwrap();
    let Element::Image(image) = &document.elements[0] else {
        panic!()
    };
    assert_eq!(image.name, "Better name");
    let valid = document.clone();
    for edit in [
        LayerEdit::Opacity { opacity: -0.01 },
        LayerEdit::Opacity { opacity: 100.01 },
        LayerEdit::Opacity { opacity: f64::NAN },
        LayerEdit::Translate {
            delta_x: f64::INFINITY,
            delta_y: 0.,
        },
        LayerEdit::Duplicate {
            new_id: "shape".into(),
        },
    ] {
        assert!(document.edit_layer("hidden-image", edit).is_err());
        assert_eq!(document, valid);
    }
    document
        .edit_layer("locked-text", LayerEdit::Lock { locked: false })
        .unwrap();
    document
        .edit_layer("locked-text", LayerEdit::Delete)
        .unwrap();
    assert_eq!(document.elements.len(), 3);
    assert_eq!(document.elements[1], before.elements[2]);
    assert_eq!(document.extra, before.extra);
}

#[test]
fn overflowing_hidden_layer_movement_cannot_make_an_unreadable_draft() {
    let initial: Document = serde_json::from_value(fixture().document).unwrap();
    for original in initial.elements {
        let mut document = Document {
            width: 20.,
            height: 10.,
            background: None,
            elements: vec![original],
            extra: Default::default(),
        };
        let id = document.elements[0].base().id.clone();
        document
            .edit_layer(&id, LayerEdit::Lock { locked: false })
            .unwrap();
        document
            .edit_layer(&id, LayerEdit::Visibility { visible: false })
            .unwrap();
        document
            .edit_layer(
                &id,
                LayerEdit::Translate {
                    delta_x: 1e308,
                    delta_y: -1e308,
                },
            )
            .unwrap();
        let before = document.clone();
        assert!(
            document
                .edit_layer(
                    &id,
                    LayerEdit::Translate {
                        delta_x: 1e308,
                        delta_y: -1e308
                    }
                )
                .is_err()
        );
        assert_eq!(document, before);
        let encoded = serde_json::to_value(&document).unwrap();
        assert_eq!(
            serde_json::from_value::<Document>(encoded).unwrap(),
            document
        );
    }
}

#[test]
fn typed_document_remains_opaque_v1_draft_manifest_compatible() {
    let root = tempfile::tempdir().unwrap();
    let mut value = fixture().document;
    value["elements"][0]["src"] = Value::String("data:image/png;base64,unchanged".into());
    let document: Document = serde_json::from_value(value).unwrap();
    let stored = serde_json::to_value(&document).unwrap();
    editor_draft::save(
        root.path(),
        SaveRequest {
            artifact_id: "capture-1".into(),
            document: stored.clone(),
            assets: vec![],
            updated_at_ms: 91,
        },
    )
    .unwrap();
    let loaded = editor_draft::load(root.path(), "capture-1", |artifact, asset| {
        format!("test-host://{artifact}/{asset}")
    })
    .unwrap()
    .unwrap();
    assert_json_equivalent(loaded.document, stored);
    assert_eq!(loaded.updated_at_ms, 91);
}

#[test]
fn snapshot_history_matches_no_op_cap_undo_redo_and_branching_oracle() {
    let fixture = fixture().history;
    let initial = fixture.initial;
    let mut history = DocumentHistory::new(initial.clone());
    assert_eq!(fixture.operations.len(), fixture.expected.len());

    for (operation, expected) in fixture.operations.into_iter().zip(fixture.expected) {
        let changed = match operation {
            HistoryOperation::Commit {
                width,
                marker,
                branch,
            } => {
                let mut document = initial.clone();
                document.width = width;
                if let Some(marker) = marker {
                    document
                        .extra
                        .insert("marker".into(), serde_json::json!({ "i": marker }));
                }
                if branch {
                    document.extra.insert("branch".into(), Value::Bool(true));
                }
                history.commit(document)
            }
            HistoryOperation::Undo => history.undo(),
            HistoryOperation::Redo => history.redo(),
        };
        assert_eq!(changed, expected.changed);
        assert_eq!(history.current().width, expected.current.width);
        assert_eq!(
            history
                .current()
                .extra
                .get("marker")
                .and_then(|value| value.get("i"))
                .and_then(Value::as_u64),
            expected.current.marker
        );
        assert_eq!(
            history
                .current()
                .extra
                .get("branch")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            expected.current.branch
        );
        assert_eq!(history.undo_len(), expected.undo);
        assert_eq!(history.redo_len(), expected.redo);
    }
}
