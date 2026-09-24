use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use captures_app::{
    Request, Response, execute,
    recording_editor::{RecordingEditorOpenRequest, RecordingEditorSession},
};
use captures_history::ArtifactKind;
use captures_media::{
    CancelToken, EditSpec, ExportFormat, ExportSpec, MediaToolchain, QualityPreset,
};

fn tools() -> Option<(PathBuf, PathBuf)> {
    let ffmpeg: PathBuf = std::env::var_os("CAPTURES_TEST_FFMPEG")
        .unwrap_or_else(|| "ffmpeg".into())
        .into();
    let ffprobe: PathBuf = std::env::var_os("CAPTURES_TEST_FFPROBE")
        .unwrap_or_else(|| "ffprobe".into())
        .into();
    MediaToolchain::new(ffmpeg.clone(), ffprobe.clone())
        .verify()
        .ok()?;
    Some((ffmpeg, ffprobe))
}

fn open(
    root: &Path,
    path: &Path,
    active: Vec<String>,
    tools: Option<&(PathBuf, PathBuf)>,
) -> Result<(captures_app::Artifact, bool), String> {
    let (ffmpeg, ffprobe) = tools.map_or((None, None), |(a, b)| (Some(a.clone()), Some(b.clone())));
    match execute(Request::OpenMedia {
        root: root.into(),
        path: path.into(),
        open_artifact_ids: active,
        ffmpeg,
        ffprobe,
    })
    .map_err(|error| error.to_string())?
    {
        Response::OpenedMedia {
            artifact,
            already_open,
        } => Ok((artifact, already_open)),
        _ => panic!("expected opened media"),
    }
}

fn fixture(path: &Path, ffmpeg: &Path) {
    let status = Command::new(ffmpeg)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "color=c=red:size=48x32:rate=10:duration=1",
            "-f",
            "lavfi",
            "-i",
            "color=c=blue:size=48x32:rate=10:duration=1",
            "-filter_complex",
            "[0:v][1:v]concat=n=2:v=1:a=0[v]",
            "-map",
            "[v]",
        ])
        .args(match path.extension().and_then(|v| v.to_str()) {
            Some("gif") => &["-f", "gif"][..],
            Some("webm") => &["-c:v", "libvpx-vp9"][..],
            _ => &["-c:v", "mpeg4"][..],
        })
        .arg(path)
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn real_recordings_open_by_content_and_reopen_same_history_reference() {
    let Some((ffmpeg, ffprobe)) = tools() else {
        return;
    };
    let tools = (ffmpeg, ffprobe);
    for (ext, kind, mime) in [
        ("mp4", ArtifactKind::Video, "video/mp4"),
        ("gif", ArtifactKind::Gif, "image/gif"),
        ("webm", ArtifactKind::Video, "video/webm"),
    ] {
        let data = tempfile::tempdir().unwrap();
        let root = data.path().join("capture-history");
        let source = data.path().join(format!("source.{ext}"));
        fixture(&source, &tools.0);
        let bytes = fs::read(&source).unwrap();
        let renamed = data.path().join(if ext == "gif" {
            "misleading.mp4"
        } else {
            "misleading.gif"
        });
        fs::rename(&source, &renamed).unwrap();
        let source = &renamed;
        let (first, active) = open(&root, source, vec![], Some(&tools)).unwrap();
        assert!(!active);
        assert_eq!(first.entry.kind, kind);
        assert_eq!(first.entry.mime_type.as_deref(), Some(mime));
        assert_eq!(
            first.entry.saved_path.as_deref(),
            source.canonicalize().unwrap().to_str()
        );
        assert_eq!(fs::read(source).unwrap(), bytes);
        assert!(first.preview_path.is_file());
        let session = RecordingEditorSession::open(
            RecordingEditorOpenRequest {
                history_root: root.clone(),
                artifact_id: first.entry.id.clone(),
            },
            MediaToolchain::new(tools.0.clone(), tools.1.clone()),
        )
        .unwrap();
        drop(session);
        let invalid = (
            data.path().join("no-ffmpeg"),
            data.path().join("no-ffprobe"),
        );
        let (same, active) =
            open(&root, source, vec![first.entry.id.clone()], Some(&invalid)).unwrap();
        assert!(active);
        assert_eq!(same.entry.id, first.entry.id);
        let (reloaded, active) = open(&root, source, vec![], Some(&tools)).unwrap();
        assert!(!active);
        assert_eq!(reloaded.entry.id, first.entry.id);
        assert_eq!(
            captures_history::load(&root, chrono::Utc::now())
                .unwrap()
                .len(),
            1
        );
        assert_eq!(fs::read(source).unwrap(), bytes);
    }
}

#[test]
fn still_images_need_no_tools_and_failed_media_preflight_leaves_history_empty() {
    let data = tempfile::tempdir().unwrap();
    let root = data.path().join("capture-history");
    let source = data.path().join("still.data");
    image::RgbaImage::from_fn(11, 7, |x, y| {
        image::Rgba([x as u8 * 17, 3, y as u8 * 19, 255])
    })
    .save_with_format(&source, image::ImageFormat::Png)
    .unwrap();
    let invalid = (
        data.path().join("missing-ffmpeg"),
        data.path().join("missing-ffprobe"),
    );
    let (still, active) = open(&root, &source, vec![], Some(&invalid)).unwrap();
    assert!(!active);
    assert_eq!(still.entry.kind, ArtifactKind::Screenshot);
    let (again, active) =
        open(&root, &source, vec![still.entry.id.clone()], Some(&invalid)).unwrap();
    assert!(active);
    assert_eq!(again.entry.id, still.entry.id);

    let Some((ffmpeg, ffprobe)) = tools() else {
        return;
    };
    let video = data.path().join("video.mp4");
    fixture(&video, &ffmpeg);
    let original = fs::read(&video).unwrap();
    assert!(open(&root, &video, vec![], Some(&invalid)).is_err());
    let broken = data.path().join("truncated.gif");
    fs::write(&broken, b"GIF89a").unwrap();
    assert!(open(&root, &broken, vec![], Some(&(ffmpeg, ffprobe))).is_err());
    assert_eq!(fs::read(video).unwrap(), original);
    assert_eq!(
        captures_history::load(&root, chrono::Utc::now())
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn webm_preserve_mp4_transcodes_instead_of_copying_webm_bytes() {
    let Some((ffmpeg, ffprobe)) = tools() else {
        return;
    };
    let data = tempfile::tempdir().unwrap();
    let source = data.path().join("source.webm");
    fixture(&source, &ffmpeg);
    let tools = MediaToolchain::new(ffmpeg, ffprobe);
    let probe = tools.probe(&source).unwrap();
    let edit = EditSpec::default();
    let spec = ExportSpec {
        format: ExportFormat::Mp4,
        quality: QualityPreset::Preserve,
        max_size_bytes: None,
        frames_per_second: None,
        gif_max_colors: None,
    };
    assert!(!captures_media::export_preserves_source_bytes(
        &probe, &edit, &spec
    ));
    let output = data.path().join("output.mp4");
    tools
        .export(
            &source,
            &output,
            &edit,
            &spec,
            &CancelToken::default(),
            |_| {},
        )
        .unwrap();
    assert_eq!(
        tools.probe(&output).unwrap().metadata.mime_type,
        "video/mp4"
    );
    assert_ne!(fs::read(source).unwrap(), fs::read(output).unwrap());
}
