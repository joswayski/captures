use std::{fs, path::Path, process::Command, sync::Arc};

use captures_app::recording_editor::{
    RecordingEditorOpenRequest, RecordingEditorRequest, RecordingEditorSession,
    RecordingSaveRequest, SavedRecording,
};
use captures_history::{ArtifactKind, HistoryEntry};
use captures_media::{
    CancelToken, CropRect, EditSpec, ExportFormat, ExportSpec, ExportStage, MediaToolchain,
    QualityPreset,
};
use captures_recording::RecordingTarget;

fn tools() -> Option<MediaToolchain> {
    let tools = MediaToolchain::from_command_names();
    match tools.verify() {
        Ok(()) => Some(tools),
        Err(error) => {
            eprintln!("recording-editor FFmpeg tests skipped: {error}");
            None
        }
    }
}

fn create_recording(path: &Path) {
    let status = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "color=c=red:size=32x24:rate=10:duration=1",
            "-f",
            "lavfi",
            "-i",
            "color=c=green:size=32x24:rate=10:duration=1",
            "-f",
            "lavfi",
            "-i",
            "color=c=blue:size=32x24:rate=10:duration=1",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000:duration=3",
            "-filter_complex",
            "[0:v][1:v][2:v]concat=n=3:v=1:a=0,format=yuv420p[v]",
            "-map",
            "[v]",
            "-map",
            "3:a:0",
            "-c:v",
            "mpeg4",
            "-q:v",
            "2",
            "-c:a",
            "aac",
            "-shortest",
        ])
        .arg(path)
        .status()
        .expect("FFmpeg starts");
    assert!(status.success(), "asymmetric recording generated");
}

fn entry(id: &str, source: &Path) -> HistoryEntry {
    HistoryEntry {
        id: id.into(),
        kind: ArtifactKind::Video,
        preview_url: String::new(),
        full_url: String::new(),
        width: 32,
        height: 24,
        size_bytes: fs::metadata(source).unwrap().len(),
        created_at: "2026-09-22T00:00:00Z".into(),
        mode: None,
        saved_path: Some(source.to_string_lossy().into_owned()),
        mime_type: Some("video/mp4".into()),
        duration_ms: Some(3_000),
        target: Some(RecordingTarget::Display {
            display_id: "test-display".into(),
        }),
        has_system_audio: true,
        // Deliberately inconsistent with the one probed audio stream. Shared
        // source identity must not expose an independently editable mic track.
        has_microphone_audio: true,
        dropped_frames: 7,
    }
}

fn setup(retain_media: bool) -> Option<(tempfile::TempDir, HistoryEntry, MediaToolchain)> {
    let tools = tools()?;
    let data = tempfile::tempdir().unwrap();
    let source = data.path().join("source.mp4");
    create_recording(&source);
    let entry = entry(&uuid::Uuid::new_v4().to_string(), &source);
    let history = data.path().join("history");
    if retain_media {
        captures_history::save_recording(&history, &entry, b"poster", &source).unwrap();
    } else {
        captures_history::save_recording_reference(&history, &entry, b"poster").unwrap();
    }
    Some((data, entry, tools))
}

fn open(
    data: &tempfile::TempDir,
    entry: &HistoryEntry,
    tools: MediaToolchain,
) -> RecordingEditorSession {
    RecordingEditorSession::open(
        RecordingEditorOpenRequest {
            history_root: data.path().join("history"),
            artifact_id: entry.id.clone(),
        },
        tools,
    )
    .unwrap()
}

fn pixel(session: &RecordingEditorSession) -> [u8; 4] {
    session.frame().get_pixel(8, 8).0
}

fn assert_dominant(pixel: [u8; 4], channel: usize) {
    assert!(pixel[channel] > 90, "expected channel {channel}: {pixel:?}");
    for other in 0..3 {
        if other != channel {
            assert!(
                pixel[channel] > pixel[other] + 40,
                "expected channel {channel}: {pixel:?}"
            );
        }
    }
}

fn export_spec() -> ExportSpec {
    ExportSpec {
        format: ExportFormat::Mp4,
        quality: QualityPreset::Preserve,
        max_size_bytes: None,
        frames_per_second: None,
        gif_max_colors: None,
    }
}

#[test]
fn source_relative_scrubbing_and_edit_updates_are_atomic() {
    let Some((data, entry, tools)) = setup(true) else {
        return;
    };
    let mut session = open(&data, &entry, tools);
    assert_dominant(pixel(&session), 0);
    assert!(session.snapshot().has_system_audio);
    assert!(!session.snapshot().has_microphone_audio);

    session
        .execute(RecordingEditorRequest::Seek { position_ms: 1_200 })
        .unwrap();
    assert_dominant(pixel(&session), 1);
    session
        .execute(RecordingEditorRequest::Seek { position_ms: 2_200 })
        .unwrap();
    assert_dominant(pixel(&session), 2);
    let accepted = session.frame();
    let revision = session.snapshot().revision;
    for position_ms in [3_000, u64::MAX] {
        assert!(
            session
                .execute(RecordingEditorRequest::Seek { position_ms })
                .is_err()
        );
        assert_eq!(session.snapshot().revision, revision);
        assert!(Arc::ptr_eq(&accepted, &session.frame()));
    }

    let invalid_edits = [
        EditSpec {
            trim_start_ms: 1_000,
            trim_end_ms: Some(1_000),
            ..EditSpec::default()
        },
        EditSpec {
            trim_start_ms: 2_000,
            trim_end_ms: Some(1_000),
            ..EditSpec::default()
        },
        EditSpec {
            trim_start_ms: 0,
            trim_end_ms: Some(9_000),
            ..EditSpec::default()
        },
    ];
    for edit in invalid_edits {
        assert!(
            session
                .execute(RecordingEditorRequest::UpdateEdit { edit })
                .is_err()
        );
        assert_eq!(session.snapshot().revision, revision);
        assert!(Arc::ptr_eq(&accepted, &session.frame()));
    }

    let mut edit = EditSpec {
        trim_start_ms: 400,
        trim_end_ms: Some(2_600),
        crop: Some(CropRect {
            x: 4,
            y: 4,
            width: 24,
            height: 16,
        }),
        output_width: Some(16),
        output_height: Some(16),
        ..EditSpec::default()
    };
    // Caller audio identity is untrusted and must be overwritten.
    edit.audio.source_has_system_audio = false;
    edit.audio.source_has_microphone_audio = true;
    session
        .execute(RecordingEditorRequest::UpdateEdit { edit })
        .unwrap();
    assert_eq!(session.frame().dimensions(), (16, 16));
    assert!(session.snapshot().edit.audio.source_has_system_audio);
    assert!(!session.snapshot().edit.audio.source_has_microphone_audio);
    assert_eq!(session.snapshot().revision, revision + 1);
}

#[test]
fn save_new_publishes_trimmed_copy_history_and_never_changes_source() {
    let Some((data, entry, tools)) = setup(true) else {
        return;
    };
    let history = data.path().join("history");
    let source = entry.recording_media_path(&history).unwrap();
    let source_bytes = fs::read(&source).unwrap();
    let source_metadata = fs::read(
        captures_history::entry_directory(&history, &entry.id)
            .unwrap()
            .join(captures_history::HISTORY_METADATA_FILE),
    )
    .unwrap();
    let mut session = open(&data, &entry, tools.clone());
    session
        .execute(RecordingEditorRequest::UpdateEdit {
            edit: EditSpec {
                trim_start_ms: 500,
                trim_end_ms: Some(2_500),
                crop: Some(CropRect {
                    x: 4,
                    y: 4,
                    width: 24,
                    height: 16,
                }),
                output_width: Some(16),
                output_height: Some(16),
                ..EditSpec::default()
            },
        })
        .unwrap();
    let destination = data.path().join("saved/edited.mp4");
    let mut progress = Vec::new();
    let saved = session
        .save_new(
            RecordingSaveRequest {
                destination: destination.clone(),
                export: export_spec(),
            },
            &CancelToken::default(),
            |event| progress.push(event.stage),
        )
        .unwrap();
    let SavedRecording::Saved { artifact, path } = saved else {
        panic!("history publication should succeed")
    };
    assert_eq!(path, destination);
    assert_ne!(artifact.entry.id, entry.id);
    assert_eq!((artifact.entry.width, artifact.entry.height), (16, 16));
    assert_eq!(artifact.entry.saved_path.as_deref(), destination.to_str());
    assert_eq!(artifact.entry.dropped_frames, 7);
    assert!(artifact.preview_path.is_file());
    assert!(
        artifact
            .entry
            .recording_media_path(&history)
            .unwrap()
            .is_file()
    );
    assert_eq!(progress.first(), Some(&ExportStage::Preparing));
    assert_eq!(progress.last(), Some(&ExportStage::Complete));

    let output = tools.probe(&destination).unwrap();
    assert_eq!((output.metadata.width, output.metadata.height), (16, 16));
    assert!(
        output
            .metadata
            .duration_ms
            .is_some_and(|duration| (1_800..=2_200).contains(&duration)),
        "{:?}",
        output.metadata.duration_ms
    );
    let frame_path = data.path().join("export-frame.png");
    tools
        .extract_frame(&destination, 700, &frame_path, &CancelToken::default())
        .unwrap();
    let exported = image::open(frame_path).unwrap().into_rgba8();
    assert_dominant(exported.get_pixel(8, 8).0, 1);
    assert_eq!(fs::read(source).unwrap(), source_bytes);
    assert_eq!(
        fs::read(
            captures_history::entry_directory(&history, &entry.id)
                .unwrap()
                .join(captures_history::HISTORY_METADATA_FILE)
        )
        .unwrap(),
        source_metadata
    );
}

#[test]
fn collision_cancellation_and_post_publication_history_failure_are_recoverable() {
    let Some((data, entry, tools)) = setup(false) else {
        return;
    };
    let session = open(&data, &entry, tools);
    let collision = data.path().join("collision.mp4");
    fs::write(&collision, b"keep").unwrap();
    assert!(
        session
            .save_new(
                RecordingSaveRequest {
                    destination: collision.clone(),
                    export: export_spec(),
                },
                &CancelToken::default(),
                |_| {},
            )
            .is_err()
    );
    assert_eq!(fs::read(collision).unwrap(), b"keep");

    let cancelled = data.path().join("cancelled.mp4");
    let cancel = CancelToken::default();
    cancel.cancel();
    assert!(
        session
            .save_new(
                RecordingSaveRequest {
                    destination: cancelled.clone(),
                    export: export_spec(),
                },
                &cancel,
                |_| {},
            )
            .unwrap_err()
            .contains("cancelled")
    );
    assert!(!cancelled.exists());
    assert!(fs::read_dir(data.path()).unwrap().flatten().all(|entry| {
        !entry
            .file_name()
            .to_string_lossy()
            .starts_with(".captures-")
    }));

    // The opened source is external for a reference-only History item, so make
    // the History root unwritable without invalidating the immutable source.
    let history = data.path().join("history");
    fs::remove_dir_all(&history).unwrap();
    fs::write(&history, b"blocked").unwrap();
    let destination = data.path().join("published.mp4");
    let result = session
        .save_new(
            RecordingSaveRequest {
                destination: destination.clone(),
                export: export_spec(),
            },
            &CancelToken::default(),
            |_| {},
        )
        .unwrap();
    let SavedRecording::SavedWithoutHistory { path, warning } = result else {
        panic!("file publication must survive History failure")
    };
    assert_eq!(path, destination);
    assert!(path.is_file());
    assert!(!warning.is_empty());
}
