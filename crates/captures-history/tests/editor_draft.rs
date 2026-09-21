use std::{collections::BTreeMap, fs, sync::Arc};

use captures_history::editor_draft::{self, AssetInput, FontAssets, SaveRequest};
use serde_json::{Value, json};
use tempfile::tempdir;

fn asset(id: &str, png: Option<&[u8]>) -> AssetInput {
    AssetInput {
        id: id.into(),
        png: png.map(<[u8]>::to_vec),
    }
}

fn request(assets: Vec<AssetInput>) -> SaveRequest {
    SaveRequest {
        artifact_id: "capture-1".into(),
        document: json!({"elements": [], "opaque": {"keep": true}}),
        assets,
        updated_at_ms: 17,
    }
}

fn url(artifact: &str, asset: &str) -> String {
    format!("test-host://{artifact}/{asset}")
}

#[test]
fn shipping_wire_shape_and_manifest_round_trip_without_persisting_host_urls() {
    let root = tempdir().unwrap();
    let pixels = image::RgbaImage::from_fn(3, 2, |x, y| {
        image::Rgba([x as u8 * 73, y as u8 * 91, 29, 128])
    });
    let png = captures_history::encode_png(&pixels).unwrap();
    let document = json!({
        "width": 713, "height": 257, "unknown": {"version": "kept"},
        "elements": [
            {"kind": "image", "src": "draft-asset:edited", "originalSrc": "draft-asset:original"},
            {"kind": "text", "src": "draft-asset:not-an-image", "text": "Hello"},
            {"kind": "image", "src": "data:image/png;base64,unchanged", "originalSrc": null}
        ]
    });
    let input: SaveRequest = serde_json::from_value(json!({
        "artifact_id": "capture-1", "document": document, "updated_at_ms": 8193,
        "assets": [{"id": "edited", "png": png}, {"id": "original", "png": [5, 6, 7]}]
    }))
    .unwrap();
    editor_draft::save(root.path(), input).unwrap();
    let manifest_path = root.path().join("capture-1/manifest.json");
    let before = fs::read(&manifest_path).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&before).unwrap(),
        json!({"schema_version": 1, "artifact_id": "capture-1", "updated_at_ms": 8193, "document": document})
    );
    let mut expected = document;
    expected["elements"][0]["src"] = json!("test-host://capture-1/edited");
    expected["elements"][0]["originalSrc"] = json!("test-host://capture-1/original");
    let loaded = editor_draft::load(root.path(), "capture-1", url)
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(loaded).unwrap(),
        json!({"updated_at_ms": 8193, "document": expected})
    );
    assert_eq!(fs::read(manifest_path).unwrap(), before);
    let restored = editor_draft::read_asset(root.path(), "capture-1", "edited").unwrap();
    assert_eq!(restored, png);
    assert_eq!(
        image::load_from_memory(&restored).unwrap().to_rgba8(),
        pixels
    );
    assert_eq!(
        editor_draft::read_asset(root.path(), "capture-1", "original").unwrap(),
        [5, 6, 7]
    );
}

#[test]
fn incremental_save_reuses_replaces_adds_and_prunes_assets() {
    let root = tempdir().unwrap();
    editor_draft::save(
        root.path(),
        request(vec![
            asset("keep", Some(b"untouched")),
            asset("replace", Some(b"before")),
            asset("remove", Some(b"orphan")),
        ]),
    )
    .unwrap();
    // Omitted png and explicit null both retain previously saved bytes.
    let next: SaveRequest = serde_json::from_value(json!({
        "artifact_id": "capture-1", "document": {"changed": true}, "updated_at_ms": 93,
        "assets": [{"id": "keep"}, {"id": "replace", "png": [8, 9]}, {"id": "add", "png": [1]}]
    }))
    .unwrap();
    editor_draft::save(root.path(), next).unwrap();
    let updated = editor_draft::load(root.path(), "capture-1", url)
        .unwrap()
        .unwrap();
    assert_eq!(updated.document, json!({"changed": true}));
    assert_eq!(updated.updated_at_ms, 93);
    let retained: SaveRequest = serde_json::from_value(json!({
        "artifact_id": "capture-1", "document": {"changed": true}, "updated_at_ms": 94,
        "assets": [{"id": "keep", "png": null}, {"id": "replace", "png": null}, {"id": "add", "png": null}]
    }))
    .unwrap();
    editor_draft::save(root.path(), retained).unwrap();
    assert_eq!(
        editor_draft::read_asset(root.path(), "capture-1", "keep").unwrap(),
        b"untouched"
    );
    assert_eq!(
        editor_draft::read_asset(root.path(), "capture-1", "replace").unwrap(),
        [8, 9]
    );
    assert_eq!(
        editor_draft::read_asset(root.path(), "capture-1", "add").unwrap(),
        [1]
    );
    assert!(!root.path().join("capture-1/assets/remove.png").exists());
    assert_eq!(
        fs::read_dir(root.path().join("capture-1/assets"))
            .unwrap()
            .count(),
        3
    );
    assert_eq!(
        fs::read_dir(root.path().join("capture-1")).unwrap().count(),
        2
    );
}

#[test]
fn validation_failures_do_not_prune_or_replace_existing_draft() {
    let root = tempdir().unwrap();
    editor_draft::save(root.path(), request(vec![asset("keep", Some(b"old"))])).unwrap();
    let manifest = root.path().join("capture-1/manifest.json");
    let before = fs::read(&manifest).unwrap();
    for (bad, message) in [
        (
            asset("missing", None),
            "a previously saved draft image asset is missing; resend the full draft",
        ),
        (
            asset("empty", Some(b"")),
            "a draft image asset is empty or too large",
        ),
        (
            asset("../outside", Some(b"new")),
            "invalid screenshot editor draft id",
        ),
    ] {
        let error = editor_draft::save(root.path(), request(vec![asset("new", Some(b"new")), bad]))
            .unwrap_err();
        assert_eq!(error.to_string(), message);
        assert_eq!(fs::read(&manifest).unwrap(), before);
        assert_eq!(
            editor_draft::read_asset(root.path(), "capture-1", "keep").unwrap(),
            b"old"
        );
        assert!(!root.path().join("capture-1/assets/new.png").exists());
    }
    fs::create_dir(root.path().join("capture-1/assets/directory.png")).unwrap();
    assert_eq!(
        editor_draft::save(root.path(), request(vec![asset("directory", None)]))
            .unwrap_err()
            .to_string(),
        "a previously saved draft image asset is missing; resend the full draft"
    );
}

#[test]
fn replacement_failure_surfaces_io_error_and_leaves_the_old_manifest() {
    let root = tempdir().unwrap();
    editor_draft::save(root.path(), request(vec![asset("keep", Some(b"old"))])).unwrap();
    let manifest = root.path().join("capture-1/manifest.json");
    let before = fs::read(&manifest).unwrap();
    // A directory at the destination deterministically rejects file replacement
    // without depending on administrator/root permission behavior.
    fs::create_dir(root.path().join("capture-1/assets/blocked.png")).unwrap();
    let result = editor_draft::save(
        root.path(),
        request(vec![asset("keep", None), asset("blocked", Some(b"new"))]),
    );
    assert!(matches!(result, Err(captures_history::Error::Io(_))));
    assert_eq!(fs::read(&manifest).unwrap(), before);
    assert_eq!(
        editor_draft::read_asset(root.path(), "capture-1", "keep").unwrap(),
        b"old"
    );
    assert_eq!(
        fs::read_dir(root.path().join("capture-1/assets"))
            .unwrap()
            .count(),
        2
    );
}

#[test]
fn retained_bytes_and_asset_count_obey_both_sides_of_shipping_limits() {
    let root = tempdir().unwrap();
    editor_draft::save(root.path(), request(vec![asset("large", Some(b"x"))])).unwrap();
    let large = root.path().join("capture-1/assets/large.png");
    // Sparse file checks metadata accounting without allocating an 80 MiB buffer.
    let file = fs::OpenOptions::new().write(true).open(&large).unwrap();
    file.set_len(80 * 1024 * 1024 - 1).unwrap();
    editor_draft::save(
        root.path(),
        request(vec![asset("large", None), asset("last", Some(b"x"))]),
    )
    .unwrap();
    let manifest = fs::read(root.path().join("capture-1/manifest.json")).unwrap();
    file.set_len(80 * 1024 * 1024).unwrap();
    let error = editor_draft::save(
        root.path(),
        request(vec![asset("large", None), asset("last", Some(b"x"))]),
    )
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "unsaved edits are too large to keep as a draft; save a file first"
    );
    assert_eq!(
        fs::read(root.path().join("capture-1/manifest.json")).unwrap(),
        manifest
    );
    drop(file);

    let assets = |count| {
        (0..count)
            .map(|i| asset(&format!("image-{i}"), Some(b"x")))
            .collect()
    };
    editor_draft::save(root.path(), request(assets(64))).unwrap();
    assert_eq!(
        editor_draft::save(root.path(), request(assets(65)))
            .unwrap_err()
            .to_string(),
        "this edit has too many image layers to keep as a draft"
    );
    assert_eq!(
        fs::read_dir(root.path().join("capture-1/assets"))
            .unwrap()
            .count(),
        64
    );
}

#[test]
fn every_entry_point_rejects_unsafe_ids_and_accepts_the_length_boundary() {
    let root = tempdir().unwrap();
    for id in [
        "",
        "../outside",
        "has/slash",
        "has\\slash",
        "a.b",
        "a:b",
        "é",
        "a\0b",
        &"a".repeat(81),
    ] {
        let mut input = request(vec![]);
        input.artifact_id = id.into();
        assert!(editor_draft::save(root.path(), input).is_err(), "{id:?}");
        assert!(editor_draft::save(root.path(), request(vec![asset(id, Some(b"x"))])).is_err());
        assert!(editor_draft::load(root.path(), id, url).is_err());
        assert!(editor_draft::discard(root.path(), id).is_err());
        assert!(editor_draft::read_asset(root.path(), id, "image").is_err());
        assert!(editor_draft::read_asset(root.path(), "capture-1", id).is_err());
    }
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    let id = "a".repeat(80);
    let mut input = request(vec![asset("a_b-09", Some(b"x"))]);
    input.artifact_id = id.clone();
    editor_draft::save(root.path(), input).unwrap();
    assert_eq!(
        editor_draft::read_asset(root.path(), &id, "a_b-09").unwrap(),
        b"x"
    );
}

#[test]
fn load_reads_legacy_manifest_and_distinguishes_cleanup_from_errors() {
    let root = tempdir().unwrap();
    assert!(
        editor_draft::load(root.path(), "absent", url)
            .unwrap()
            .is_none()
    );
    // Write old-format files independently of save(), so reader and writer
    // cannot silently agree on a changed schema or filename.
    for (schema, id, document, discarded) in [
        (1, "capture-1", json!({"elements": []}), false),
        (2, "capture-1", json!({"elements": []}), true),
        (
            1,
            "capture-1",
            json!({"elements": [{"kind": "image", "src": "draft-asset:missing"}]}),
            true,
        ),
        (
            1,
            "capture-1",
            json!({"elements": [{"kind": "image", "originalSrc": "draft-asset:../escape"}]}),
            true,
        ),
    ] {
        fs::create_dir_all(root.path().join("capture-1/assets")).unwrap();
        fs::write(root.path().join("capture-1/manifest.json"), serde_json::to_vec(&json!({
            "schema_version": schema, "artifact_id": id, "updated_at_ms": 47, "document": document
        })).unwrap()).unwrap();
        let loaded = editor_draft::load(root.path(), "capture-1", url).unwrap();
        assert_eq!(loaded.is_none(), discarded);
        assert_eq!(root.path().join("capture-1").exists(), !discarded);
        if let Some(loaded) = loaded {
            assert_eq!(loaded.document, document);
            assert_eq!(loaded.updated_at_ms, 47);
        }
    }
    fs::create_dir_all(root.path().join("capture-1")).unwrap();
    let manifest = root.path().join("capture-1/manifest.json");
    for bytes in [
        b"{".as_slice(),
        br#"{"schema_version":1,"artifact_id":"other","updated_at_ms":47,"document":{}}"#,
    ] {
        fs::write(&manifest, bytes).unwrap();
        assert!(editor_draft::load(root.path(), "capture-1", url).is_err());
        assert_eq!(fs::read(&manifest).unwrap(), bytes);
    }
}

#[test]
fn caller_roots_and_discard_are_isolated_from_other_drafts_history_and_exports() {
    let root = tempdir().unwrap();
    let native = root.path().join("native-drafts");
    let shipping = root.path().join("shipping-drafts");
    let history = root.path().join("history/capture-1");
    fs::create_dir_all(&history).unwrap();
    fs::write(history.join("capture.png"), b"history").unwrap();
    fs::write(root.path().join("export.png"), b"export").unwrap();
    editor_draft::save(&native, request(vec![asset("image", Some(b"native"))])).unwrap();
    editor_draft::save(&shipping, request(vec![asset("image", Some(b"shipping"))])).unwrap();
    let mut other = request(vec![]);
    other.artifact_id = "other".into();
    editor_draft::save(&native, other).unwrap();
    editor_draft::discard(&native, "capture-1").unwrap();
    editor_draft::discard(&native, "capture-1").unwrap();
    assert!(!native.join("capture-1").exists());
    assert!(native.join("other/manifest.json").is_file());
    assert_eq!(
        editor_draft::read_asset(&shipping, "capture-1", "image").unwrap(),
        b"shipping"
    );
    assert_eq!(fs::read(history.join("capture.png")).unwrap(), b"history");
    assert_eq!(fs::read(root.path().join("export.png")).unwrap(), b"export");
}

fn fonts(id: &str, bytes: &[u8]) -> FontAssets {
    FontAssets {
        families: BTreeMap::from([("sans".into(), "Embedded Family".into())]),
        files: BTreeMap::from([(id.into(), Arc::from(bytes))]),
        notices: String::new(),
    }
}

#[test]
fn fonts_round_trip_as_sidecars_not_json_and_cannot_rebind_ids() {
    let root = tempdir().unwrap();
    let input = fonts("regular", b"opaque trusted font bytes");
    editor_draft::save_with_fonts(root.path(), request(vec![]), Some(&input)).unwrap();
    let manifest = root.path().join("capture-1/manifest.json");
    let before = fs::read(&manifest).unwrap();
    let value: Value = serde_json::from_slice(&before).unwrap();
    assert_eq!(
        value["fonts"],
        json!({
            "families": {"sans":"Embedded Family"}, "assets":["regular"]
        })
    );
    let loaded = editor_draft::load(root.path(), "capture-1", url)
        .unwrap()
        .unwrap();
    assert_eq!(loaded.fonts.as_ref(), Some(&input));
    assert_eq!(
        serde_json::to_value(&loaded).unwrap(),
        json!({
            "document": request(vec![]).document, "updated_at_ms":17
        })
    );
    assert!(
        editor_draft::save_with_fonts(
            root.path(),
            request(vec![]),
            Some(&fonts("regular", b"replacement"))
        )
        .unwrap_err()
        .to_string()
        .contains("cannot change")
    );
    assert_eq!(fs::read(manifest).unwrap(), before);
    assert_eq!(
        fs::read(root.path().join("capture-1/fonts/regular.font")).unwrap(),
        b"opaque trusted font bytes"
    );
    editor_draft::save_with_fonts(root.path(), request(vec![]), Some(&input)).unwrap();
    editor_draft::save(root.path(), request(vec![])).unwrap();
    assert!(!root.path().join("capture-1/fonts/regular.font").exists());
    assert!(
        editor_draft::load(root.path(), "capture-1", url)
            .unwrap()
            .unwrap()
            .fonts
            .is_none()
    );
}

#[test]
fn failed_manifest_publication_retains_previous_fonts_then_success_prunes() {
    let root = tempdir().unwrap();
    let previous = fonts("previous", b"old");
    editor_draft::save_with_fonts(root.path(), request(vec![]), Some(&previous)).unwrap();
    let manifest = root.path().join("capture-1/manifest.json");
    let backup = root.path().join("capture-1/previous.json");
    fs::rename(&manifest, &backup).unwrap();
    fs::create_dir(&manifest).unwrap(); // Force manifest replacement to fail after font writes.
    let next = fonts("next", b"different");
    assert!(editor_draft::save_with_fonts(root.path(), request(vec![]), Some(&next)).is_err());
    assert_eq!(
        fs::read(root.path().join("capture-1/fonts/previous.font")).unwrap(),
        b"old"
    );
    assert_eq!(
        fs::read(root.path().join("capture-1/fonts/next.font")).unwrap(),
        b"different"
    );
    fs::remove_dir(&manifest).unwrap();
    fs::rename(backup, manifest).unwrap();
    assert_eq!(
        editor_draft::load(root.path(), "capture-1", url)
            .unwrap()
            .unwrap()
            .fonts,
        Some(previous)
    );
    editor_draft::save_with_fonts(root.path(), request(vec![]), Some(&next)).unwrap();
    assert!(!root.path().join("capture-1/fonts/previous.font").exists());
    assert_eq!(
        editor_draft::load(root.path(), "capture-1", url)
            .unwrap()
            .unwrap()
            .fonts,
        Some(next)
    );
}

#[test]
fn font_notices_round_trip_and_reject_oversize_before_mutation() {
    let root = tempdir().unwrap();
    let mut input = fonts("licensed", b"font");
    input.notices = "Copyright\nLicense: é\n".into();
    editor_draft::save_with_fonts(root.path(), request(vec![]), Some(&input)).unwrap();
    assert_eq!(
        editor_draft::load(root.path(), "capture-1", url)
            .unwrap()
            .unwrap()
            .fonts,
        Some(input.clone())
    );
    input.notices = "x".repeat(64 * 1024);
    editor_draft::save_with_fonts(root.path(), request(vec![]), Some(&input)).unwrap();
    let manifest = root.path().join("capture-1/manifest.json");
    let before = fs::read(&manifest).unwrap();
    input.notices.push('x');
    assert!(editor_draft::save_with_fonts(root.path(), request(vec![]), Some(&input)).is_err());
    assert_eq!(fs::read(&manifest).unwrap(), before);
}

#[test]
fn font_validation_precedes_mutation_and_shares_the_image_byte_budget() {
    let root = tempdir().unwrap();
    editor_draft::save(root.path(), request(vec![asset("keep", Some(b"old"))])).unwrap();
    let manifest = root.path().join("capture-1/manifest.json");
    let before = fs::read(&manifest).unwrap();
    let mut too_many = fonts("valid", b"font");
    too_many.files = (0..65)
        .map(|i| (format!("f{i}"), Arc::from(b"x".as_slice())))
        .collect();
    for invalid in [
        fonts("../outside", b"x"),
        fonts("empty", b""),
        too_many,
        FontAssets {
            families: BTreeMap::new(),
            ..fonts("valid", b"x")
        },
    ] {
        assert!(
            editor_draft::save_with_fonts(root.path(), request(vec![]), Some(&invalid)).is_err()
        );
        assert_eq!(fs::read(&manifest).unwrap(), before);
        assert_eq!(
            editor_draft::read_asset(root.path(), "capture-1", "keep").unwrap(),
            b"old"
        );
        assert!(!root.path().join("capture-1/fonts").exists());
    }
    let retained = fs::OpenOptions::new()
        .write(true)
        .open(root.path().join("capture-1/assets/keep.png"))
        .unwrap();
    retained.set_len(80 * 1024 * 1024 - 3).unwrap();
    let font = fonts("limit", b"abc");
    editor_draft::save_with_fonts(root.path(), request(vec![asset("keep", None)]), Some(&font))
        .unwrap();
    let before = fs::read(&manifest).unwrap();
    let mut licensed = font.clone();
    licensed.notices = "x".into();
    assert!(
        editor_draft::save_with_fonts(
            root.path(),
            request(vec![asset("keep", None)]),
            Some(&licensed)
        )
        .is_err()
    );
    assert_eq!(fs::read(&manifest).unwrap(), before);
    retained.set_len(80 * 1024 * 1024 - 2).unwrap();
    assert!(
        editor_draft::save_with_fonts(root.path(), request(vec![asset("keep", None)]), Some(&font))
            .is_err()
    );
    assert_eq!(fs::read(manifest).unwrap(), before);
}

#[test]
fn invalid_or_missing_font_sidecars_error_without_discarding_the_draft() {
    let root = tempdir().unwrap();
    editor_draft::save_with_fonts(root.path(), request(vec![]), Some(&fonts("face", b"font")))
        .unwrap();
    let manifest = root.path().join("capture-1/manifest.json");
    let original: Value = serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
    for ids in [
        json!(["face", "face"]),
        json!(["../outside"]),
        json!(["missing"]),
    ] {
        let mut invalid = original.clone();
        invalid["fonts"]["assets"] = ids;
        let bytes = serde_json::to_vec(&invalid).unwrap();
        fs::write(&manifest, &bytes).unwrap();
        assert!(editor_draft::load(root.path(), "capture-1", url).is_err());
        assert_eq!(fs::read(&manifest).unwrap(), bytes);
    }
    fs::write(&manifest, serde_json::to_vec(&original).unwrap()).unwrap();
    let sidecar = fs::OpenOptions::new()
        .write(true)
        .open(root.path().join("capture-1/fonts/face.font"))
        .unwrap();
    for len in [0, 80 * 1024 * 1024 + 1] {
        sidecar.set_len(len).unwrap();
        assert!(editor_draft::load(root.path(), "capture-1", url).is_err());
        assert!(manifest.is_file());
    }
}
