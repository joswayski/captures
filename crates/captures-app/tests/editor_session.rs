use std::{fs, path::Path, sync::Arc};

use captures_app::{
    editor::{Element, OptionalNullable, Rect},
    editor_session::{EditorSession, OpenRequest, Request},
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
