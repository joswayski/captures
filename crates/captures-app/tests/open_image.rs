use std::{fs, path::Path};

use captures_app::{
    Request, Response,
    editor_session::{EditorSession, OpenRequest, Request as EditorRequest},
    execute,
};
use captures_history::ArtifactKind;
use captures_recording::RecordingTarget;
use image::{ImageFormat, Rgba, RgbaImage};

fn pixels() -> RgbaImage {
    RgbaImage::from_fn(80, 60, |x, y| {
        if x < 40 {
            Rgba([225, y as u8, 17, 255])
        } else {
            Rgba([13, y as u8, 211, 255])
        }
    })
}

fn open(
    root: &Path,
    path: &Path,
    open_artifact_ids: Vec<String>,
) -> Result<(captures_app::Artifact, bool), captures_app::Error> {
    let Response::OpenedImage {
        artifact,
        already_open,
    } = execute(Request::OpenImage {
        root: root.into(),
        path: path.into(),
        open_artifact_ids,
    })?
    else {
        panic!("expected opened image")
    };
    Ok((artifact, already_open))
}

fn editor(root: &Path, id: &str) -> EditorSession {
    EditorSession::open(OpenRequest {
        history_root: root.into(),
        drafts_root: root.with_file_name("editor-drafts"),
        artifact_id: id.into(),
    })
    .unwrap()
}

#[test]
fn opens_asymmetric_png_jpeg_webp_as_owned_history_without_changing_source() {
    for format in [ImageFormat::Png, ImageFormat::Jpeg, ImageFormat::WebP] {
        let data = tempfile::tempdir().unwrap();
        let root = data.path().join("capture-history");
        let source = data.path().join("source.data"); // Bytes, not extension, determine format.
        if format == ImageFormat::Jpeg {
            image::DynamicImage::ImageRgb8(image::DynamicImage::ImageRgba8(pixels()).into_rgb8())
                .save_with_format(&source, format)
                .unwrap();
        } else {
            pixels().save_with_format(&source, format).unwrap();
        }
        let canonical = source.canonicalize().unwrap();
        let original = fs::read(&source).unwrap();
        let expected = captures_app::editor_image_decode::decode_opened_image(&source).unwrap();
        let (artifact, already_open) = open(&root, &source, vec![]).unwrap();
        assert!(!already_open);
        assert_eq!(artifact.entry.saved_path.as_deref(), canonical.to_str());
        assert_eq!((artifact.entry.width, artifact.entry.height), (80, 60));
        assert_eq!(artifact.entry.mime_type.as_deref(), Some("image/png"));
        assert_eq!(fs::read(&source).unwrap(), original);
        assert!(artifact.preview_path.is_file());
        assert_eq!(
            image::open(&artifact.image_path).unwrap().to_rgba8(),
            expected
        );
        assert_eq!(
            editor(&root, &artifact.entry.id)
                .snapshot()
                .original_export_path,
            Some(canonical.as_path())
        );
        assert_eq!(captures_app::list(&root).unwrap().len(), 1);
        if format == ImageFormat::Jpeg {
            assert!(expected.get_pixel(5, 30)[0] > expected.get_pixel(75, 30)[0]);
            assert!(expected.get_pixel(75, 30)[2] > expected.get_pixel(5, 30)[2]);
        } else {
            assert_eq!(expected, pixels());
        }
    }
}

#[test]
fn aliases_and_open_ids_preserve_draft_and_closed_reload_reuses_id() {
    let data = tempfile::tempdir().unwrap();
    let root = data.path().join("capture-history");
    let source = data.path().join("source.png");
    pixels().save(&source).unwrap();
    let (first, _) = open(&root, &source, vec![]).unwrap();
    let id = first.entry.id.clone();
    let mut session = editor(&root, &id);
    session
        .execute(EditorRequest::ResizeCanvas {
            width: 91.,
            height: 63.,
        })
        .unwrap();
    session
        .execute(EditorRequest::SaveDraft { updated_at_ms: 42 })
        .unwrap();
    let draft = root.with_file_name("editor-drafts").join(&id);
    let manifest = fs::read(draft.join("manifest.json")).unwrap();
    let old_png = fs::read(&first.image_path).unwrap();
    let alias = data.path().join(".").join("source.png");

    // Even external source corruption cannot replace the already-open editor.
    fs::write(&source, b"corrupt").unwrap();
    let (again, already_open) = open(&root, &alias, vec![id.clone()]).unwrap();
    assert!(already_open);
    assert_eq!(again.entry.id, id);
    assert_eq!(fs::read(&first.image_path).unwrap(), old_png);
    assert_eq!(fs::read(draft.join("manifest.json")).unwrap(), manifest);
    assert_eq!(session.pixels().dimensions(), (91, 63));
    let error = open(&root, &alias, vec![]).unwrap_err().to_string();
    assert!(
        error.contains("History") && error.contains("discard"),
        "{error}"
    );
    assert_eq!(fs::read(&first.image_path).unwrap(), old_png);
    assert_eq!(fs::read(draft.join("manifest.json")).unwrap(), manifest);

    // A closed editor cannot discard its only draft merely by retrying open.
    drop(session);
    captures_history::editor_draft::discard(&root.with_file_name("editor-drafts"), &id).unwrap();
    assert!(open(&root, &alias, vec![]).is_err()); // Now decoding rejects the corrupt source.
    assert_eq!(fs::read(&first.image_path).unwrap(), old_png);

    let replacement = RgbaImage::from_fn(37, 19, |x, y| Rgba([x as u8 * 4, 29, y as u8 * 11, 255]));
    replacement.save(&source).unwrap();
    let source_bytes = fs::read(&source).unwrap();
    let (reloaded, already_open) = open(&root, &alias, vec![]).unwrap();
    assert!(!already_open);
    assert_eq!(reloaded.entry.id, id);
    assert_eq!((reloaded.entry.width, reloaded.entry.height), (37, 19));
    assert_ne!(fs::read(&reloaded.image_path).unwrap(), old_png);
    assert!(!draft.exists());
    assert_eq!(editor(&root, &id).pixels().dimensions(), (37, 19));
    assert_eq!(fs::read(source).unwrap(), source_bytes);
    assert_eq!(captures_app::list(&root).unwrap().len(), 1);
}

#[test]
fn unsupported_missing_and_corrupt_sources_do_not_create_history() {
    let data = tempfile::tempdir().unwrap();
    let root = data.path().join("capture-history");
    let source = data.path().join("source.data");
    assert!(open(&root, &source, vec![]).is_err());
    for (bytes, expected) in [
        (b"GIF89a".as_slice(), "PNG, JPEG or WebP"),
        (b"not an image", "image"),
    ] {
        fs::write(&source, bytes).unwrap();
        assert!(
            open(&root, &source, vec![])
                .unwrap_err()
                .to_string()
                .contains(expected)
        );
        assert!(captures_app::list(&root).unwrap().is_empty());
        assert_eq!(fs::read(&source).unwrap(), bytes);
    }
    RgbaImage::new(2, 3)
        .save_with_format(&source, ImageFormat::Tiff)
        .unwrap();
    assert!(
        open(&root, &source, vec![])
            .unwrap_err()
            .to_string()
            .contains("PNG, JPEG or WebP")
    );
    assert!(captures_app::list(&root).unwrap().is_empty());
}

#[test]
fn a_recording_with_the_same_saved_path_is_not_an_existing_screenshot() {
    let data = tempfile::tempdir().unwrap();
    let root = data.path().join("capture-history");
    let source = data.path().join("source.png");
    pixels().save(&source).unwrap();
    let (first, _) = open(&root, &source, vec![]).unwrap();
    let preview = fs::read(&first.preview_path).unwrap();
    let source_bytes = fs::read(&source).unwrap();
    captures_history::delete(&root, &first.entry.id).unwrap();
    let mut recording = first.entry.clone();
    recording.kind = ArtifactKind::Video;
    recording.mode = None;
    recording.mime_type = Some("video/mp4".into());
    recording.duration_ms = Some(100);
    recording.target = Some(RecordingTarget::Display {
        display_id: "fixture".into(),
    });
    captures_history::save_recording_reference(&root, &recording, &preview).unwrap();
    let (opened, already_open) = open(&root, &source, vec![recording.id.clone()]).unwrap();
    assert!(!already_open);
    assert_ne!(opened.entry.id, recording.id);
    assert_eq!(
        captures_history::load(&root, chrono::Utc::now())
            .unwrap()
            .len(),
        2
    );
    assert_eq!(fs::read(&source).unwrap(), source_bytes);
    assert_eq!(image::open(opened.image_path).unwrap().to_rgba8(), pixels());
}

#[test]
fn external_open_applies_exif_rotation_and_icc_transform_before_history_publication() {
    use image::ImageEncoder;
    use moxcms::{ColorProfile, ToneReprCurve};

    let data = tempfile::tempdir().unwrap();
    let root = data.path().join("capture-history");
    let source = data.path().join("orientation.jpeg");
    image::DynamicImage::ImageRgb8(image::DynamicImage::ImageRgba8(pixels()).into_rgb8())
        .save_with_format(&source, ImageFormat::Jpeg)
        .unwrap();
    let ordinary = image::open(&source).unwrap().to_rgba8();
    let jpeg = fs::read(&source).unwrap();
    let exif = b"Exif\0\0II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x06\0\0\0\0\0\0\0";
    let mut oriented = vec![0xff, 0xd8, 0xff, 0xe1];
    oriented.extend_from_slice(&((exif.len() + 2) as u16).to_be_bytes());
    oriented.extend_from_slice(exif);
    oriented.extend_from_slice(&jpeg[2..]);
    fs::write(&source, &oriented).unwrap();
    let (artifact, _) = open(&root, &source, vec![]).unwrap();
    let rotated = image::open(&artifact.image_path).unwrap().to_rgba8();
    assert_eq!(rotated.dimensions(), (60, 80));
    assert_eq!(rotated.get_pixel(4, 5), ordinary.get_pixel(5, 59 - 4));
    assert_eq!(fs::read(&source).unwrap(), oriented);

    let profiled = data.path().join("profile.png");
    let mut linear = ColorProfile::new_srgb();
    linear.cicp = None;
    linear.red_trc = Some(ToneReprCurve::Parametric(vec![1.]));
    linear.green_trc = linear.red_trc.clone();
    linear.blue_trc = linear.red_trc.clone();
    let profile = linear.encode().unwrap();
    let mut encoded = Vec::new();
    let mut encoder = image::codecs::png::PngEncoder::new(&mut encoded);
    encoder.set_icc_profile(profile).unwrap();
    encoder
        .write_image(&[128, 64, 32, 73], 1, 1, image::ExtendedColorType::Rgba8)
        .unwrap();
    fs::write(&profiled, &encoded).unwrap();
    let (artifact, _) = open(&root, &profiled, vec![]).unwrap();
    let pixel = image::open(&artifact.image_path)
        .unwrap()
        .to_rgba8()
        .get_pixel(0, 0)
        .0;
    assert_eq!(pixel[3], 73);
    assert!(
        pixel[0].abs_diff(188) <= 1 && pixel[1].abs_diff(137) <= 1 && pixel[2].abs_diff(99) <= 1
    );
    assert_eq!(fs::read(profiled).unwrap(), encoded);
}
