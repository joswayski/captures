use captures_app::editor::{
    ClosedShapeCreate, CropDrag, Document, DocumentHistory, Element, FreehandPathCreate,
    ImageTransform, LayerEdit, MoveDrag, OpenShapeCreate, Point, Rect, ResizeDrag, ResizeHandle,
    bounded_crop_rect, preview_rotation, rotation_angle, rotation_handle, smooth_path_centerline,
};
use captures_history::editor_draft::{self, SaveRequest};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fixture {
    initialization: InitializationCase,
    document: Value,
    crops: Vec<CropCase>,
    crop_drags: Vec<CropDragCase>,
    translations: Vec<TranslationCase>,
    crop_rects: Vec<DocumentCropCase>,
    canvas_sizes: Vec<CanvasSizeCase>,
    trims: Vec<TrimCase>,
    shape_creations: Vec<ShapeCreationCase>,
    open_shape_creations: Vec<OpenShapeCreationCase>,
    freehand_creations: Vec<FreehandCreationCase>,
    hit_tests: Vec<HitTestCase>,
    moves: Vec<MoveDragCase>,
    resize: ResizeFixture,
    rotations: Value,
    orientations: Vec<OrientationCase>,
    layers: Value,
    history: HistoryCase,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResizeFixture {
    hit_tests: Vec<ResizeHitTestCase>,
    drags: Vec<ResizeDragCase>,
}

#[derive(Deserialize)]
struct ResizeHitTestCase {
    name: String,
    element: Element,
    radius: f64,
    queries: Vec<ResizeHitTestQuery>,
}

#[derive(Deserialize)]
struct ResizeHitTestQuery {
    point: Point,
    expected: Option<ResizeHandle>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResizeDragCase {
    name: String,
    input: Document,
    id: String,
    handle: ResizeHandle,
    display_scale: f64,
    current: Point,
    lock_aspect: bool,
    expected: Value,
    committed: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MoveDragCase {
    name: String,
    input: Document,
    id: String,
    display_scale: f64,
    delta: Point,
    expected: Value,
    committed: Value,
}

#[derive(Deserialize)]
struct HitTestCase {
    name: String,
    input: Document,
    bounds: Option<Rect>,
    outline: Option<[Point; 4]>,
    queries: Vec<HitTestQuery>,
}

#[derive(Deserialize)]
struct HitTestQuery {
    point: Point,
    tolerance: f64,
    expected: Option<String>,
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

#[derive(Deserialize)]
struct CropDragCase {
    origin: Point,
    bounds: Bounds,
    initial: CropDragStep,
    updates: Vec<CropDragStep>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CropDragStep {
    current: Point,
    preset_aspect: Option<f64>,
    shift_key: bool,
    expected: Rect,
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
struct TrimCase {
    name: String,
    input: Document,
    expected: Value,
}

#[derive(Deserialize)]
struct ShapeCreationCase {
    name: String,
    input: Document,
    request: ClosedShapeCreate,
    expected: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpenShapeCreationCase {
    name: String,
    input: Document,
    request: OpenShapeCreate,
    path_length: f64,
    expected: Option<Value>,
}

#[derive(Deserialize)]
struct FreehandCreationCase {
    name: String,
    input: Document,
    request: FreehandPathCreate,
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

#[test]
fn rotation_geometry_and_edits_match_typescript() {
    fn equivalent(actual: Value, expected: &Value) {
        match (actual, expected) {
            (Value::Number(actual), Value::Number(expected)) => {
                assert!(
                    (actual.as_f64().unwrap() - expected.as_f64().unwrap()).abs() < 1e-9,
                    "{actual} != {expected}"
                );
            }
            (Value::Array(actual), Value::Array(expected)) => {
                assert_eq!(actual.len(), expected.len());
                for (actual, expected) in actual.into_iter().zip(expected) {
                    equivalent(actual, expected);
                }
            }
            (Value::Object(actual), Value::Object(expected)) => {
                assert_eq!(actual.len(), expected.len(), "{actual:?} != {expected:?}");
                for (key, expected) in expected {
                    equivalent(actual[key].clone(), expected);
                }
            }
            (actual, expected) => assert_eq!(&actual, expected),
        }
    }
    let cases = fixture().rotations;
    for case in cases["angles"].as_array().unwrap() {
        equivalent(
            json!(
                rotation_angle(
                    case["radians"].as_f64().unwrap(),
                    case["snap"]
                        .as_bool()
                        .unwrap()
                        .then_some(case["snapDegrees"].as_f64().unwrap_or(15.))
                )
                .unwrap()
            ),
            &case["expected"],
        );
    }
    for case in cases["handles"].as_array().unwrap() {
        let handle = rotation_handle(
            serde_json::from_value(case["outline"].clone()).unwrap(),
            case["radians"].as_f64().unwrap(),
            case["scale"].as_f64().unwrap(),
            case["canvas"]["width"].as_f64().unwrap(),
            case["canvas"]["height"].as_f64().unwrap(),
        );
        equivalent(serde_json::to_value(handle).unwrap(), &case["expected"]);
    }
    for case in cases["gestures"].as_array().unwrap() {
        let preview = preview_rotation(
            serde_json::from_value(case["outline"].clone()).unwrap(),
            case["initial"].as_f64().unwrap(),
            serde_json::from_value(case["start"].clone()).unwrap(),
            serde_json::from_value(case["current"].clone()).unwrap(),
            case["snap"]
                .as_bool()
                .unwrap()
                .then_some(case["snapDegrees"].as_f64().unwrap_or(15.)),
        )
        .unwrap();
        equivalent(serde_json::to_value(preview).unwrap(), &case["expected"]);
    }
    for case in cases["edits"].as_array().unwrap() {
        let mut document: Document = serde_json::from_value(case["input"].clone()).unwrap();
        document
            .edit_layer(
                case["id"].as_str().unwrap(),
                LayerEdit::Rotate {
                    radians: case["radians"].as_f64().unwrap(),
                },
            )
            .unwrap();
        equivalent(serde_json::to_value(document).unwrap(), &case["expected"]);
    }
}

#[test]
fn rotation_invalid_input_and_locked_noop_preserve_history() {
    let mut document = fixture().hit_tests.remove(0).input;
    let outline = document.elements[0].selection_outline().unwrap();
    let start = Point { x: 10., y: 20. };
    assert!(rotation_angle(f64::INFINITY, Some(15.)).is_none());
    for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert_eq!(
            rotation_angle(0.73, Some(invalid)),
            Some(std::f64::consts::FRAC_PI_4)
        );
    }
    assert!(rotation_handle(outline, 0., 0., 100., 100.).is_none());
    assert!(
        preview_rotation(
            outline,
            0.,
            start,
            Point {
                x: f64::NAN,
                y: 20.
            },
            None
        )
        .is_none()
    );
    let before = document.clone();
    let id = document.elements[0].base().id.clone();
    assert!(
        document
            .edit_layer(&id, LayerEdit::Rotate { radians: f64::NAN })
            .is_err()
    );
    assert_eq!(document, before);
    document
        .edit_layer(&id, LayerEdit::Lock { locked: true })
        .unwrap();
    let mut history = DocumentHistory::new(document.clone());
    document
        .edit_layer(&id, LayerEdit::Rotate { radians: 0.7 })
        .unwrap();
    assert!(!history.commit(document));
    assert_eq!(history.undo_len(), 0);
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

fn assert_json_close(actual: Value, expected: Value, context: &str) {
    match (actual, expected) {
        (Value::Number(actual), Value::Number(expected)) => assert!(
            (actual.as_f64().unwrap() - expected.as_f64().unwrap()).abs() < 1e-8,
            "{context}: {actual} != {expected}"
        ),
        (Value::Array(actual), Value::Array(expected)) => {
            assert_eq!(actual.len(), expected.len(), "{context}");
            for (actual, expected) in actual.into_iter().zip(expected) {
                assert_json_close(actual, expected, context);
            }
        }
        (Value::Object(actual), Value::Object(expected)) => {
            assert_eq!(
                actual.len(),
                expected.len(),
                "{context}: {actual:?} != {expected:?}"
            );
            for (key, expected) in expected {
                assert_json_close(actual[&key].clone(), expected, context);
            }
        }
        (actual, expected) => assert_eq!(actual, expected, "{context}"),
    }
}

#[test]
fn move_drag_previews_and_commits_match_shipping_typescript() {
    for case in fixture().moves {
        let original = case.input.clone();
        let drag = MoveDrag::new(&case.input, &case.id, case.display_scale)
            .unwrap_or_else(|error| panic!("{}: {error}", case.name));
        let actual = drag
            .preview(case.delta)
            .unwrap_or_else(|error| panic!("{}: {error}", case.name));
        let actual_json = json!({
            "element": actual.element,
            "outline": actual.outline,
            "guides": actual.guides,
        });
        assert_json_close(actual_json.clone(), case.expected, &case.name);

        // Gesture previews are always derived from the captured document, not
        // accumulated from an earlier preview, and never mutate that document.
        let _ = drag.preview(Point { x: -3.25, y: 7.75 }).unwrap();
        let repeated = drag.preview(case.delta).unwrap();
        assert_json_close(
            serde_json::to_value(repeated).unwrap(),
            actual_json,
            &format!("{} repeated preview", case.name),
        );
        assert_eq!(case.input, original, "{} mutated preview input", case.name);

        let mut committed = case.input;
        committed
            .edit_layer(
                &case.id,
                LayerEdit::DragMove {
                    delta_x: case.delta.x,
                    delta_y: case.delta.y,
                    display_scale: case.display_scale,
                },
            )
            .unwrap_or_else(|error| panic!("{}: {error}", case.name));
        assert_json_close(
            serde_json::to_value(committed).unwrap(),
            case.committed,
            &case.name,
        );
    }
}

#[test]
fn move_drag_rejects_invalid_and_unsupported_edits_and_honors_locking() {
    let case = fixture().moves.remove(0);
    assert!(MoveDrag::new(&case.input, "missing", 1.).is_err());
    assert!(MoveDrag::new(&case.input, &case.id, f64::NAN).is_err());
    assert!(MoveDrag::new(&case.input, &case.id, f64::INFINITY).is_err());
    let drag = MoveDrag::new(&case.input, &case.id, 1.).unwrap();
    assert!(drag.preview(Point { x: f64::NAN, y: 0. }).is_err());
    assert!(
        drag.preview(Point {
            x: 0.,
            y: f64::INFINITY
        })
        .is_err()
    );

    let mut invalid = case.input.clone();
    let before = invalid.clone();
    assert!(
        invalid
            .edit_layer(
                &case.id,
                LayerEdit::DragMove {
                    delta_x: f64::NAN,
                    delta_y: 1.,
                    display_scale: 1.,
                },
            )
            .is_err()
    );
    assert_eq!(invalid, before);

    let mut missing = case.input.clone();
    let before = missing.clone();
    assert!(
        missing
            .edit_layer(
                "missing",
                LayerEdit::DragMove {
                    delta_x: 1.,
                    delta_y: 1.,
                    display_scale: 1.,
                },
            )
            .is_err()
    );
    assert_eq!(missing, before);

    let mut locked = case.input;
    locked
        .edit_layer(&case.id, LayerEdit::Lock { locked: true })
        .unwrap();
    assert!(
        MoveDrag::new(&locked, &case.id, 1.)
            .unwrap()
            .preview(Point { x: 10., y: 12. })
            .is_ok()
    );
    let before = locked.clone();
    locked
        .edit_layer(
            &case.id,
            LayerEdit::DragMove {
                delta_x: 10.,
                delta_y: 12.,
                display_scale: 1.,
            },
        )
        .unwrap();
    assert_eq!(locked, before);

    let mut unsupported = fixture().moves.remove(0).input;
    let mut text = fixture().document["elements"][1].clone();
    text["locked"] = json!(false);
    text["fontSize"] = json!(0);
    unsupported
        .elements
        .push(serde_json::from_value(text).unwrap());
    assert!(MoveDrag::new(&unsupported, "locked-text", 1.).is_err());
    let before = unsupported.clone();
    assert!(
        unsupported
            .edit_layer(
                "locked-text",
                LayerEdit::DragMove {
                    delta_x: 1.,
                    delta_y: 2.,
                    display_scale: 1.,
                },
            )
            .is_err()
    );
    assert_eq!(unsupported, before);
}

#[test]
fn resize_handles_previews_and_commits_match_typescript() {
    let resize = fixture().resize;
    assert_eq!(
        serde_json::to_value([
            ResizeHandle::Nw,
            ResizeHandle::N,
            ResizeHandle::Ne,
            ResizeHandle::E,
            ResizeHandle::Se,
            ResizeHandle::S,
            ResizeHandle::Sw,
            ResizeHandle::W,
        ])
        .unwrap(),
        json!(["nw", "n", "ne", "e", "se", "s", "sw", "w"])
    );
    for case in resize.hit_tests {
        for query in case.queries {
            assert_eq!(
                case.element
                    .resize_handle_at(query.point, case.radius)
                    .unwrap(),
                query.expected,
                "{} at {:?}",
                case.name,
                query.point
            );
        }
    }
    for case in resize.drags {
        let drag = ResizeDrag::new(&case.input, &case.id, case.handle, case.display_scale)
            .unwrap_or_else(|error| panic!("{}: {error}", case.name));
        let actual = drag.preview(case.current, case.lock_aspect).unwrap();
        assert_json_close(
            json!({
                "element": actual.element,
                "outline": actual.outline,
                "guides": actual.guides,
            }),
            case.expected,
            &case.name,
        );

        let mut committed = case.input;
        committed
            .edit_layer(
                &case.id,
                LayerEdit::Resize {
                    handle: case.handle,
                    current: case.current,
                    display_scale: case.display_scale,
                    lock_aspect: case.lock_aspect,
                },
            )
            .unwrap_or_else(|error| panic!("{}: {error}", case.name));
        assert_json_close(
            serde_json::to_value(committed).unwrap(),
            case.committed,
            &case.name,
        );
    }
}

#[test]
fn resize_rejects_invalid_locked_and_unsupported_edits_transactionally() {
    let case = fixture().resize.drags.remove(0);
    let element = &case.input.elements[0];
    assert!(
        element
            .resize_handle_at(Point { x: f64::NAN, y: 1. }, 5.)
            .is_err()
    );
    assert!(
        element
            .resize_handle_at(Point { x: 1., y: 1. }, f64::INFINITY)
            .is_err()
    );
    assert!(ResizeDrag::new(&case.input, "missing", case.handle, 1.).is_err());
    assert!(ResizeDrag::new(&case.input, &case.id, case.handle, f64::NAN).is_err());
    let drag = ResizeDrag::new(&case.input, &case.id, case.handle, 1.).unwrap();
    assert!(
        drag.preview(
            Point {
                x: f64::INFINITY,
                y: 1.
            },
            false
        )
        .is_err()
    );

    let mut invalid = case.input.clone();
    let before = invalid.clone();
    assert!(
        invalid
            .edit_layer(
                &case.id,
                LayerEdit::Resize {
                    handle: case.handle,
                    current: Point { x: f64::NAN, y: 1. },
                    display_scale: 1.,
                    lock_aspect: false,
                }
            )
            .is_err()
    );
    assert_eq!(invalid, before);

    let mut locked = case.input.clone();
    locked
        .edit_layer(&case.id, LayerEdit::Lock { locked: true })
        .unwrap();
    let before = locked.clone();
    locked
        .edit_layer(
            &case.id,
            LayerEdit::Resize {
                handle: case.handle,
                current: case.current,
                display_scale: 1.,
                lock_aspect: false,
            },
        )
        .unwrap();
    assert_eq!(locked, before);

    let mut unsupported = case.input;
    let mut text = fixture().document["elements"][1].clone();
    text["locked"] = json!(false);
    text["fontSize"] = json!(0);
    unsupported
        .elements
        .push(serde_json::from_value(text).unwrap());
    let before = unsupported.clone();
    assert!(ResizeDrag::new(&unsupported, "locked-text", ResizeHandle::Se, 1.).is_err());
    assert!(
        unsupported
            .edit_layer(
                "locked-text",
                LayerEdit::Resize {
                    handle: ResizeHandle::Se,
                    current: Point { x: 700., y: 100. },
                    display_scale: 1.,
                    lock_aspect: true,
                }
            )
            .is_err()
    );
    assert_eq!(unsupported, before);
}

#[test]
fn canvas_selection_bounds_and_hits_match_typescript() {
    for case in fixture().hit_tests {
        if let Some(expected) = case.outline {
            for (actual, expected) in case.input.elements[0]
                .selection_outline()
                .unwrap()
                .into_iter()
                .zip(expected)
            {
                assert!(
                    (actual.x - expected.x).abs() < 1e-9 && (actual.y - expected.y).abs() < 1e-9,
                    "{}: {actual:?} != {expected:?}",
                    case.name
                );
            }
        }
        if let Some(expected) = case.bounds {
            let actual = case.input.elements[0].selection_bounds().unwrap();
            for (actual, expected) in [
                (actual.x, expected.x),
                (actual.y, expected.y),
                (actual.width, expected.width),
                (actual.height, expected.height),
            ] {
                assert!(
                    (actual - expected).abs() < 1e-9,
                    "{}: {actual} != {expected}",
                    case.name
                );
            }
        }
        for query in case.queries {
            let hit = case.input.hit_test(query.point, query.tolerance).unwrap();
            assert_eq!(
                hit.map(|element| element.base().id.as_str()),
                query.expected.as_deref(),
                "{} at {:?}, tolerance {}",
                case.name,
                query.point,
                query.tolerance,
            );
        }
    }
}

#[test]
fn canvas_selection_reports_invalid_layout_and_pointer_inputs() {
    let mut input = fixture().hit_tests.remove(0).input;
    let point = Point { x: 0., y: 20. };
    let mut text = fixture().document["elements"][1].clone();
    text["locked"] = json!(false);
    text["fontSize"] = json!(0);
    input.elements.push(serde_json::from_value(text).unwrap());
    assert!(input.hit_test(point, 0.).unwrap_err().contains("Text size"));
    assert!(input.elements[1].selection_bounds().is_err());
    let Element::Text(text) = &mut input.elements[1] else {
        panic!()
    };
    text.base.visible = false;
    assert_eq!(
        input.hit_test(point, 0.).unwrap().unwrap().base().id,
        "image"
    );
    let Element::Text(text) = &mut input.elements[1] else {
        panic!()
    };
    text.base.visible = true;
    text.base.locked = true;
    assert_eq!(
        input.hit_test(point, 0.).unwrap().unwrap().base().id,
        "image"
    );
    let front = input.elements[0].clone();
    let Element::Text(text) = &mut input.elements[1] else {
        panic!()
    };
    text.base.locked = false;
    input.elements.push(front);
    assert_eq!(
        input.hit_test(point, 0.).unwrap().unwrap().base().id,
        "image"
    );
    for (point, tolerance) in [
        (
            Point {
                x: f64::NAN,
                y: 20.,
            },
            0.,
        ),
        (
            Point {
                x: 0.,
                y: f64::INFINITY,
            },
            0.,
        ),
        (point, f64::NAN),
        (point, f64::INFINITY),
        (point, -1.),
    ] {
        assert!(input.hit_test(point, tolerance).is_err());
    }
    let mut shape = fixture().document["elements"][2].clone();
    shape["shape"] = json!("future-shape");
    input.elements.push(serde_json::from_value(shape).unwrap());
    assert!(
        input
            .hit_test(point, 0.)
            .unwrap_err()
            .contains("Unsupported shape")
    );
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
fn canvas_trim_matches_shipping_bounds_and_is_idempotent() {
    for case in fixture().trims {
        let mut document = case.input;
        document.trim_to_content().expect(&case.name);
        assert_eq!(document.width, case.expected["width"].as_f64().unwrap());
        assert_eq!(document.height, case.expected["height"].as_f64().unwrap());
        assert_json_close(
            serde_json::to_value(&document).unwrap(),
            case.expected,
            &case.name,
        );
        let trimmed = document.clone();
        document.trim_to_content().expect(&case.name);
        assert_eq!(document, trimmed, "{}", case.name);
    }
}

#[test]
fn interactive_crop_drag_matches_typescript_state_transitions() {
    for case in fixture().crop_drags {
        let bounds = case.bounds.as_rect();
        let mut drag = CropDrag::new(
            case.origin,
            bounds,
            case.initial.preset_aspect,
            case.initial.shift_key,
        );
        assert_eq!(drag.rect(), case.initial.expected);

        for step in case.updates {
            assert_eq!(
                drag.update(step.current, step.preset_aspect, step.shift_key),
                step.expected
            );
            assert_eq!(drag.rect(), step.expected);
        }
    }
}

#[test]
fn closed_shape_creation_and_outside_expansion_match_typescript() {
    for case in fixture().shape_creations {
        let mut document = case.input;
        let id = document.create_closed_shape(case.request).unwrap();
        let Some(Element::Shape(created)) = document.elements.last_mut() else {
            panic!("{} did not append a shape", case.name)
        };
        assert_eq!(created.base.id, id);
        created.base.id = "fixture-created-shape".into();
        assert_json_equivalent(serde_json::to_value(document).unwrap(), case.expected);
    }
}

#[test]
fn degenerate_closed_shape_creation_is_rejected_without_inventing_pixels() {
    let original = Document::new_capture("asset://original", 7., 3., None);
    for (start, end) in [
        (json!({"x": 2, "y": 1}), json!({"x": 2, "y": 2})),
        (json!({"x": 2, "y": 1}), json!({"x": 4, "y": 1})),
        (json!({"x": 2, "y": 1}), json!({"x": 2, "y": 1})),
    ] {
        let create: ClosedShapeCreate = serde_json::from_value(json!({
            "shape": "rectangle",
            "start": start,
            "end": end,
        }))
        .unwrap();
        let mut document = original.clone();
        assert!(document.create_closed_shape(create).is_err());
        assert_eq!(document, original);
    }
}

#[test]
fn straight_line_and_arrow_creation_bounds_match_typescript() {
    for case in fixture().open_shape_creations {
        let original = case.input.clone();
        let mut document = case.input;
        let result = document.create_open_shape(case.request);
        let Some(expected) = case.expected else {
            assert!(result.is_err(), "{} ({})", case.name, case.path_length);
            assert_eq!(document, original, "{}", case.name);
            continue;
        };
        let id = result.unwrap();
        let Some(Element::Shape(created)) = document.elements.last_mut() else {
            panic!("{} did not append a shape", case.name)
        };
        assert_eq!(created.base.id, id);
        created.base.id = "fixture-created-open-shape".into();
        assert_json_equivalent(serde_json::to_value(document).unwrap(), expected);
    }
}

#[test]
fn freehand_creation_bounds_and_translation_match_typescript() {
    for case in fixture().freehand_creations {
        let mut document = case.input;
        let id = document.create_freehand_path(case.request).unwrap();
        let Some(Element::Path(created)) = document.elements.last_mut() else {
            panic!("{} did not append a freehand path", case.name)
        };
        assert_eq!(created.base.id, id);
        created.base.id = "fixture-created-freehand-path".into();
        assert_json_equivalent(serde_json::to_value(document).unwrap(), case.expected);
    }
}

#[test]
fn freehand_creation_rejects_empty_and_non_renderer_geometry_without_mutation() {
    let original = Document::new_capture("asset://original", 7., 3., None);
    for points in [
        Vec::new(),
        vec![Point { x: f64::NAN, y: 1. }],
        vec![Point {
            x: f64::from(f32::MAX) * 2.,
            y: 1.,
        }],
    ] {
        let mut document = original.clone();
        assert!(
            document
                .create_freehand_path(FreehandPathCreate {
                    points,
                    style: Default::default(),
                    opacity: 100.,
                })
                .is_err()
        );
        assert_eq!(document, original);
    }
}

#[test]
fn freehand_preview_centerline_uses_the_shared_renderer_smoothing() {
    let points = [
        Point { x: 2., y: 5. },
        Point { x: 8., y: -7. },
        Point { x: 14., y: 5. },
    ];
    let samples = smooth_path_centerline(&points);
    assert_eq!(samples.len(), 26);
    assert_eq!(samples[0], points[0]);
    assert_eq!(*samples.last().unwrap(), points[2]);
    assert_eq!(samples[12], Point { x: 7.25, y: -2.5 });
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
fn fresh_photo_rotation_fits_the_canvas_while_layered_overhang_stays_clipped() {
    let mut fresh = Document::new_capture("asset://fresh", 7., 3., None);
    fresh
        .edit_layer(
            "capture-background",
            LayerEdit::ImageTransform {
                transform: ImageTransform::RotateClockwise,
            },
        )
        .unwrap();
    assert_eq!((fresh.width, fresh.height), (3., 7.));
    let Element::Image(image) = &fresh.elements[0] else {
        panic!()
    };
    assert_eq!((image.base.x, image.base.y), (0., 0.));
    assert_eq!((image.width, image.height), (3., 7.));

    let mut layered = Document::new_capture("asset://layered", 7., 3., None);
    let Element::Image(mut overlay) = layered.elements[0].clone() else {
        panic!()
    };
    overlay.base.id = "overlay".into();
    overlay.base.x = 1.;
    overlay.base.y = 1.;
    overlay.width = 2.;
    overlay.height = 1.;
    layered.elements.push(Element::Image(overlay.clone()));
    layered
        .edit_layer(
            "capture-background",
            LayerEdit::ImageTransform {
                transform: ImageTransform::RotateClockwise,
            },
        )
        .unwrap();
    assert_eq!((layered.width, layered.height), (7., 3.));
    let Element::Image(rotated) = &layered.elements[0] else {
        panic!()
    };
    assert_eq!((rotated.base.x, rotated.base.y), (2., -2.));
    assert_eq!(layered.elements[1], Element::Image(overlay));
}

#[test]
fn fully_off_canvas_transform_expands_and_translates_every_layer() {
    let mut document = Document::new_capture("asset://outside", 7., 3., None);
    let Element::Image(image) = &mut document.elements[0] else {
        panic!()
    };
    image.base.x = -10.;
    image.base.y = 1.;
    let Element::Image(mut hidden) = document.elements[0].clone() else {
        panic!()
    };
    hidden.base.id = "hidden".into();
    hidden.base.x = 2.;
    hidden.base.y = 2.;
    hidden.base.visible = false;
    document.elements.push(Element::Image(hidden));

    document
        .edit_layer(
            "capture-background",
            LayerEdit::ImageTransform {
                transform: ImageTransform::RotateClockwise,
            },
        )
        .unwrap();
    assert_eq!((document.width, document.height), (15., 7.));
    let Element::Image(rotated) = &document.elements[0] else {
        panic!()
    };
    assert_eq!((rotated.base.x, rotated.base.y), (0., 0.));
    assert_eq!((rotated.width, rotated.height), (3., 7.));
    assert_eq!(
        (document.elements[1].base().x, document.elements[1].base().y),
        (10., 3.)
    );
}

#[test]
fn hidden_images_skip_fresh_photo_fit_and_fill_tolerance_is_strict() {
    let mut hidden = Document::new_capture("asset://hidden", 7., 3., None);
    let Element::Image(image) = &mut hidden.elements[0] else {
        panic!()
    };
    image.base.visible = false;
    hidden
        .edit_layer(
            "capture-background",
            LayerEdit::ImageTransform {
                transform: ImageTransform::RotateClockwise,
            },
        )
        .unwrap();
    assert_eq!((hidden.width, hidden.height), (7., 3.));
    let Element::Image(image) = &hidden.elements[0] else {
        panic!()
    };
    assert_eq!((image.base.x, image.base.y), (2., -2.));
    assert_eq!((image.width, image.height), (3., 7.));

    let rotate = |x| {
        let mut document = Document::new_capture("asset://tolerance", 7., 3., None);
        let Element::Image(image) = &mut document.elements[0] else {
            panic!()
        };
        image.base.x = x;
        document
            .edit_layer(
                "capture-background",
                LayerEdit::ImageTransform {
                    transform: ImageTransform::RotateClockwise,
                },
            )
            .unwrap();
        document
    };
    let inside = rotate(0.009);
    assert_eq!((inside.width, inside.height), (4., 7.));
    assert!((inside.elements[0].base().x - 0.009).abs() < 1e-12);
    assert_eq!(inside.elements[0].base().y, 0.);
    let boundary = rotate(0.01);
    assert_eq!((boundary.width, boundary.height), (7., 3.));
    assert_eq!(
        (boundary.elements[0].base().x, boundary.elements[0].base().y),
        (2.01, -2.)
    );
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
