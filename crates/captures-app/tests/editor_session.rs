use std::{collections::BTreeMap, fs, path::Path, sync::Arc};

use captures_app::{
    editor::{
        AnnotationStylePatch, Element, ImageOrientation, LayerEdit, OptionalNullable, Point, Rect,
    },
    editor_session::{EditorSession, ImportImage, OpenRequest, Request},
};
use captures_capture::CaptureMode;
use image::{Rgba, RgbaImage};
use serde_json::json;

fn setup() -> (tempfile::TempDir, String, RgbaImage) {
    let data = tempfile::tempdir().unwrap();
    let image = RgbaImage::from_fn(7, 3, |x, y| Rgba([x as u8 * 31, y as u8 * 71, 19, 255]));
    let capture =
        captures_app::persist_screenshot(&data.path().join("history"), &image, CaptureMode::Region)
            .unwrap();
    (data, capture.entry.id, image)
}

fn open(root: &Path, id: &str) -> Result<EditorSession, String> {
    EditorSession::open(OpenRequest {
        history_root: root.join("history"),
        drafts_root: root.join("drafts"),
        artifact_id: id.into(),
    })
}

fn crop() -> Request {
    Request::Crop {
        rect: Rect {
            x: 2.,
            y: 1.,
            width: 4.,
            height: 2.,
        },
    }
}

fn image_transform(id: &str, transform: &str) -> Request {
    serde_json::from_value(json!({
        "operation": "layer",
        "id": id,
        "edit": {
            "action": "image_transform",
            "transform": transform,
        },
    }))
    .unwrap()
}

#[test]
fn trim_is_one_undo_step_preserves_pixels_redo_and_original_and_reopens() {
    let (data, id, original) = setup();
    let path = data.path().join("history").join(&id).join("capture.png");
    let original_file = fs::read(&path).unwrap();
    let mut editor = open(data.path(), &id).unwrap();
    editor
        .execute(Request::ResizeCanvas {
            width: 12.,
            height: 8.,
        })
        .unwrap();
    let enlarged = editor.snapshot().document.clone();
    let trim = || serde_json::from_value::<Request>(json!({"operation": "trim_canvas"})).unwrap();
    editor.execute(trim()).unwrap();
    assert_eq!(editor.pixels().as_ref(), &original);
    assert_eq!(
        (
            editor.snapshot().document.width,
            editor.snapshot().document.height
        ),
        (7., 3.)
    );
    assert_eq!(editor.snapshot().document.elements, enlarged.elements);
    editor.execute(Request::Undo).unwrap();
    assert_eq!(editor.snapshot().document, &enlarged);
    editor.execute(Request::Redo).unwrap();
    editor
        .execute(Request::ResizeCanvas {
            width: 11.,
            height: 6.,
        })
        .unwrap();
    editor.execute(Request::Undo).unwrap();
    let pixels = editor.pixels();
    editor.execute(trim()).unwrap();
    assert!(
        editor.snapshot().can_redo,
        "already-tight trim must preserve redo"
    );
    assert!(Arc::ptr_eq(&pixels, &editor.pixels()));
    editor
        .execute(Request::SaveDraft { updated_at_ms: 29 })
        .unwrap();
    let reopened = open(data.path(), &id).unwrap();
    assert_eq!(reopened.snapshot().document, editor.snapshot().document);
    assert_eq!(reopened.pixels(), editor.pixels());
    assert_eq!(fs::read(path).unwrap(), original_file);
}

#[test]
fn trim_render_failure_keeps_accepted_frame_draft_and_redo() {
    let (data, id, _) = setup();
    let mut editor = open(data.path(), &id).unwrap();
    editor
        .execute(Request::SaveDraft { updated_at_ms: 31 })
        .unwrap();
    let manifest = data.path().join("drafts").join(&id).join("manifest.json");
    let mut value: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
    let mut off_canvas = value["document"]["elements"][0].clone();
    off_canvas["id"] = json!("overhanging-image");
    off_canvas["x"] = json!(-20_000);
    value["document"]["elements"]
        .as_array_mut()
        .unwrap()
        .push(off_canvas);
    fs::write(&manifest, serde_json::to_vec(&value).unwrap()).unwrap();
    let mut editor = open(data.path(), &id).unwrap();
    editor
        .execute(Request::ResizeCanvas {
            width: 8.,
            height: 4.,
        })
        .unwrap();
    editor.execute(Request::Undo).unwrap();
    let before = serde_json::to_value(editor.snapshot()).unwrap();
    let pixels = editor.pixels();
    let persisted = fs::read(&manifest).unwrap();
    let error = editor.execute(Request::TrimCanvas).unwrap_err();
    assert!(error.contains("16384 pixels per side"), "{error}");
    assert_eq!(serde_json::to_value(editor.snapshot()).unwrap(), before);
    assert!(editor.snapshot().can_redo);
    assert!(Arc::ptr_eq(&pixels, &editor.pixels()));
    assert_eq!(fs::read(manifest).unwrap(), persisted);
}

#[test]
fn background_commands_preserve_layers_and_are_transactional_undoable_and_persisted() {
    let (data, id, original) = setup();
    let path = data.path().join("history").join(&id).join("capture.png");
    let original_file = fs::read(&path).unwrap();
    let mut editor = open(data.path(), &id).unwrap();
    editor
        .execute(Request::ResizeCanvas {
            width: 10.,
            height: 5.,
        })
        .unwrap();
    let layers = editor.snapshot().document.elements.clone();
    let set = |color: Option<&str>| {
        serde_json::from_value::<Request>(json!({
            "operation": "set_background", "color": color,
        }))
        .unwrap()
    };
    editor.execute(set(Some("#21436580"))).unwrap();
    assert_eq!(editor.pixels().get_pixel(9, 4), &Rgba([33, 67, 101, 128]));
    assert_eq!(editor.pixels().get_pixel(2, 1), original.get_pixel(2, 1));
    assert_eq!(editor.snapshot().document.elements, layers);
    let before = serde_json::to_value(editor.snapshot()).unwrap();
    let pixels = editor.pixels();
    assert!(editor.execute(set(Some("invalid"))).is_err());
    assert_eq!(serde_json::to_value(editor.snapshot()).unwrap(), before);
    assert!(Arc::ptr_eq(&pixels, &editor.pixels()));
    editor.execute(set(None)).unwrap();
    assert_eq!(editor.pixels().get_pixel(9, 4), &Rgba([0, 0, 0, 0]));
    editor.execute(Request::Undo).unwrap();
    assert_eq!(
        editor.snapshot().document.background.as_deref(),
        Some("#21436580")
    );
    let pixels = editor.pixels();
    editor.execute(set(Some("#21436580"))).unwrap();
    assert!(editor.snapshot().can_redo, "a no-op must not clear redo");
    assert!(Arc::ptr_eq(&pixels, &editor.pixels()));
    editor.execute(Request::Redo).unwrap();
    editor
        .execute(Request::SaveDraft { updated_at_ms: 17 })
        .unwrap();
    let reopened = open(data.path(), &id).unwrap();
    assert_eq!(reopened.snapshot().document.background, None);
    assert_eq!(reopened.snapshot().document.elements, layers);
    assert_eq!(reopened.pixels(), editor.pixels());
    assert_eq!(fs::read(path).unwrap(), original_file);
}

#[test]
fn image_background_edits_retain_original_assets_and_preserve_redo_on_failure() {
    let (data, id, original) = setup();
    let path = data.path().join("history").join(&id).join("capture.png");
    let original_file = fs::read(&path).unwrap();
    let mut editor = open(data.path(), &id).unwrap();
    let retained_frame = editor.pixels();
    let wand = |x| Request::RemoveImageBackground {
        point: Point { x, y: 1.5 },
        tolerance: 0.,
        contiguous: true,
    };
    let Element::Image(initial) = &editor.snapshot().document.elements[0] else {
        panic!()
    };
    let original_source = initial.src.clone();
    assert!(
        initial.base.locked,
        "the original capture must be editable without unlocking"
    );
    editor.execute(wand(2.5)).unwrap();
    let Element::Image(first) = &editor.snapshot().document.elements[0] else {
        panic!()
    };
    let first_source = first.src.clone();
    assert_ne!(first_source, original_source);
    assert_eq!(
        first.original_src,
        OptionalNullable::Value(original_source.clone())
    );
    assert_eq!(first.base.id, "capture-background");
    assert_eq!(editor.snapshot().document.background, None);
    let mut expected = original.clone();
    expected.put_pixel(2, 1, Rgba([0; 4]));
    assert_eq!(*editor.pixels(), expected);
    editor.execute(wand(3.5)).unwrap();
    editor.execute(Request::Undo).unwrap();
    let before = serde_json::to_value(editor.snapshot()).unwrap();
    let pixels = editor.pixels();
    for x in [2.5, 7., f64::NAN] {
        assert!(editor.execute(wand(x)).is_err());
        assert_eq!(serde_json::to_value(editor.snapshot()).unwrap(), before);
        assert!(Arc::ptr_eq(&pixels, &editor.pixels()));
        assert!(editor.snapshot().can_redo);
    }
    editor.execute(Request::Redo).unwrap();
    expected.put_pixel(3, 1, Rgba([0; 4]));
    assert_eq!(*editor.pixels(), expected);
    editor
        .execute(Request::SaveDraft { updated_at_ms: 29 })
        .unwrap();
    let mut reopened = open(data.path(), &id).unwrap();
    assert_eq!(*reopened.pixels(), expected);
    reopened.execute(wand(4.5)).unwrap();
    let Element::Image(reopened_image) = &reopened.snapshot().document.elements[0] else {
        panic!()
    };
    assert_eq!(
        reopened_image.original_src,
        OptionalNullable::Value(original_source)
    );
    assert_ne!(reopened_image.src, first_source);
    assert_eq!(
        *retained_frame, original,
        "old UI frames and source bitmaps remain immutable"
    );
    assert_eq!(fs::read(path).unwrap(), original_file);
}

fn background_brush(points: &[(f64, f64)], mode: &str) -> Request {
    serde_json::from_value(json!({
        "operation": "paint_image_background",
        "points": points.iter().map(|(x, y)| json!({"x": x, "y": y})).collect::<Vec<_>>(),
        "size": 2.0,
        "softness": 0.0,
        "mode": mode,
    }))
    .unwrap()
}

#[test]
fn background_brush_erases_restores_and_undoes_each_completed_stroke() {
    let (data, id, original) = setup();
    let mut editor = open(data.path(), &id).unwrap();
    editor
        .execute(Request::SetBackground {
            color: Some("#123456".into()),
        })
        .unwrap();
    let Element::Image(initial) = &editor.snapshot().document.elements[0] else {
        panic!()
    };
    assert!(initial.base.locked);
    let original_src = initial.src.clone();

    editor
        .execute(background_brush(&[(2.5, 1.5)], "erase"))
        .unwrap();
    assert_eq!(editor.pixels().get_pixel(2, 1), &Rgba([0; 4]));
    let Element::Image(erased) = &editor.snapshot().document.elements[0] else {
        panic!()
    };
    assert_eq!(
        erased.original_src,
        OptionalNullable::Value(original_src.clone())
    );
    assert_eq!(editor.snapshot().document.background, None);
    editor.execute(Request::Undo).unwrap();
    assert_eq!(editor.pixels().get_pixel(2, 1), original.get_pixel(2, 1));
    editor.execute(Request::Redo).unwrap();
    assert_eq!(editor.pixels().get_pixel(2, 1), &Rgba([0; 4]));

    editor
        .execute(background_brush(&[(2.5, 1.5)], "restore"))
        .unwrap();
    assert_eq!(*editor.pixels(), original);
    let Element::Image(restored) = &editor.snapshot().document.elements[0] else {
        panic!()
    };
    assert_eq!(
        restored.original_src,
        OptionalNullable::Value(original_src.clone())
    );
    editor
        .execute(Request::SaveDraft { updated_at_ms: 31 })
        .unwrap();
    let reopened = open(data.path(), &id).unwrap();
    let Element::Image(reopened_image) = &reopened.snapshot().document.elements[0] else {
        panic!()
    };
    assert_eq!(
        reopened_image.original_src,
        OptionalNullable::Value(original_src)
    );
    assert_eq!(reopened.pixels().get_pixel(2, 1), original.get_pixel(2, 1));
}

#[test]
fn background_brush_locks_initial_target_and_failures_and_noops_are_atomic() {
    let (data, id, _) = setup();
    let mut editor = open(data.path(), &id).unwrap();
    editor
        .import_image(ImportImage {
            pixels: RgbaImage::from_pixel(2, 3, Rgba([200, 30, 40, 255])),
            name: "front".into(),
            selected_id: None,
            point: Some(Point { x: 1., y: 1.5 }),
        })
        .unwrap();
    let mut document = editor.snapshot().document.clone();
    let Element::Image(front) = &mut document.elements[1] else {
        panic!()
    };
    front.base.x = 0.;
    front.base.y = 0.;
    front.width = 2.;
    front.height = 3.;
    front.base.locked = true;
    editor.execute(Request::Commit { document }).unwrap();
    editor
        .execute(background_brush(&[(1.5, 1.5), (5.5, 1.5)], "erase"))
        .unwrap();
    assert_eq!(editor.pixels().get_pixel(5, 1)[3], 255);
    assert_eq!(editor.pixels().get_pixel(1, 1)[3], 0);

    editor.execute(Request::Undo).unwrap();
    let before = serde_json::to_value(editor.snapshot()).unwrap();
    let frame = editor.pixels();
    for request in [
        background_brush(&[], "erase"),
        background_brush(&[(20., 20.)], "erase"),
        background_brush(&[(1.5, 1.5)], "restore"),
        serde_json::from_value(json!({
            "operation": "paint_image_background", "points": [{"x": 1, "y": 1}],
            "size": 0, "softness": 0, "mode": "erase"
        }))
        .unwrap(),
    ] {
        assert!(editor.execute(request).is_err());
        assert_eq!(serde_json::to_value(editor.snapshot()).unwrap(), before);
        assert!(Arc::ptr_eq(&frame, &editor.pixels()));
        assert!(editor.snapshot().can_redo);
    }
    editor.execute(Request::Redo).unwrap();
    editor
        .execute(Request::SetBackground {
            color: Some("#123456".into()),
        })
        .unwrap();
    editor
        .execute(Request::SetBackground {
            color: Some("#654321".into()),
        })
        .unwrap();
    editor.execute(Request::Undo).unwrap();
    assert!(editor.snapshot().can_redo);
    let frame = editor.pixels();
    let before = serde_json::to_value(editor.snapshot()).unwrap();
    editor
        .execute(background_brush(&[(1.5, 1.5)], "erase"))
        .unwrap();
    assert_eq!(serde_json::to_value(editor.snapshot()).unwrap(), before);
    assert!(Arc::ptr_eq(&frame, &editor.pixels()));
}

#[test]
fn background_brush_scales_radius_in_oriented_natural_pixels_and_keeps_first_target() {
    let (data, id, original) = setup();
    let mut editor = open(data.path(), &id).unwrap();
    let mut document = editor.snapshot().document.clone();
    document.width = 9.;
    document.height = 14.;
    let Element::Image(image) = &mut document.elements[0] else {
        panic!()
    };
    image.orientation = Some(ImageOrientation::Rotate90);
    image.width = 9.;
    image.height = 14.;
    editor.execute(Request::Commit { document }).unwrap();
    editor
        .execute(
            serde_json::from_value(json!({
                "operation": "paint_image_background", "mode": "erase", "size": 6, "softness": 0,
                "points": [{"x": 1.5, "y": 5}, {"x": -100, "y": 5}]
            }))
            .unwrap(),
        )
        .unwrap();
    editor
        .execute(Request::SaveDraft { updated_at_ms: 37 })
        .unwrap();
    let Element::Image(image) = &editor.snapshot().document.elements[0] else {
        panic!()
    };
    let asset = data
        .path()
        .join("drafts")
        .join(&id)
        .join("assets")
        .join(format!(
            "{}.png",
            image.src.strip_prefix("draft-asset:").unwrap()
        ));
    let actual = image::open(asset).unwrap().to_rgba8();
    // 90° swaps the displayed natural width to 3. A six-document-pixel brush
    // on a nine-pixel-wide layer has radius 1, not 7/3. The seed is (2,2).
    let mut expected = original;
    for (x, y) in [(1, 1), (2, 1), (1, 2), (2, 2)] {
        expected.put_pixel(x, y, Rgba([0; 4]));
    }
    assert_eq!(actual, expected);
}

#[test]
fn wand_picks_front_visible_locked_image_without_modifying_hidden_or_underlying_images() {
    let (data, id, original) = setup();
    let mut editor = open(data.path(), &id).unwrap();
    editor
        .import_image(ImportImage {
            pixels: RgbaImage::from_pixel(7, 3, Rgba([201, 43, 71, 255])),
            name: "Overlay".into(),
            selected_id: None,
            point: Some(Point { x: 3.5, y: 1.5 }),
        })
        .unwrap();
    let mut document = editor.snapshot().document.clone();
    let Element::Image(front) = &mut document.elements[1] else {
        panic!()
    };
    front.base.x = 0.;
    front.base.y = 0.;
    front.base.locked = true;
    front.width = 7.;
    front.height = 3.;
    let source = front.src.clone();
    let mut hidden = front.clone();
    hidden.base.id = "hidden-front".into();
    hidden.base.visible = false;
    document.elements.push(Element::Image(hidden));
    editor.execute(Request::Commit { document }).unwrap();
    editor
        .execute(Request::RemoveImageBackground {
            point: Point { x: 2.5, y: 1.5 },
            tolerance: 0.,
            contiguous: true,
        })
        .unwrap();
    let snapshot = editor.snapshot();
    let Element::Image(front) = &snapshot.document.elements[1] else {
        panic!()
    };
    let Element::Image(hidden) = &snapshot.document.elements[2] else {
        panic!()
    };
    assert_ne!(front.src, source);
    assert_eq!(hidden.src, source);
    assert_eq!(
        *editor.pixels(),
        original,
        "clearing the top visible image reveals the unmodified capture"
    );
    let before = serde_json::to_value(editor.snapshot()).unwrap();
    assert!(
        editor
            .execute(Request::RemoveImageBackground {
                point: Point { x: 2.5, y: 1.5 },
                tolerance: 0.,
                contiguous: true,
            })
            .is_err(),
        "a transparent top image must not expose the underlying image to the wand"
    );
    assert_eq!(serde_json::to_value(editor.snapshot()).unwrap(), before);
}

#[test]
fn wand_tolerance_rounds_and_clamps_like_shipping_controls() {
    for (tolerance, cleared) in [(-1., 1), (30.49, 1), (30.5, 2), (256., 21)] {
        let (data, id, _) = setup();
        let mut editor = open(data.path(), &id).unwrap();
        editor
            .execute(Request::RemoveImageBackground {
                point: Point { x: 0., y: 0. },
                tolerance,
                contiguous: true,
            })
            .unwrap();
        assert_eq!(
            editor
                .pixels()
                .pixels()
                .filter(|pixel| pixel[3] == 0)
                .count(),
            cleared
        );
    }
}

#[test]
fn exports_encode_current_pixels_without_mutating_session_or_original_files() {
    use captures_app::editor_session::ExportOptions;

    let (data, id, _) = setup();
    let directory = data.path().join("history").join(&id);
    let original = fs::read(directory.join("capture.png")).unwrap();
    let metadata = fs::read(directory.join("metadata.json")).unwrap();
    let mut editor = open(data.path(), &id).unwrap();
    let mut document = editor.snapshot().document.clone();
    document.background = None;
    editor.execute(Request::Commit { document }).unwrap();
    editor.execute(crop()).unwrap();
    editor
        .execute(Request::ResizeCanvas {
            width: 6.,
            height: 3.,
        })
        .unwrap();
    editor
        .execute(Request::ResizeCanvas {
            width: 9.,
            height: 5.,
        })
        .unwrap();
    editor.execute(Request::Undo).unwrap();
    assert!(editor.snapshot().can_undo && editor.snapshot().can_redo);
    assert!(editor.snapshot().unsaved_changes);
    let before = serde_json::to_value(editor.snapshot()).unwrap();
    let pixels = editor.pixels();
    let expected = RgbaImage::from_fn(6, 3, |x, y| {
        // Crop translates layers without destroying off-canvas source pixels.
        if x < 5 && y < 2 {
            Rgba([(x + 2) as u8 * 31, (y + 1) as u8 * 71, 19, 255])
        } else {
            Rgba([0, 0, 0, 0])
        }
    });
    for format in ["png", "jpeg", "webp"] {
        let options: ExportOptions = serde_json::from_value(json!({
            "format":format, "quality":"preserve", "quality_value":100, "png":{},
        }))
        .unwrap();
        let bytes = editor.encode_export(options).unwrap();
        let decoded = image::load_from_memory(&bytes).unwrap().into_rgba8();
        assert_eq!(decoded.dimensions(), (6, 3));
        if format == "jpeg" {
            assert!(decoded.pixels().all(|pixel| pixel[3] == 255));
            assert!(
                decoded.get_pixel(5, 2).0[..3]
                    .iter()
                    .all(|channel| *channel >= 250)
            );
            assert!(decoded.get_pixel(0, 0)[0] < 100);
        } else {
            assert_eq!(decoded, expected);
        }
        let limited = ExportOptions {
            max_size_bytes: Some(0),
            ..options
        };
        assert!(editor.encode_export(limited).is_err());
        assert_eq!(serde_json::to_value(editor.snapshot()).unwrap(), before);
        assert!(Arc::ptr_eq(&pixels, &editor.pixels()));
    }
    assert!(!data.path().join("drafts").exists());
    assert_eq!(fs::read(directory.join("capture.png")).unwrap(), original);
    assert_eq!(fs::read(directory.join("metadata.json")).unwrap(), metadata);
    editor.execute(Request::Redo).unwrap();
    assert_eq!(editor.pixels().dimensions(), (9, 5));
}

#[test]
fn layer_json_commands_render_transactionally_and_restore_shared_assets() {
    let (data, id, original) = setup();
    let mut editor = open(data.path(), &id).unwrap();
    let layer = |id: &str, edit| {
        serde_json::from_value::<Request>(json!({
            "operation": "layer", "id": id, "edit": edit,
        }))
        .unwrap()
    };
    editor
        .execute(layer(
            "capture-background",
            json!({
                "action": "duplicate", "new_id": "copy",
            }),
        ))
        .unwrap();
    editor
        .execute(layer(
            "copy",
            json!({
                "action": "translate", "delta_x": -22, "delta_y": -23,
            }),
        ))
        .unwrap();
    assert_eq!(editor.pixels().get_pixel(3, 1).0, [31, 0, 19, 255]);
    editor
        .execute(layer(
            "copy",
            json!({"action": "visibility", "visible": false}),
        ))
        .unwrap();
    assert_eq!(editor.pixels().as_ref(), &original);
    editor.execute(Request::Undo).unwrap();
    assert_eq!(editor.pixels().get_pixel(3, 1).0, [31, 0, 19, 255]);
    assert!(editor.snapshot().can_redo);
    let frame = editor.pixels();
    // Locked no-ops and invalid edits cannot destroy redo or the retained frame.
    editor
        .execute(layer("capture-background", json!({"action": "delete"})))
        .unwrap();
    assert!(
        editor
            .execute(layer("copy", json!({"action": "opacity", "opacity": 101})))
            .is_err()
    );
    assert!(editor.snapshot().can_redo);
    assert!(Arc::ptr_eq(&frame, &editor.pixels()));
    editor
        .execute(layer("copy", json!({"action": "opacity", "opacity": 50})))
        .unwrap();
    let pixel = editor.pixels().get_pixel(3, 1).0;
    assert!((61..=63).contains(&pixel[0]) && (34..=36).contains(&pixel[1]));
    assert_eq!(&pixel[2..], &[19, 255]);
    assert!(!editor.snapshot().can_redo);
    editor
        .execute(Request::SaveDraft { updated_at_ms: 7 })
        .unwrap();
    let restored = open(data.path(), &id).unwrap();
    assert_eq!(restored.snapshot().document, editor.snapshot().document);
    assert_eq!(restored.pixels(), editor.pixels());
    assert_eq!(
        fs::read_dir(data.path().join("drafts").join(&id).join("assets"))
            .unwrap()
            .count(),
        1
    );
    let disk = image::open(data.path().join("history").join(id).join("capture.png"))
        .unwrap()
        .into_rgba8();
    assert_eq!(disk, original);
}

#[test]
fn annotation_control_projection_resolves_defaults_without_authoring_them() {
    let (data, id, _) = setup();
    let mut editor = open(data.path(), &id).unwrap();
    let mut document = editor.snapshot().document.clone();
    for (id, shape, width) in [("closed", "rectangle", 10.), ("open", "arrow", 3.)] {
        document.elements.push(
            serde_json::from_value(json!({
                "kind": "shape", "id": id, "shape": shape,
                "x": 0, "y": 0, "endX": 5, "endY": 2, "controls": [],
                "visible": false, "locked": true, "opacity": 100, "blendMode": "source-over",
                "style": {"color": "legacy-color", "strokeWidth": width, "futureStyle": 17}
            }))
            .unwrap(),
        );
    }
    document.elements.push(
        serde_json::from_value(json!({
            "kind": "path", "id": "path", "x": 0, "y": 0,
            "points": [{"x": 0, "y": 0}], "visible": false, "locked": true, "opacity": 100,
            "blendMode": "source-over",
            "style": {"color": "#abcdef", "strokeWidth": 3, "dropShadow": true,
                "dropShadowStyle": {"color": "invalid", "opacity": 130, "blur": -4,
                    "offsetX": -700, "offsetY": 650, "futureShadow": 19}}
        }))
        .unwrap(),
    );
    editor
        .execute(Request::Commit {
            document: document.clone(),
        })
        .unwrap();
    editor
        .execute(Request::SaveDraft { updated_at_ms: 1 })
        .unwrap();
    editor
        .execute(Request::ResizeCanvas {
            width: 8.,
            height: 4.,
        })
        .unwrap();
    editor.execute(Request::Undo).unwrap();
    let frame = editor.pixels();
    let manifest = data.path().join("drafts").join(&id).join("manifest.json");
    let saved = fs::read(&manifest).unwrap();
    let snapshot = serde_json::to_value(editor.snapshot()).unwrap();
    let controls = &snapshot["annotation_controls"];
    assert_eq!(controls.as_object().unwrap().len(), 3);
    assert!(controls.get("capture-background").is_none());
    assert_eq!(
        controls["closed"],
        json!({
            "closed": true, "color": "legacy-color", "fill": null, "strokeWidth": 10.,
            "strokeEnabled": true, "dropShadow": false,
            "dropShadowStyle": {"color": "#000000", "opacity": 45., "blur": 8.5,
                "offsetX": 0., "offsetY": 3.}
        })
    );
    assert_eq!(controls["open"]["closed"], false);
    assert_eq!(controls["open"]["dropShadowStyle"]["blur"], 6.);
    assert_eq!(controls["open"]["dropShadowStyle"]["offsetY"], 2.);
    assert_eq!(controls["path"]["closed"], false);
    assert_eq!(controls["path"]["dropShadow"], true);
    assert_eq!(
        controls["path"]["dropShadowStyle"],
        json!({
            "color": "#000000", "opacity": 100., "blur": 0.,
            "offsetX": -500., "offsetY": 500., "futureShadow": 19
        })
    );
    assert_eq!(editor.snapshot().document, &document);
    assert!(!editor.snapshot().unsaved_changes);
    assert!(editor.snapshot().can_redo);
    assert!(Arc::ptr_eq(&frame, &editor.pixels()));
    assert_eq!(fs::read(manifest).unwrap(), saved);
    let reopened = open(data.path(), &id).unwrap();
    assert_eq!(reopened.snapshot().document, &document);
    assert_eq!(
        serde_json::to_value(reopened.snapshot()).unwrap()["annotation_controls"],
        *controls
    );
    editor.execute(Request::Redo).unwrap();
    assert_eq!(editor.pixels().dimensions(), (8, 4));
}

#[test]
fn annotation_style_json_patches_locked_hidden_layers_and_preserves_history_and_drafts() {
    let (data, id, _) = setup();
    let mut editor = open(data.path(), &id).unwrap();
    let mut baseline = editor.snapshot().document.clone();
    baseline.width = 32.;
    baseline.height = 24.;
    baseline.background = None;
    baseline
        .extra
        .insert("futureDocument".into(), json!({"keep": "annotation-style"}));
    let Element::Image(background) = &mut baseline.elements[0] else {
        panic!()
    };
    background.base.visible = false;
    background
        .extra
        .insert("futureImage".into(), json!({"keep": true}));
    baseline.elements.extend([
        serde_json::from_value(json!({
            "kind": "shape",
            "id": "locked-shape",
            "x": 5,
            "y": 7,
            "endX": 15,
            "endY": 17,
            "controls": [],
            "shape": "rectangle",
            "locked": true,
            "visible": true,
            "opacity": 100,
            "blendMode": "source-over",
            "style": {
                "color": "#111111",
                "fill": "#cc2233",
                "strokeWidth": 4,
                "strokeEnabled": true,
                "dropShadow": false,
                "dropShadowStyle": {
                    "color": "#334455",
                    "opacity": 70,
                    "blur": 2,
                    "offsetX": 1,
                    "offsetY": -3,
                    "futureShadow": {"keep": [3, 1]}
                },
                "futureStyle": {"keep": 9}
            },
            "futureShape": ["preserve"]
        }))
        .unwrap(),
        serde_json::from_value(json!({
            "kind": "path",
            "id": "hidden-path",
            "x": 3,
            "y": 20,
            "points": [{"x": 3, "y": 20}, {"x": 23, "y": 20}],
            "locked": true,
            "visible": false,
            "opacity": 100,
            "blendMode": "source-over",
            "style": {
                "color": "#0000ff",
                "fill": "#abcdef",
                "strokeWidth": 3,
                "strokeEnabled": false,
                "dropShadow": false,
                "futurePathStyle": "keep"
            },
            "futurePath": {"keep": true}
        }))
        .unwrap(),
        serde_json::from_value(json!({
            "kind": "text",
            "id": "hidden-text",
            "x": 1,
            "y": 1,
            "locked": true,
            "visible": false,
            "opacity": 100,
            "blendMode": "source-over",
            "text": "unsupported target",
            "fontSize": 14,
            "width": 80,
            "fontFamily": "Inter",
            "bold": false,
            "italic": false,
            "align": "left",
            "color": "#ffffff",
            "background": null,
            "outlined": false,
            "roundedBackground": false,
            "futureText": "keep"
        }))
        .unwrap(),
    ]);
    editor
        .execute(Request::Commit { document: baseline })
        .unwrap();
    editor
        .execute(Request::SaveDraft { updated_at_ms: 1 })
        .unwrap();
    drop(editor);

    let mut editor = open(data.path(), &id).unwrap();
    let layer = |id: &str, edit| {
        serde_json::from_value::<Request>(json!({
            "operation": "layer", "id": id, "edit": edit,
        }))
        .unwrap()
    };
    editor
        .execute(layer(
            "locked-shape",
            json!({
                "action": "annotation_style",
                "patch": {
                    "color": "#123456",
                    "fill": "#e04090",
                    "strokeWidth": 8,
                    "strokeEnabled": false,
                    "dropShadowStyle": {
                        "color": "#20c060",
                        "opacity": 80,
                        "blur": 0,
                        "offsetX": 6
                    }
                }
            }),
        ))
        .unwrap();
    let enabled = editor.snapshot().document.clone();
    let Element::Shape(shape) = &enabled.elements[1] else {
        panic!()
    };
    assert!(shape.base.locked);
    assert_eq!(shape.style.color, "#123456");
    assert_eq!(shape.style.fill.as_deref(), Some("#e04090"));
    assert_eq!(shape.style.stroke_width, 8.);
    assert_eq!(shape.style.stroke_enabled, Some(false));
    assert_eq!(shape.style.drop_shadow, Some(true));
    assert_eq!(shape.style.extra["futureStyle"], json!({"keep": 9}));
    let shadow = shape.style.drop_shadow_style.as_ref().unwrap();
    assert_eq!(shadow.color, "#20c060");
    assert_eq!((shadow.opacity, shadow.blur), (80., 0.));
    assert_eq!((shadow.offset_x, shadow.offset_y), (6., -3.));
    assert_eq!(shadow.extra["futureShadow"], json!({"keep": [3, 1]}));
    assert_eq!(shape.extra["futureShape"], json!(["preserve"]));
    assert_eq!(editor.pixels().get_pixel(10, 12).0, [224, 64, 144, 255]);
    let shadow_pixel = editor.pixels().get_pixel(18, 9).0;
    for (actual, expected) in shadow_pixel[..3].iter().zip([32_u8, 192, 96]) {
        assert!(actual.abs_diff(expected) <= 1, "{shadow_pixel:?}");
    }
    assert_eq!(shadow_pixel[3], 204);

    editor
        .execute(layer(
            "locked-shape",
            json!({
                "action": "annotation_style",
                "patch": {"dropShadow": false}
            }),
        ))
        .unwrap();
    let Element::Shape(disabled) = &editor.snapshot().document.elements[1] else {
        panic!()
    };
    assert_eq!(disabled.style.drop_shadow, Some(false));
    assert_eq!(disabled.style.drop_shadow_style.as_ref(), Some(shadow));
    assert_eq!(editor.pixels().get_pixel(18, 9).0, [0, 0, 0, 0]);

    editor.execute(Request::Undo).unwrap();
    assert_eq!(editor.snapshot().document, &enabled);
    assert!(editor.snapshot().can_redo);
    let retained_frame = editor.pixels();
    let retained_snapshot = serde_json::to_value(editor.snapshot()).unwrap();
    let manifest_path = data.path().join("drafts").join(&id).join("manifest.json");
    let retained_manifest = fs::read(&manifest_path).unwrap();
    for request in [
        layer(
            "capture-background",
            json!({
                "action": "annotation_style",
                "patch": {"color": "#ffffff"}
            }),
        ),
        layer(
            "hidden-text",
            json!({
                "action": "annotation_style",
                "patch": {"strokeWidth": 12}
            }),
        ),
        layer(
            "locked-shape",
            json!({"action": "annotation_style", "patch": {}}),
        ),
        layer(
            "locked-shape",
            json!({
                "action": "annotation_style",
                "patch": {"dropShadowStyle": {}}
            }),
        ),
    ] {
        editor.execute(request).unwrap();
        assert_eq!(
            serde_json::to_value(editor.snapshot()).unwrap(),
            retained_snapshot
        );
        assert!(Arc::ptr_eq(&retained_frame, &editor.pixels()));
        assert!(editor.snapshot().can_redo);
        assert_eq!(fs::read(&manifest_path).unwrap(), retained_manifest);
    }
    assert!(
        editor
            .execute(layer(
                "missing",
                json!({
                    "action": "annotation_style",
                    "patch": {"color": "#ffffff"}
                }),
            ))
            .is_err()
    );
    assert!(
        editor
            .execute(layer(
                "locked-shape",
                json!({
                    "action": "annotation_style",
                    "patch": {"fill": "invalid-color"}
                }),
            ))
            .is_err()
    );
    assert_eq!(
        serde_json::to_value(editor.snapshot()).unwrap(),
        retained_snapshot
    );
    assert!(Arc::ptr_eq(&retained_frame, &editor.pixels()));
    assert!(editor.snapshot().can_redo);
    for stroke_width in [f64::NAN, f64::INFINITY] {
        assert!(
            editor
                .execute(Request::Layer {
                    id: "hidden-path".into(),
                    edit: LayerEdit::AnnotationStyle {
                        patch: AnnotationStylePatch {
                            stroke_width: Some(stroke_width),
                            ..Default::default()
                        },
                    },
                })
                .is_err()
        );
        assert_eq!(
            serde_json::to_value(editor.snapshot()).unwrap(),
            retained_snapshot
        );
        assert!(Arc::ptr_eq(&retained_frame, &editor.pixels()));
        assert!(editor.snapshot().can_redo);
        assert_eq!(fs::read(&manifest_path).unwrap(), retained_manifest);
    }

    editor.execute(Request::Redo).unwrap();
    editor
        .execute(layer(
            "locked-shape",
            json!({
                "action": "annotation_style",
                "patch": {"dropShadowStyle": {"offsetX": 7}}
            }),
        ))
        .unwrap();
    let Element::Shape(shape) = &editor.snapshot().document.elements[1] else {
        panic!()
    };
    let shadow = shape.style.drop_shadow_style.as_ref().unwrap();
    assert_eq!(shape.style.drop_shadow, Some(true));
    assert_eq!((shadow.offset_x, shadow.offset_y), (7., -3.));
    assert_eq!(shadow.color, "#20c060");
    assert_eq!(shadow.extra["futureShadow"], json!({"keep": [3, 1]}));

    editor
        .execute(layer(
            "hidden-path",
            json!({
                "action": "annotation_style",
                "patch": {
                    "color": "#ffaa00",
                    "fill": null,
                    "strokeWidth": 5,
                    "strokeEnabled": true,
                    "dropShadowStyle": {"offsetY": 4}
                }
            }),
        ))
        .unwrap();
    let Element::Path(path) = &editor.snapshot().document.elements[2] else {
        panic!()
    };
    assert!(path.base.locked && !path.base.visible);
    assert_eq!(path.style.color, "#ffaa00");
    assert_eq!(path.style.stroke_width, 5.);
    assert_eq!(path.style.fill.as_deref(), Some("#abcdef"));
    assert_eq!(path.style.stroke_enabled, Some(false));
    assert_eq!(path.style.drop_shadow, Some(true));
    let path_shadow = path.style.drop_shadow_style.as_ref().unwrap();
    assert_eq!(path_shadow.color, "#000000");
    assert_eq!((path_shadow.opacity, path_shadow.blur), (45., 6.));
    assert_eq!((path_shadow.offset_x, path_shadow.offset_y), (0., 4.));
    assert_eq!(path.style.extra["futurePathStyle"], "keep");
    assert_eq!(path.extra["futurePath"], json!({"keep": true}));

    editor
        .execute(layer(
            "hidden-path",
            json!({"action": "visibility", "visible": true}),
        ))
        .unwrap();
    let path_pixel = editor.pixels().get_pixel(12, 20).0;
    assert!(path_pixel[0] >= 250 && path_pixel[1] >= 165 && path_pixel[2] <= 5);
    let final_document = editor.snapshot().document.clone();
    let final_pixels = editor.pixels();
    editor
        .execute(Request::SaveDraft { updated_at_ms: 2 })
        .unwrap();
    drop(editor);

    let restored = open(data.path(), &id).unwrap();
    assert_eq!(restored.snapshot().document, &final_document);
    assert_eq!(restored.pixels(), final_pixels);
    assert_eq!(
        restored.snapshot().document.extra["futureDocument"],
        json!({"keep": "annotation-style"})
    );
}

#[test]
fn image_transform_json_preserves_center_pixels_history_and_draft_data() {
    let (data, id, original) = setup();
    let mut editor = open(data.path(), &id).unwrap();
    editor
        .execute(image_transform("capture-background", "rotate-clockwise"))
        .unwrap();
    assert_eq!(editor.pixels().dimensions(), (3, 7));
    assert_eq!(
        (
            editor.snapshot().document.width,
            editor.snapshot().document.height
        ),
        (3., 7.)
    );
    for y in 0..7 {
        for x in 0..3 {
            assert_eq!(
                editor.pixels().get_pixel(x, y),
                original.get_pixel(y, 2 - x),
                "fresh-photo pixel ({x}, {y})"
            );
        }
    }
    editor.execute(Request::Undo).unwrap();
    assert_eq!(editor.pixels().as_ref(), &original);

    let mut document = editor.snapshot().document.clone();
    document.width = 11.;
    document.height = 11.;
    document.background = None;
    document
        .extra
        .insert("futureDocument".into(), json!({"keep": [3, 1]}));
    let Element::Image(image) = &mut document.elements[0] else {
        panic!()
    };
    image.base.x = 2.;
    image.base.y = 4.;
    image
        .extra
        .insert("futureImage".into(), json!({"keep": true}));
    document.elements.push(
        serde_json::from_value(json!({
            "kind": "text",
            "id": "locked-note",
            "x": -3.5,
            "y": 8.25,
            "locked": true,
            "visible": false,
            "opacity": 100,
            "blendMode": "source-over",
            "text": "future note",
            "fontSize": 17,
            "width": 91,
            "fontFamily": "Inter",
            "bold": false,
            "italic": false,
            "align": "left",
            "color": "#123456",
            "background": null,
            "outlined": false,
            "roundedBackground": false,
            "futureText": {"keep": "too"}
        }))
        .unwrap(),
    );
    editor.execute(Request::Commit { document }).unwrap();
    editor
        .execute(Request::SaveDraft { updated_at_ms: 1 })
        .unwrap();
    drop(editor);

    // Reopening makes the prepared asymmetric document the persisted baseline,
    // so history assertions below measure only layer-transform commands.
    let mut editor = open(data.path(), &id).unwrap();
    let baseline = editor.snapshot().document.clone();
    assert!(!editor.snapshot().can_undo);

    // Shipping permits transforms on the locked capture background.
    editor
        .execute(image_transform("capture-background", "rotate-clockwise"))
        .unwrap();
    let rotated = editor.snapshot().document.clone();
    let Element::Image(image) = &rotated.elements[0] else {
        panic!()
    };
    assert!(image.base.locked);
    assert_eq!(image.orientation, Some(ImageOrientation::Rotate90));
    assert_eq!((image.base.x, image.base.y), (4., 2.));
    assert_eq!((image.width, image.height), (3., 7.));
    assert_eq!(
        (
            image.base.x + image.width / 2.,
            image.base.y + image.height / 2.
        ),
        (5.5, 5.5)
    );
    assert_eq!(image.extra["futureImage"], json!({"keep": true}));
    assert_eq!(rotated.extra["futureDocument"], json!({"keep": [3, 1]}));
    assert_eq!(rotated.elements[1], baseline.elements[1]);
    let pixels = editor.pixels();
    assert_eq!(pixels.get_pixel(3, 2).0, [0, 0, 0, 0]);
    for y in 0..7 {
        for x in 0..3 {
            assert_eq!(
                pixels.get_pixel(x + 4, y + 2),
                original.get_pixel(y, 2 - x),
                "rotated pixel ({x}, {y})"
            );
        }
    }

    editor.execute(Request::Undo).unwrap();
    assert_eq!(editor.snapshot().document, &baseline);
    assert!(editor.snapshot().can_redo);
    let baseline_frame = editor.pixels();
    // Shipping ignores image-transform actions for non-image layers. The exact
    // no-op and a rejected missing-layer command must retain redo and its frame.
    editor
        .execute(image_transform("locked-note", "flip-horizontal"))
        .unwrap();
    assert!(editor.snapshot().can_redo);
    assert!(Arc::ptr_eq(&baseline_frame, &editor.pixels()));
    assert!(
        editor
            .execute(image_transform("missing-layer", "flip-horizontal"))
            .is_err()
    );
    assert!(editor.snapshot().can_redo);
    assert!(Arc::ptr_eq(&baseline_frame, &editor.pixels()));

    editor.execute(Request::Redo).unwrap();
    assert_eq!(editor.snapshot().document, &rotated);
    assert_eq!(editor.pixels(), pixels);
    editor
        .execute(Request::SaveDraft { updated_at_ms: 2 })
        .unwrap();
    drop(editor);

    let mut restored = open(data.path(), &id).unwrap();
    assert_eq!(restored.snapshot().document, &rotated);
    assert_eq!(restored.pixels(), pixels);
    assert_eq!(
        restored.snapshot().document.extra["futureDocument"],
        json!({"keep": [3, 1]})
    );
    // Hidden image layers remain transformable, but continue contributing no
    // pixels. Left-composition turns flip-horizontal × rotate-90 into transpose.
    restored
        .execute(
            serde_json::from_value(json!({
                "operation": "layer",
                "id": "capture-background",
                "edit": {"action": "visibility", "visible": false},
            }))
            .unwrap(),
        )
        .unwrap();
    restored
        .execute(image_transform("capture-background", "flip-horizontal"))
        .unwrap();
    let Element::Image(hidden) = &restored.snapshot().document.elements[0] else {
        panic!()
    };
    assert_eq!(hidden.orientation, Some(ImageOrientation::Transpose));
    assert!(
        restored
            .pixels()
            .pixels()
            .all(|pixel| pixel.0 == [0, 0, 0, 0])
    );
}

#[test]
fn closed_shape_json_creation_renders_and_rolls_back_history_before_draft_reopen() {
    let (data, id, original) = setup();
    let mut editor = open(data.path(), &id).unwrap();
    let mut document = editor.snapshot().document.clone();
    document.width = 12.;
    document.height = 8.;
    document.background = None;
    document
        .extra
        .insert("futureDocument".into(), json!({"keep": "shape-create"}));
    let Element::Image(image) = &mut document.elements[0] else {
        panic!()
    };
    image
        .extra
        .insert("futureImage".into(), json!({"keep": [7, 3]}));
    editor.execute(Request::Commit { document }).unwrap();
    editor
        .execute(Request::SaveDraft { updated_at_ms: 1 })
        .unwrap();
    drop(editor);

    // Reopen so creation is one transaction on top of a persisted baseline.
    let mut editor = open(data.path(), &id).unwrap();
    let baseline = editor.snapshot().document.clone();
    let baseline_frame = editor.pixels();
    let rectangle: Request = serde_json::from_value(json!({
        "operation": "create_closed_shape",
        "shape": "rectangle",
        "start": {"x": 1.25, "y": 1.25},
        "end": {"x": 5.75, "y": 5.75}
    }))
    .unwrap();
    editor.execute(rectangle).unwrap();
    let rectangle_document = editor.snapshot().document.clone();
    let Element::Shape(rectangle) = rectangle_document.elements.last().unwrap() else {
        panic!()
    };
    assert!(!rectangle.base.id.is_empty());
    assert_ne!(rectangle.base.id, "capture-background");
    assert_eq!(rectangle.shape, "rectangle");
    assert_eq!((rectangle.base.x, rectangle.base.y), (1.25, 1.25));
    assert_eq!((rectangle.end_x, rectangle.end_y), (5.75, 5.75));
    assert!(!rectangle.base.locked && rectangle.base.visible);
    assert_eq!(rectangle.base.opacity, 100.);
    assert_eq!(rectangle.base.blend_mode, "source-over");
    assert_eq!(rectangle.style.color, "#ff3b5c");
    assert_eq!(rectangle.style.fill.as_deref(), Some("#ff3b5c"));
    assert_eq!(rectangle.style.stroke_width, 8.);
    assert_eq!(rectangle.style.stroke_enabled, Some(false));
    assert_eq!(rectangle.style.drop_shadow, Some(false));
    assert_eq!(editor.pixels().get_pixel(3, 3).0, [255, 59, 92, 255]);
    assert_eq!(editor.pixels().get_pixel(10, 7).0, [0, 0, 0, 0]);

    editor.execute(Request::Undo).unwrap();
    assert_eq!(editor.snapshot().document, &baseline);
    assert_eq!(editor.pixels(), baseline_frame);
    assert!(editor.snapshot().can_redo);
    let before_failure = serde_json::to_value(editor.snapshot()).unwrap();
    let retained_frame = editor.pixels();
    let degenerate: Request = serde_json::from_value(json!({
        "operation": "create_closed_shape",
        "shape": "rectangle",
        "start": {"x": 4, "y": 2},
        "end": {"x": 4, "y": 6}
    }))
    .unwrap();
    assert!(editor.execute(degenerate).is_err());
    assert_eq!(
        serde_json::to_value(editor.snapshot()).unwrap(),
        before_failure
    );
    assert!(Arc::ptr_eq(&retained_frame, &editor.pixels()));
    assert!(editor.snapshot().can_redo);
    let invalid_fill: Request = serde_json::from_value(json!({
        "operation": "create_closed_shape",
        "shape": "ellipse",
        "start": {"x": 7, "y": 1},
        "end": {"x": 11, "y": 6},
        "style": {
            "color": "#00ff00",
            "fill": "not-a-color",
            "strokeWidth": 3,
            "strokeEnabled": false,
            "dropShadow": false
        }
    }))
    .unwrap();
    assert!(editor.execute(invalid_fill).is_err());
    assert_eq!(
        serde_json::to_value(editor.snapshot()).unwrap(),
        before_failure
    );
    assert!(Arc::ptr_eq(&retained_frame, &editor.pixels()));
    assert!(editor.snapshot().can_redo);

    editor.execute(Request::Redo).unwrap();
    assert_eq!(editor.snapshot().document, &rectangle_document);
    // A custom ellipse fully outside the negative edges grows the canvas and
    // translates every existing layer before rendering the new pixels.
    let ellipse: Request = serde_json::from_value(json!({
        "operation": "create_closed_shape",
        "shape": "ellipse",
        "start": {"x": -20, "y": -15},
        "end": {"x": -16, "y": -11},
        "style": {
            "color": "#00ff00",
            "fill": "#00ff00",
            "strokeWidth": 3,
            "strokeEnabled": false,
            "dropShadow": false,
            "futureStyle": {"keep": true}
        },
        "opacity": 100
    }))
    .unwrap();
    editor.execute(ellipse).unwrap();
    let final_document = editor.snapshot().document.clone();
    assert_eq!((final_document.width, final_document.height), (35., 26.));
    assert_eq!(
        (
            final_document.elements[0].base().x,
            final_document.elements[0].base().y
        ),
        (23., 18.)
    );
    let Element::Shape(ellipse) = final_document.elements.last().unwrap() else {
        panic!()
    };
    assert_eq!(ellipse.shape, "ellipse");
    assert_eq!((ellipse.base.x, ellipse.base.y), (3., 3.));
    assert_eq!((ellipse.end_x, ellipse.end_y), (7., 7.));
    assert_eq!(ellipse.style.extra["futureStyle"], json!({"keep": true}));
    assert_eq!(editor.pixels().get_pixel(5, 5).0, [0, 255, 0, 255]);
    assert_eq!(editor.pixels().get_pixel(23, 18), original.get_pixel(0, 0));
    assert_eq!(
        final_document.extra["futureDocument"],
        json!({"keep": "shape-create"})
    );
    let Element::Image(image) = &final_document.elements[0] else {
        panic!()
    };
    assert_eq!(image.extra["futureImage"], json!({"keep": [7, 3]}));

    editor
        .execute(Request::SaveDraft { updated_at_ms: 2 })
        .unwrap();
    let final_pixels = editor.pixels();
    drop(editor);
    let restored = open(data.path(), &id).unwrap();
    assert_eq!(restored.snapshot().document, &final_document);
    assert_eq!(restored.pixels(), final_pixels);
}

#[test]
fn open_shape_json_creation_renders_expands_and_restores_draft_history() {
    let (data, id, _) = setup();
    let mut editor = open(data.path(), &id).unwrap();
    let mut document = editor.snapshot().document.clone();
    document.width = 40.;
    document.height = 30.;
    document.background = None;
    document
        .extra
        .insert("futureDocument".into(), json!({"keep": "open-create"}));
    let Element::Image(background) = &mut document.elements[0] else {
        panic!()
    };
    background.base.visible = false;
    background
        .extra
        .insert("futureImage".into(), json!({"keep": [4, 9]}));
    editor.execute(Request::Commit { document }).unwrap();
    editor
        .execute(Request::SaveDraft { updated_at_ms: 1 })
        .unwrap();
    let manifest_path = data.path().join("drafts").join(&id).join("manifest.json");
    let baseline_manifest = fs::read(&manifest_path).unwrap();
    let baseline = editor.snapshot().document.clone();

    // Shipping keeps even a click-only line: round caps render it as a dot.
    let dot: Request = serde_json::from_value(json!({
        "operation": "create_open_shape",
        "shape": "line",
        "start": {"x": 5.25, "y": 4.75},
        "end": {"x": 5.25, "y": 4.75}
    }))
    .unwrap();
    editor.execute(dot).unwrap();
    let line_document = editor.snapshot().document.clone();
    let Element::Shape(line) = line_document.elements.last().unwrap() else {
        panic!()
    };
    assert!(!line.base.id.is_empty());
    assert_eq!(line.shape, "line");
    assert_eq!((line.base.x, line.base.y), (5.25, 4.75));
    assert_eq!((line.end_x, line.end_y), (5.25, 4.75));
    assert!(line.controls.is_empty());
    assert_eq!(line.style.fill, None);
    assert!(!line.base.locked && line.base.visible);
    assert_eq!(line.base.opacity, 100.);
    assert_eq!(line.base.blend_mode, "source-over");
    assert_eq!(editor.pixels().get_pixel(5, 5).0, [255, 59, 92, 255]);

    editor.execute(Request::Undo).unwrap();
    assert_eq!(editor.snapshot().document, &baseline);
    assert!(editor.snapshot().can_redo);
    let before_failure = serde_json::to_value(editor.snapshot()).unwrap();
    let retained_frame = editor.pixels();
    let short_arrow: Request = serde_json::from_value(json!({
        "operation": "create_open_shape",
        "shape": "arrow",
        "start": {"x": 7.25, "y": 9.5},
        "end": {"x": 8.749, "y": 9.5}
    }))
    .unwrap();
    assert!(editor.execute(short_arrow).is_err());
    let invalid_color: Request = serde_json::from_value(json!({
        "operation": "create_open_shape",
        "shape": "line",
        "start": {"x": 2, "y": 2},
        "end": {"x": 9, "y": 6},
        "style": {
            "color": "not-a-color",
            "fill": "#00ff00",
            "strokeWidth": 4,
            "strokeEnabled": true,
            "dropShadow": false
        }
    }))
    .unwrap();
    assert!(editor.execute(invalid_color).is_err());
    assert_eq!(
        serde_json::to_value(editor.snapshot()).unwrap(),
        before_failure
    );
    assert!(Arc::ptr_eq(&retained_frame, &editor.pixels()));
    assert!(editor.snapshot().can_redo);
    assert_eq!(fs::read(&manifest_path).unwrap(), baseline_manifest);

    editor.execute(Request::Redo).unwrap();
    assert_eq!(editor.snapshot().document, &line_document);
    let outside_arrow: Request = serde_json::from_value(json!({
        "operation": "create_open_shape",
        "shape": "arrow",
        "start": {"x": -55.5, "y": -38.25},
        "end": {"x": -39.25, "y": -29.75},
        "style": {
            "color": "#2277dd",
            "fill": "#00ff00",
            "strokeWidth": 6,
            "strokeEnabled": true,
            "dropShadow": true,
            "dropShadowStyle": {
                "color": "#112233",
                "opacity": 80,
                "blur": 4,
                "offsetX": 9,
                "offsetY": -2
            },
            "futureStyle": {"preserve": "open"}
        },
        "opacity": 62.5
    }))
    .unwrap();
    editor.execute(outside_arrow).unwrap();
    let final_document = editor.snapshot().document.clone();
    assert_eq!((final_document.width, final_document.height), (114., 87.));
    assert_eq!(
        (
            final_document.elements[0].base().x,
            final_document.elements[0].base().y
        ),
        (74., 57.)
    );
    let Element::Shape(translated_line) = &final_document.elements[1] else {
        panic!()
    };
    assert_eq!(
        (translated_line.base.x, translated_line.base.y),
        (79.25, 61.75)
    );
    let Element::Shape(arrow) = final_document.elements.last().unwrap() else {
        panic!()
    };
    assert_eq!(arrow.shape, "arrow");
    assert_eq!((arrow.base.x, arrow.base.y), (18.5, 18.75));
    assert_eq!((arrow.end_x, arrow.end_y), (34.75, 27.25));
    assert_eq!(arrow.style.fill, None);
    assert_eq!(
        arrow.style.extra["futureStyle"],
        json!({"preserve": "open"})
    );
    assert!(
        editor
            .pixels()
            .pixels()
            .any(|pixel| pixel.0 == [34, 119, 221, 219])
    );
    assert!(
        editor
            .pixels()
            .pixels()
            .any(|pixel| pixel[3] > 0 && pixel[3] < 159)
    );
    assert_eq!(
        final_document.extra["futureDocument"],
        json!({"keep": "open-create"})
    );
    let Element::Image(background) = &final_document.elements[0] else {
        panic!()
    };
    assert_eq!(background.extra["futureImage"], json!({"keep": [4, 9]}));

    let final_pixels = editor.pixels();
    editor.execute(Request::Undo).unwrap();
    assert_eq!(editor.snapshot().document, &line_document);
    editor.execute(Request::Redo).unwrap();
    assert_eq!(editor.snapshot().document, &final_document);
    assert_eq!(editor.pixels(), final_pixels);
    editor
        .execute(Request::SaveDraft { updated_at_ms: 2 })
        .unwrap();
    drop(editor);
    let restored = open(data.path(), &id).unwrap();
    assert_eq!(restored.snapshot().document, &final_document);
    assert_eq!(restored.pixels(), final_pixels);
}

#[test]
fn freehand_json_creation_renders_expands_and_restores_draft_history() {
    let (data, id, _) = setup();
    let mut editor = open(data.path(), &id).unwrap();
    let mut document = editor.snapshot().document.clone();
    document.width = 40.;
    document.height = 30.;
    document.background = None;
    document
        .extra
        .insert("futureDocument".into(), json!({"keep": "freehand-create"}));
    let Element::Image(background) = &mut document.elements[0] else {
        panic!()
    };
    background.base.visible = false;
    background
        .extra
        .insert("futureImage".into(), json!({"keep": [5, 2]}));
    editor.execute(Request::Commit { document }).unwrap();
    editor
        .execute(Request::SaveDraft { updated_at_ms: 1 })
        .unwrap();
    let manifest_path = data.path().join("drafts").join(&id).join("manifest.json");
    let baseline_manifest = fs::read(&manifest_path).unwrap();
    let baseline = editor.snapshot().document.clone();

    let dot: Request = serde_json::from_value(json!({
        "operation": "create_freehand_path",
        "points": [{"x": 5.25, "y": 4.75}]
    }))
    .unwrap();
    editor.execute(dot).unwrap();
    let dot_document = editor.snapshot().document.clone();
    let Element::Path(path) = dot_document.elements.last().unwrap() else {
        panic!()
    };
    assert!(!path.base.id.is_empty());
    assert_eq!((path.base.x, path.base.y), (5.25, 4.75));
    assert_eq!(path.points, vec![Point { x: 5.25, y: 4.75 }]);
    assert_eq!(path.style.fill, None);
    assert!(!path.base.locked && path.base.visible);
    assert_eq!(path.base.opacity, 100.);
    assert_eq!(path.base.blend_mode, "source-over");
    assert_eq!(editor.pixels().get_pixel(5, 5).0, [255, 59, 92, 255]);

    editor.execute(Request::Undo).unwrap();
    assert_eq!(editor.snapshot().document, &baseline);
    assert!(editor.snapshot().can_redo);
    let before_failure = serde_json::to_value(editor.snapshot()).unwrap();
    let retained_frame = editor.pixels();
    for invalid in [
        json!({"operation": "create_freehand_path", "points": []}),
        json!({
            "operation": "create_freehand_path",
            "points": [{"x": 2, "y": 2}],
            "style": {
                "color": "not-a-color",
                "fill": null,
                "strokeWidth": 4,
                "strokeEnabled": true,
                "dropShadow": false
            }
        }),
    ] {
        let request: Request = serde_json::from_value(invalid).unwrap();
        assert!(editor.execute(request).is_err());
    }
    assert_eq!(
        serde_json::to_value(editor.snapshot()).unwrap(),
        before_failure
    );
    assert!(Arc::ptr_eq(&retained_frame, &editor.pixels()));
    assert!(editor.snapshot().can_redo);
    assert_eq!(fs::read(&manifest_path).unwrap(), baseline_manifest);

    editor.execute(Request::Redo).unwrap();
    assert_eq!(editor.snapshot().document, &dot_document);
    let outside: Request = serde_json::from_value(json!({
        "operation": "create_freehand_path",
        "points": [
            {"x": -58.5, "y": -39.25},
            {"x": -48.75, "y": -32.5},
            {"x": -48.75, "y": -32.5},
            {"x": -42.125, "y": -36.75}
        ],
        "style": {
            "color": "#2277dd",
            "fill": "#00ff00",
            "strokeWidth": 3.5,
            "strokeEnabled": true,
            "dropShadow": true,
            "dropShadowStyle": {
                "color": "#112233",
                "opacity": 80,
                "blur": 4,
                "offsetX": 9,
                "offsetY": -2
            },
            "futureStyle": {"preserve": "freehand"}
        },
        "opacity": 62.5
    }))
    .unwrap();
    editor.execute(outside).unwrap();
    let final_document = editor.snapshot().document.clone();
    assert_eq!((final_document.width, final_document.height), (120., 91.));
    assert_eq!(
        (
            final_document.elements[0].base().x,
            final_document.elements[0].base().y
        ),
        (80., 61.)
    );
    let Element::Path(translated_dot) = &final_document.elements[1] else {
        panic!()
    };
    assert_eq!(
        (translated_dot.base.x, translated_dot.base.y),
        (85.25, 65.75)
    );
    assert_eq!(translated_dot.points, vec![Point { x: 85.25, y: 65.75 }]);
    let Element::Path(path) = final_document.elements.last().unwrap() else {
        panic!()
    };
    assert_eq!((path.base.x, path.base.y), (21.5, 21.75));
    assert_eq!(
        path.points,
        vec![
            Point { x: 21.5, y: 21.75 },
            Point { x: 31.25, y: 28.5 },
            Point { x: 31.25, y: 28.5 },
            Point {
                x: 37.875,
                y: 24.25
            },
        ]
    );
    assert_eq!(path.style.fill, None);
    assert_eq!(
        path.style.extra["futureStyle"],
        json!({"preserve": "freehand"})
    );
    assert!(
        editor
            .pixels()
            .pixels()
            .any(|pixel| { pixel[0] == 34 && pixel[1] == 119 && pixel[2] == 221 && pixel[3] > 0 })
    );
    assert!(
        editor
            .pixels()
            .pixels()
            .any(|pixel| { pixel[0] == 17 && pixel[1] == 34 && pixel[2] == 51 && pixel[3] > 0 })
    );
    assert_eq!(
        final_document.extra["futureDocument"],
        json!({"keep": "freehand-create"})
    );
    let Element::Image(background) = &final_document.elements[0] else {
        panic!()
    };
    assert_eq!(background.extra["futureImage"], json!({"keep": [5, 2]}));

    let final_pixels = editor.pixels();
    editor.execute(Request::Undo).unwrap();
    assert_eq!(editor.snapshot().document, &dot_document);
    editor.execute(Request::Redo).unwrap();
    assert_eq!(editor.snapshot().document, &final_document);
    assert_eq!(editor.pixels(), final_pixels);
    editor
        .execute(Request::SaveDraft { updated_at_ms: 2 })
        .unwrap();
    drop(editor);
    let restored = open(data.path(), &id).unwrap();
    assert_eq!(restored.snapshot().document, &final_document);
    assert_eq!(restored.pixels(), final_pixels);
}

#[test]
fn image_import_is_atomic_undoable_and_draft_owned() {
    let (data, id, original) = setup();
    let mut editor = open(data.path(), &id).unwrap();
    let mut baseline = editor.snapshot().document.clone();
    baseline
        .extra
        .insert("futureDocument".into(), json!({"keep": [9, 2]}));
    let Element::Image(background) = &mut baseline.elements[0] else {
        panic!()
    };
    background
        .extra
        .insert("futureImage".into(), json!({"keep": true}));
    editor
        .execute(Request::Commit {
            document: baseline.clone(),
        })
        .unwrap();

    let imported = RgbaImage::from_fn(3, 2, |x, y| {
        Rgba([181 + x as u8 * 19, 23 + y as u8 * 61, 97, 255])
    });
    let layer_id = editor
        .import_image(ImportImage {
            pixels: imported.clone(),
            name: "asymmetric.png".into(),
            selected_id: Some("capture-background".into()),
            point: None,
        })
        .unwrap();
    let imported_document = editor.snapshot().document.clone();
    assert_eq!(
        (imported_document.width, imported_document.height),
        (7., 5.)
    );
    assert_eq!(imported_document.extra, baseline.extra);
    assert_eq!(imported_document.elements.len(), 2);
    let Element::Image(layer) = &imported_document.elements[1] else {
        panic!()
    };
    assert_eq!(layer.base.id, layer_id);
    assert_eq!((layer.base.x, layer.base.y), (2., 3.));
    assert_eq!((layer.width, layer.height), (3., 2.));
    assert_eq!((layer.natural_width, layer.natural_height), (3., 2.));
    assert!(!layer.base.locked && layer.base.visible);
    assert_eq!(layer.base.opacity, 100.);
    assert_eq!(layer.base.blend_mode, "source-over");
    assert_eq!(layer.source, "imported");
    assert!(layer.src.starts_with("draft-asset:"));
    assert!(matches!(layer.original_src, OptionalNullable::Null));
    assert_eq!(layer.source_artifact_id, None);
    assert_eq!(layer.name, "asymmetric.png");
    assert_eq!(
        imported_document.elements[0], baseline.elements[0],
        "positive-edge expansion must not translate existing layers"
    );
    for y in 0..2 {
        for x in 0..3 {
            assert_eq!(
                editor.pixels().get_pixel(x + 2, y + 3),
                imported.get_pixel(x, y)
            );
        }
    }
    assert_eq!(editor.pixels().get_pixel(6, 4).0, [247, 247, 245, 255]);

    let imported_frame = editor.pixels();
    editor.execute(Request::Undo).unwrap();
    assert_eq!(editor.snapshot().document, &baseline);
    assert_eq!(editor.pixels().as_ref(), &original);
    assert!(editor.snapshot().can_redo);

    // Invalid pixels and coordinates fail before changing history, retained
    // frames/assets, or the filesystem. The prior imported asset remains owned
    // so redo can render it without decoding or host access.
    let before_failure = serde_json::to_value(editor.snapshot()).unwrap();
    let frame_before_failure = editor.pixels();
    assert!(
        editor
            .import_image(ImportImage {
                pixels: RgbaImage::new(0, 2),
                name: "empty.png".into(),
                selected_id: None,
                point: None,
            })
            .is_err()
    );
    assert!(
        editor
            .import_image(ImportImage {
                pixels: RgbaImage::new(1, 1),
                name: "bad-point.png".into(),
                selected_id: None,
                point: Some(Point {
                    x: f64::NAN,
                    y: -3.5,
                }),
            })
            .is_err()
    );
    assert_eq!(
        serde_json::to_value(editor.snapshot()).unwrap(),
        before_failure
    );
    assert!(Arc::ptr_eq(&frame_before_failure, &editor.pixels()));
    assert!(!data.path().join("drafts").exists());

    editor.execute(Request::Redo).unwrap();
    assert_eq!(editor.snapshot().document, &imported_document);
    assert_eq!(editor.pixels(), imported_frame);
    editor
        .execute(Request::SaveDraft { updated_at_ms: 44 })
        .unwrap();
    let assets = data.path().join("drafts").join(&id).join("assets");
    assert_eq!(fs::read_dir(&assets).unwrap().count(), 2);
    drop(editor);

    let restored = open(data.path(), &id).unwrap();
    assert_eq!(restored.snapshot().document, &imported_document);
    assert_eq!(restored.pixels(), imported_frame);
    assert!(!restored.snapshot().can_undo);
    assert!(!restored.snapshot().can_redo);
    assert_eq!(
        restored.snapshot().document.extra["futureDocument"],
        json!({"keep": [9, 2]})
    );
    let Element::Image(restored_background) = &restored.snapshot().document.elements[0] else {
        panic!()
    };
    assert_eq!(
        restored_background.extra["futureImage"],
        json!({"keep": true})
    );
}

#[test]
fn failed_import_render_preserves_redo_frame_and_files() {
    let (data, id, _) = setup();
    let mut editor = open(data.path(), &id).unwrap();
    let mut off_canvas = editor.snapshot().document.clone();
    let Element::Image(target) = &mut off_canvas.elements[0] else {
        panic!()
    };
    target.base.x = 16_382.;
    editor
        .execute(Request::Commit {
            document: off_canvas.clone(),
        })
        .unwrap();
    let mut branch = off_canvas;
    branch
        .extra
        .insert("futureBranch".into(), json!("redo survives"));
    editor
        .execute(Request::Commit { document: branch })
        .unwrap();
    editor.execute(Request::Undo).unwrap();
    assert!(editor.snapshot().can_redo);

    let before = serde_json::to_value(editor.snapshot()).unwrap();
    let frame = editor.pixels();
    let error = editor
        .import_image(ImportImage {
            pixels: RgbaImage::from_pixel(1, 1, Rgba([17, 91, 203, 255])),
            name: "would-overflow.png".into(),
            selected_id: Some("capture-background".into()),
            point: Some(Point { x: 16_383., y: 1. }),
        })
        .unwrap_err();
    assert!(error.contains("rendering is limited"), "{error}");
    assert_eq!(serde_json::to_value(editor.snapshot()).unwrap(), before);
    assert!(editor.snapshot().can_redo);
    assert!(Arc::ptr_eq(&frame, &editor.pixels()));
    assert!(!data.path().join("drafts").exists());
    editor.execute(Request::Redo).unwrap();
    assert_eq!(
        editor.snapshot().document.extra["futureBranch"],
        json!("redo survives")
    );
}

#[test]
fn crop_undo_branch_failure_and_retained_frames_have_transactional_semantics() {
    let (data, id, original) = setup();
    let mut editor = open(data.path(), &id).unwrap();
    let initial = editor.pixels();
    assert_eq!(initial.as_ref(), &original);
    assert!(!editor.snapshot().unsaved_changes);
    editor.execute(crop()).unwrap();
    let cropped = editor.pixels();
    assert_eq!(cropped.dimensions(), (4, 2));
    assert_eq!(cropped.get_pixel(0, 0).0, [62, 71, 19, 255]);
    assert_eq!(cropped.get_pixel(3, 1).0, [155, 142, 19, 255]);
    assert!(editor.snapshot().can_undo && editor.snapshot().unsaved_changes);
    editor.execute(Request::Undo).unwrap();
    assert_eq!(editor.pixels().as_ref(), &original);
    assert!(!editor.snapshot().unsaved_changes);
    assert!(editor.snapshot().can_redo);
    let frame = editor.pixels();
    // A no-op must not destroy redo or replace the shared frame.
    editor
        .execute(Request::ResizeCanvas {
            width: 7.,
            height: 3.,
        })
        .unwrap();
    assert!(editor.snapshot().can_redo);
    assert!(Arc::ptr_eq(&frame, &editor.pixels()));
    // A rejected render must also leave redo and current pixels untouched.
    assert!(
        editor
            .execute(Request::ResizeCanvas {
                width: 16385.,
                height: 3.
            })
            .is_err()
    );
    assert!(editor.snapshot().can_redo);
    assert!(Arc::ptr_eq(&frame, &editor.pixels()));
    editor.execute(Request::Redo).unwrap();
    assert_eq!(editor.pixels(), cropped);
    editor.execute(Request::Undo).unwrap();
    editor
        .execute(Request::ResizeCanvas {
            width: 9.,
            height: 4.,
        })
        .unwrap();
    assert!(!editor.snapshot().can_redo);
    assert_eq!(editor.pixels().get_pixel(8, 3).0, [247, 247, 245, 255]);
    drop(editor);
    assert_eq!(initial.as_ref(), &original);
    assert_eq!(cropped.get_pixel(0, 0).0, [62, 71, 19, 255]);
}

#[test]
fn save_restore_discard_preserves_source_export_and_unknown_document_fields() {
    let (data, id, original) = setup();
    let capture = data.path().join("history").join(&id).join("capture.png");
    let original_bytes = fs::read(&capture).unwrap();
    let export = data.path().join("export.png");
    fs::write(&export, &original_bytes).unwrap();
    let mut editor = open(data.path(), &id).unwrap();
    let mut document = editor.snapshot().document.clone();
    document
        .extra
        .insert("futureSetting".into(), json!({"keep":true}));
    let Element::Image(image) = &mut document.elements[0] else {
        panic!()
    };
    image.original_src = OptionalNullable::Value(image.src.clone());
    image.extra.insert("futureLayer".into(), json!([2, 7]));
    editor.execute(Request::Commit { document }).unwrap();
    editor.execute(crop()).unwrap();
    editor
        .execute(Request::SaveDraft {
            updated_at_ms: 123456,
        })
        .unwrap();
    assert!(!editor.snapshot().unsaved_changes);
    assert!(editor.snapshot().has_draft);
    let saved = editor.snapshot().document.clone();
    let pixels = editor.pixels();
    drop(editor);
    let mut restored = open(data.path(), &id).unwrap();
    assert_eq!(restored.snapshot().document, &saved);
    assert_eq!(restored.pixels(), pixels);
    assert!(restored.snapshot().has_draft);
    assert!(!restored.snapshot().unsaved_changes);
    assert!(!restored.snapshot().can_undo);
    let folder = data.path().join("drafts").join(&id);
    assert_eq!(fs::read_dir(folder.join("assets")).unwrap().count(), 1);
    restored.execute(Request::DiscardDraft).unwrap();
    assert!(!folder.exists());
    assert!(!restored.snapshot().has_draft);
    assert!(!restored.snapshot().can_undo);
    assert_eq!(restored.pixels().as_ref(), &original);
    assert_eq!(fs::read(capture).unwrap(), original_bytes);
    assert_eq!(fs::read(export).unwrap(), original_bytes);
}

#[test]
fn failed_save_and_missing_original_discard_do_not_claim_persistence_or_destroy_draft() {
    let (data, id, _) = setup();
    let mut editor = open(data.path(), &id).unwrap();
    editor.execute(crop()).unwrap();
    fs::write(data.path().join("drafts"), b"blocked").unwrap();
    assert!(
        editor
            .execute(Request::SaveDraft { updated_at_ms: 9 })
            .is_err()
    );
    assert!(editor.snapshot().unsaved_changes);
    assert!(!editor.snapshot().has_draft);
    fs::remove_file(data.path().join("drafts")).unwrap();
    editor
        .execute(Request::SaveDraft { updated_at_ms: 10 })
        .unwrap();
    let pixels = editor.pixels();
    fs::remove_file(data.path().join("history").join(&id).join("capture.png")).unwrap();
    // The draft owns all pixels and is still openable without the original PNG.
    assert_eq!(open(data.path(), &id).unwrap().pixels(), pixels);
    assert!(editor.execute(Request::DiscardDraft).is_err());
    assert!(editor.snapshot().has_draft);
    assert!(Arc::ptr_eq(&editor.pixels(), &pixels));
    assert!(
        data.path()
            .join("drafts")
            .join(id)
            .join("manifest.json")
            .is_file()
    );
}

#[test]
fn draft_paths_and_hidden_original_sources_cannot_escape_owned_assets() {
    let (data, id, _) = setup();
    let mut editor = open(data.path(), &id).unwrap();
    let frame = editor.pixels();
    let mut document = editor.snapshot().document.clone();
    let Element::Image(image) = &mut document.elements[0] else {
        panic!()
    };
    image.base.visible = false;
    image.original_src = OptionalNullable::Value("file:///etc/passwd".into());
    assert!(editor.execute(Request::Commit { document }).is_err());
    assert!(Arc::ptr_eq(&frame, &editor.pixels()));
    assert!(!editor.snapshot().can_undo);
    editor
        .execute(Request::SaveDraft { updated_at_ms: 1 })
        .unwrap();
    let manifest = data.path().join("drafts").join(&id).join("manifest.json");
    let mut value: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
    value["document"]["elements"][0]["src"] = json!("file:///etc/passwd");
    fs::write(&manifest, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(
        open(data.path(), &id)
            .err()
            .unwrap()
            .contains("owned draft assets")
    );
    assert!(manifest.is_file()); // Unsupported drafts stay available for other hosts.
    assert!(open(data.path(), "../outside").is_err());
}

#[test]
fn image_header_limits_reject_before_decode_and_invalid_numbers_preserve_history() {
    let (data, id, _) = setup();
    let mut editor = open(data.path(), &id).unwrap();
    for request in [
        Request::ResizeCanvas {
            width: f64::NAN,
            height: 3.,
        },
        Request::Crop {
            rect: Rect {
                x: 0.,
                y: f64::INFINITY,
                width: 1.,
                height: 1.,
            },
        },
    ] {
        assert!(editor.execute(request).is_err());
        assert!(!editor.snapshot().can_undo);
    }
    let oversized = RgbaImage::new(16385, 1);
    let bytes = captures_history::encode_png(&oversized).unwrap();
    fs::write(
        data.path().join("history").join(&id).join("capture.png"),
        bytes,
    )
    .unwrap();
    assert!(open(data.path(), &id).err().unwrap().contains("dimension"));
}

fn text_fonts() -> captures_history::editor_draft::FontAssets {
    captures_history::editor_draft::FontAssets {
        families: BTreeMap::from([("sans".into(), "Captures Shaping Test".into())]),
        files: BTreeMap::from([(
            "regular".into(),
            Arc::from(include_bytes!("../../captures-image/tests/shaping-regular.ttf").as_slice()),
        )]),
    }
}

fn open_text(
    root: &Path,
    id: &str,
    fonts: captures_history::editor_draft::FontAssets,
) -> Result<EditorSession, String> {
    EditorSession::open_with_fonts(
        OpenRequest {
            history_root: root.join("history"),
            drafts_root: root.join("drafts"),
            artifact_id: id.into(),
        },
        Some(fonts),
    )
}

fn add_text(editor: &mut EditorSession) {
    let mut document = editor.snapshot().document.clone();
    document.width = 200.;
    document.height = 180.;
    document.elements.push(
        serde_json::from_value(json!({
            "kind":"text", "id":"label", "x":20, "y":20, "visible":true, "locked":false,
            "opacity":100, "blendMode":"source-over", "text":"L", "fontSize":80,
            "width":100, "fontFamily":"sans", "bold":false, "italic":false,
            "align":"left", "color":"#ff0000", "background":null,
            "outlined":false, "roundedBackground":false
        }))
        .unwrap(),
    );
    editor.execute(Request::Commit { document }).unwrap();
}

#[test]
fn text_session_owns_fonts_across_edits_output_and_draft_restore() {
    fn assert_send<T: Send>() {}
    assert_send::<EditorSession>();
    let (data, id, original) = setup();
    let mut editor = open_text(data.path(), &id, text_fonts()).unwrap();
    add_text(&mut editor);
    // Original fixture L: 56 advance, 48×64 ink, baseline/centering put ink at y38.
    assert_eq!(editor.pixels().get_pixel(22, 45).0, [255, 0, 0, 255]);
    assert_eq!(editor.pixels().get_pixel(60, 60).0, [247, 247, 245, 255]);
    let painted = editor.pixels();
    editor.execute(Request::Undo).unwrap();
    assert_eq!(editor.pixels().as_ref(), &original);
    editor.execute(Request::Redo).unwrap();
    assert_eq!(editor.pixels(), painted);
    let options = serde_json::from_value(json!({
        "format":"png", "quality":"preserve", "quality_value":100, "png":{}
    }))
    .unwrap();
    let encoded = editor.encode_export(options).unwrap();
    assert_eq!(
        image::load_from_memory(&encoded).unwrap().to_rgba8(),
        *painted
    );
    editor
        .execute(Request::SaveDraft { updated_at_ms: 53 })
        .unwrap();
    assert!(!editor.snapshot().unsaved_changes);
    drop(editor);

    // No system font discovery or caller bytes needed to reopen this exact frame.
    let mut restored = open(data.path(), &id).unwrap();
    assert_eq!(restored.pixels(), painted);
    let mut changed_defaults = text_fonts();
    changed_defaults
        .files
        .insert("regular".into(), Arc::from(b"new OS font".as_slice()));
    assert_eq!(
        open_text(data.path(), &id, changed_defaults)
            .unwrap()
            .pixels(),
        painted
    );
    restored
        .import_image(ImportImage {
            pixels: RgbaImage::from_pixel(2, 3, Rgba([0, 255, 0, 255])),
            name: "added".into(),
            selected_id: None,
            point: Some(Point { x: 130., y: 120. }),
        })
        .unwrap();
    assert_eq!(restored.pixels().get_pixel(22, 45).0, [255, 0, 0, 255]);
    restored
        .execute(Request::Crop {
            rect: Rect {
                x: 10.,
                y: 5.,
                width: 180.,
                height: 150.,
            },
        })
        .unwrap();
    assert_eq!(restored.pixels().get_pixel(12, 40).0, [255, 0, 0, 255]);
    restored
        .execute(Request::SaveDraft { updated_at_ms: 54 })
        .unwrap();
    assert_eq!(open(data.path(), &id).unwrap().pixels(), restored.pixels());
    restored.execute(Request::DiscardDraft).unwrap();
    assert_eq!(restored.pixels().as_ref(), &original);
    assert!(!data.path().join("drafts").join(&id).exists());
    // Discard abandons edits, not the current worker's text capability.
    add_text(&mut restored);
    assert_eq!(restored.pixels(), painted);
}

#[test]
fn text_failures_preserve_frames_redo_and_saved_drafts_without_fallback() {
    let (data, id, _) = setup();
    let mut editor = open_text(data.path(), &id, text_fonts()).unwrap();
    add_text(&mut editor);
    editor
        .execute(Request::SaveDraft { updated_at_ms: 59 })
        .unwrap();
    let draft = data.path().join("drafts").join(&id);
    let manifest = fs::read(draft.join("manifest.json")).unwrap();
    let painted = editor.pixels();
    editor
        .execute(Request::ResizeCanvas {
            width: 210.,
            height: 180.,
        })
        .unwrap();
    editor.execute(Request::Undo).unwrap();
    let before = serde_json::to_value(editor.snapshot()).unwrap();
    let frame = editor.pixels();
    for rotation in [false, true] {
        let mut document = editor.snapshot().document.clone();
        let Element::Text(label) = document.elements.last_mut().unwrap() else {
            panic!()
        };
        if rotation {
            label.base.rotation = Some(30.);
        } else {
            label.font_family = "missing".into();
        }
        assert!(editor.execute(Request::Commit { document }).is_err());
        assert_eq!(serde_json::to_value(editor.snapshot()).unwrap(), before);
        assert!(Arc::ptr_eq(&frame, &editor.pixels()));
        assert!(editor.snapshot().can_redo);
        assert_eq!(fs::read(draft.join("manifest.json")).unwrap(), manifest);
    }
    let font_path = draft.join("fonts/regular.font");
    let bytes = fs::read(&font_path).unwrap();
    fs::remove_file(&font_path).unwrap();
    assert!(open_text(data.path(), &id, text_fonts()).is_err());
    assert_eq!(fs::read(draft.join("manifest.json")).unwrap(), manifest);
    fs::write(&font_path, b"corrupt").unwrap();
    assert!(
        open_text(data.path(), &id, text_fonts())
            .err()
            .unwrap()
            .contains("font")
    );
    assert_eq!(fs::read(draft.join("manifest.json")).unwrap(), manifest);
    fs::write(font_path, bytes).unwrap();
    assert_eq!(open(data.path(), &id).unwrap().pixels(), painted);
}
