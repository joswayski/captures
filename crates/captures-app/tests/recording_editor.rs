use std::{
    fs,
    path::Path,
    process::Command,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

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

fn create_independent_audio_recording(path: &Path) {
    let status = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "color=c=red:size=32x24:rate=10:duration=3",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=330:sample_rate=48000:duration=3,aformat=channel_layouts=stereo",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000:duration=3,aformat=channel_layouts=stereo",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=880:sample_rate=48000:duration=3,aformat=channel_layouts=stereo",
            "-map",
            "0:v:0",
            "-map",
            "1:a:0",
            "-map",
            "2:a:0",
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
    assert!(status.success(), "independent-track recording generated");
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
    preview_spec(ExportFormat::Mp4, QualityPreset::Preserve)
}

fn preview_spec(format: ExportFormat, quality: QualityPreset) -> ExportSpec {
    ExportSpec {
        format,
        quality,
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
    assert_eq!(session.snapshot().preview_export, &export_spec());

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
fn preview_format_updates_serialize_and_roll_back_as_one_snapshot() {
    let Some((data, entry, tools)) = setup(true) else {
        return;
    };
    let mut session = open(&data, &entry, tools);
    let edit = EditSpec {
        output_width: Some(17),
        output_height: Some(17),
        ..EditSpec::default()
    };
    let gif = preview_spec(ExportFormat::Gif, QualityPreset::Preserve);
    let request: RecordingEditorRequest = serde_json::from_value(serde_json::json!({
        "operation": "update_preview",
        "edit": edit,
        "export": gif,
    }))
    .unwrap();
    session.execute(request).unwrap();
    assert_eq!(session.frame().dimensions(), (16, 16));
    assert_eq!(session.snapshot().preview_export, &gif);
    assert!(session.snapshot().edit.audio.source_has_system_audio);
    assert!(!session.snapshot().edit.audio.source_has_microphone_audio);
    let accepted = serde_json::to_value(session.snapshot()).unwrap();
    assert_eq!(accepted["preview_export"]["format"], "gif");
    let frame = session.frame();

    for unsupported in [
        preview_spec(ExportFormat::WebM, QualityPreset::Preserve),
        ExportSpec {
            max_size_bytes: Some(1_000_000),
            ..export_spec()
        },
    ] {
        assert!(
            session
                .execute(RecordingEditorRequest::UpdatePreview {
                    edit: EditSpec {
                        output_width: Some(20),
                        output_height: Some(18),
                        ..EditSpec::default()
                    },
                    export: unsupported,
                })
                .is_err()
        );
        assert_eq!(serde_json::to_value(session.snapshot()).unwrap(), accepted);
        assert!(Arc::ptr_eq(&frame, &session.frame()));
    }

    session
        .execute(RecordingEditorRequest::UpdateEdit {
            edit: EditSpec {
                output_width: Some(19),
                output_height: Some(17),
                ..EditSpec::default()
            },
        })
        .unwrap();
    assert_eq!(session.frame().dimensions(), (18, 16));
    assert_eq!(session.snapshot().preview_export, &gif);
    session
        .execute(RecordingEditorRequest::Seek { position_ms: 1_200 })
        .unwrap();
    assert_eq!(session.snapshot().preview_export, &gif);

    let before_failure = serde_json::to_value(session.snapshot()).unwrap();
    let retained = session.frame();
    let history = data.path().join("history");
    fs::remove_file(entry.recording_media_path(&history).unwrap()).unwrap();
    assert!(
        session
            .execute(RecordingEditorRequest::UpdatePreview {
                edit: EditSpec::default(),
                export: export_spec(),
            })
            .is_err()
    );
    assert_eq!(
        serde_json::to_value(session.snapshot()).unwrap(),
        before_failure
    );
    assert!(Arc::ptr_eq(&retained, &session.frame()));
}

#[test]
fn estimating_accepted_preview_is_read_only() {
    let Some((data, entry, tools)) = setup(true) else {
        return;
    };
    let history = data.path().join("history");
    let source = entry.recording_media_path(&history).unwrap();
    let source_bytes = fs::read(&source).unwrap();
    let mut session = open(&data, &entry, tools);
    let gif = preview_spec(ExportFormat::Gif, QualityPreset::Preserve);
    session
        .execute(RecordingEditorRequest::UpdatePreview {
            edit: EditSpec {
                trim_start_ms: 500,
                trim_end_ms: Some(2_500),
                output_width: Some(16),
                output_height: Some(12),
                ..EditSpec::default()
            },
            export: gif,
        })
        .unwrap();
    let snapshot = serde_json::to_value(session.snapshot()).unwrap();
    let frame = session.frame();
    let history_entries = fs::read_dir(&history).unwrap().count();

    let estimate = session.estimate_export(&CancelToken::default()).unwrap();

    assert!(estimate.exact);
    assert!(estimate.size_bytes > 0);
    assert_eq!(
        serde_json::to_value(estimate).unwrap(),
        serde_json::json!({"size_bytes": estimate.size_bytes, "exact": true})
    );
    assert_eq!(serde_json::to_value(session.snapshot()).unwrap(), snapshot);
    assert!(Arc::ptr_eq(&frame, &session.frame()));
    assert_eq!(fs::read_dir(&history).unwrap().count(), history_entries);
    assert_eq!(fs::read(source).unwrap(), source_bytes);
}

#[test]
fn timeline_thumbnails_span_the_immutable_source_without_changing_session_state() {
    let Some((data, entry, tools)) = setup(true) else {
        return;
    };
    let history = data.path().join("history");
    let source = entry.recording_media_path(&history).unwrap();
    let source_bytes = fs::read(&source).unwrap();
    let mut session = open(&data, &entry, tools);
    session
        .execute(RecordingEditorRequest::UpdatePreview {
            edit: EditSpec {
                trim_start_ms: 1_100,
                trim_end_ms: Some(2_300),
                crop: Some(CropRect {
                    x: 4,
                    y: 4,
                    width: 20,
                    height: 14,
                }),
                output_width: Some(18),
                output_height: Some(12),
                ..EditSpec::default()
            },
            export: preview_spec(ExportFormat::Gif, QualityPreset::Standard),
        })
        .unwrap();
    session
        .execute(RecordingEditorRequest::Seek { position_ms: 1_500 })
        .unwrap();
    let snapshot = serde_json::to_value(session.snapshot()).unwrap();
    let frame = session.frame();
    let history_entries = fs::read_dir(&history).unwrap().count();

    let thumbnails = session
        .timeline_thumbnails(&CancelToken::default())
        .unwrap();

    assert_eq!(
        (
            thumbnails.frame_count,
            thumbnails.frame_width,
            thumbnails.frame_height,
            thumbnails.sprite_width,
            thumbnails.sprite_height,
        ),
        (12, 160, 90, 1_920, 90)
    );
    assert_eq!(thumbnails.pixels().dimensions(), (1_920, 90));
    assert_dominant(thumbnails.pixels().get_pixel(80, 45).0, 0);
    assert_dominant(thumbnails.pixels().get_pixel(1_840, 45).0, 2);
    assert_eq!(serde_json::to_value(session.snapshot()).unwrap(), snapshot);
    assert!(Arc::ptr_eq(&frame, &session.frame()));
    assert_eq!(fs::read_dir(&history).unwrap().count(), history_entries);
    assert_eq!(fs::read(&source).unwrap(), source_bytes);

    let cancelled = CancelToken::default();
    cancelled.cancel();
    assert!(session.timeline_thumbnails(&cancelled).is_err());
    assert_eq!(serde_json::to_value(session.snapshot()).unwrap(), snapshot);
    assert!(Arc::ptr_eq(&frame, &session.frame()));
    assert_eq!(fs::read(&source).unwrap(), source_bytes);

    fs::remove_file(source).unwrap();
    assert!(
        session
            .timeline_thumbnails(&CancelToken::default())
            .is_err()
    );
    assert_eq!(serde_json::to_value(session.snapshot()).unwrap(), snapshot);
    assert!(Arc::ptr_eq(&frame, &session.frame()));
}

#[test]
fn playback_uses_accepted_spatial_preview_trim_and_retains_session_state() {
    let Some((data, entry, tools)) = setup(true) else {
        return;
    };
    let source = entry
        .recording_media_path(&data.path().join("history"))
        .unwrap();
    let source_before = fs::read(&source).unwrap();
    let mut session = open(&data, &entry, tools);
    let edit = EditSpec {
        trim_start_ms: 1_100,
        trim_end_ms: Some(2_300),
        crop: Some(CropRect {
            x: 3,
            y: 2,
            width: 21,
            height: 17,
        }),
        output_width: Some(1_600),
        output_height: Some(400),
        ..EditSpec::default()
    };
    session
        .execute(RecordingEditorRequest::UpdatePreview {
            edit,
            export: preview_spec(ExportFormat::Gif, QualityPreset::Standard),
        })
        .unwrap();
    let snapshot_before = serde_json::to_value(session.snapshot()).unwrap();
    let accepted_frame = session.frame();
    let cancel = CancelToken::default();
    let mut playback = session.playback(2_999, &cancel).unwrap();
    assert_eq!(playback.start_position_ms(), 1_100);
    assert_eq!((playback.width(), playback.height()), (1_280, 320));
    assert_eq!(playback.frames_per_second(), 15);

    let mut frames = Vec::new();
    while let Some(frame) = playback.next_frame().unwrap() {
        assert!(frame.position_ms >= 1_100 && frame.position_ms < 2_300);
        assert_eq!(frame.pixels().dimensions(), (1_280, 320));
        frames.push(frame);
    }
    assert!(!cancel.is_cancelled(), "natural EOF preserves caller token");
    assert!(
        frames.len() >= 12,
        "persistent decoder returned temporal frames"
    );
    assert_dominant(frames.first().unwrap().pixels().get_pixel(8, 8).0, 1);
    assert_dominant(frames.last().unwrap().pixels().get_pixel(8, 8).0, 2);
    drop(playback);

    assert_eq!(
        serde_json::to_value(session.snapshot()).unwrap(),
        snapshot_before
    );
    assert!(Arc::ptr_eq(&session.frame(), &accepted_frame));
    assert_eq!(fs::read(source).unwrap(), source_before);

    let slow = ExportSpec {
        frames_per_second: Some(2),
        ..preview_spec(ExportFormat::Gif, QualityPreset::Standard)
    };
    let edit = session.snapshot().edit.clone();
    session
        .execute(RecordingEditorRequest::UpdatePreview { edit, export: slow })
        .unwrap();
    let cancel = CancelToken::default();
    let mut playback = session.playback(1_100, &cancel).unwrap();
    assert!(playback.next_frame().unwrap().is_some());
    let cancellation = cancel.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(50));
        cancellation.cancel();
    });
    let started = Instant::now();
    let error = match playback.next_frame() {
        Ok(_) => panic!("cancelled playback must fail"),
        Err(error) => error,
    };
    assert!(error.contains("cancelled"));
    assert!(started.elapsed() < Duration::from_millis(500));
}

#[test]
fn real_exports_and_previews_share_format_specific_dimensions() {
    let Some(tools) = tools() else {
        return;
    };
    let data = tempfile::tempdir().unwrap();
    let source = data.path().join("oversized-source.mp4");
    let status = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "color=c=red:size=4000x2200:rate=1:duration=1",
            "-c:v",
            "mpeg4",
            "-q:v",
            "2",
            "-an",
        ])
        .arg(&source)
        .status()
        .expect("FFmpeg starts");
    assert!(status.success(), "oversized recording generated");
    let mut entry = entry(&uuid::Uuid::new_v4().to_string(), &source);
    entry.width = 4_000;
    entry.height = 2_200;
    entry.duration_ms = Some(1_000);
    entry.has_system_audio = false;
    entry.has_microphone_audio = false;
    let history = data.path().join("history");
    captures_history::save_recording(&history, &entry, b"poster", &source).unwrap();
    let retained_source = entry.recording_media_path(&history).unwrap();
    let source_bytes = fs::read(&retained_source).unwrap();
    let mut session = open(&data, &entry, tools.clone());

    // Preserve MP4 takes the copy/remux path, so its preview must not apply the
    // software encoder's re-encode ceiling.
    assert_eq!(session.frame().dimensions(), (4_000, 2_200));
    let preserve_path = data.path().join("preserved.mp4");
    session
        .save_new(
            RecordingSaveRequest {
                destination: preserve_path.clone(),
                export: export_spec(),
            },
            &CancelToken::default(),
            |_| {},
        )
        .unwrap();
    assert_eq!(fs::read(preserve_path).unwrap(), source_bytes);

    let standard = preview_spec(ExportFormat::Mp4, QualityPreset::Standard);
    let custom_size = EditSpec {
        output_width: Some(4_001),
        output_height: Some(601),
        ..EditSpec::default()
    };
    session
        .execute(RecordingEditorRequest::UpdatePreview {
            edit: custom_size.clone(),
            export: standard.clone(),
        })
        .unwrap();
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    let expected_mp4 = (3_840, 576);
    #[cfg(target_os = "macos")]
    let expected_mp4 = (4_000, 600);
    assert_eq!(session.frame().dimensions(), expected_mp4);
    let mp4_path = data.path().join("standard.mp4");
    session
        .save_new(
            RecordingSaveRequest {
                destination: mp4_path.clone(),
                export: standard,
            },
            &CancelToken::default(),
            |_| {},
        )
        .unwrap();
    let mp4 = tools.probe(&mp4_path).unwrap().metadata;
    assert_eq!((mp4.width, mp4.height), expected_mp4);

    let gif = preview_spec(ExportFormat::Gif, QualityPreset::Preserve);
    session
        .execute(RecordingEditorRequest::UpdatePreview {
            edit: custom_size,
            export: gif.clone(),
        })
        .unwrap();
    assert_eq!(session.frame().dimensions(), (4_000, 600));
    assert_dominant(session.frame().get_pixel(2_000, 300).0, 0);
    let gif_path = data.path().join("custom.gif");
    session
        .save_new(
            RecordingSaveRequest {
                destination: gif_path.clone(),
                export: gif,
            },
            &CancelToken::default(),
            |_| {},
        )
        .unwrap();
    let gif = tools.probe(&gif_path).unwrap().metadata;
    assert_eq!((gif.width, gif.height), (4_000, 600));

    // Automatic GIF sizing remains width-capped with FFmpeg's aspect-aware -2
    // height. Keep an odd asymmetric crop so this compares actual filter output,
    // not dimensions inferred from the even attempt fields.
    let automatic_gif_edit = EditSpec {
        crop: Some(CropRect {
            x: 10,
            y: 10,
            width: 91,
            height: 160,
        }),
        ..EditSpec::default()
    };
    let automatic_gif = preview_spec(ExportFormat::Gif, QualityPreset::Preserve);
    session
        .execute(RecordingEditorRequest::UpdatePreview {
            edit: automatic_gif_edit,
            export: automatic_gif.clone(),
        })
        .unwrap();
    let automatic_preview_dimensions = session.frame().dimensions();
    assert_eq!(automatic_preview_dimensions.0, 90);
    let automatic_gif_path = data.path().join("automatic.gif");
    session
        .save_new(
            RecordingSaveRequest {
                destination: automatic_gif_path.clone(),
                export: automatic_gif,
            },
            &CancelToken::default(),
            |_| {},
        )
        .unwrap();
    let automatic_gif = tools.probe(&automatic_gif_path).unwrap().metadata;
    assert_eq!(
        (automatic_gif.width, automatic_gif.height),
        automatic_preview_dimensions
    );
    assert_eq!(fs::read(retained_source).unwrap(), source_bytes);
}

#[test]
fn unchanged_gif_requested_as_mp4_is_reencoded_not_copied() {
    let Some(tools) = tools() else {
        return;
    };
    let data = tempfile::tempdir().unwrap();
    let source = data.path().join("source.gif");
    let status = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "color=c=blue:size=32x24:rate=2:duration=1",
            "-loop",
            "0",
        ])
        .arg(&source)
        .status()
        .expect("FFmpeg starts");
    assert!(status.success(), "GIF recording generated");
    let mut entry = entry(&uuid::Uuid::new_v4().to_string(), &source);
    entry.kind = ArtifactKind::Gif;
    entry.mime_type = Some("image/gif".into());
    entry.duration_ms = Some(1_000);
    entry.has_system_audio = false;
    entry.has_microphone_audio = false;
    let history = data.path().join("history");
    captures_history::save_recording(&history, &entry, b"poster", &source).unwrap();
    let retained_source = entry.recording_media_path(&history).unwrap();
    let source_bytes = fs::read(&retained_source).unwrap();
    let session = open(&data, &entry, tools.clone());
    assert_eq!(session.snapshot().preview_export, &export_spec());
    assert_eq!(session.frame().dimensions(), (32, 24));

    let destination = data.path().join("converted.mp4");
    session
        .save_new(
            RecordingSaveRequest {
                destination: destination.clone(),
                export: export_spec(),
            },
            &CancelToken::default(),
            |_| {},
        )
        .unwrap();
    let converted = fs::read(&destination).unwrap();
    assert_ne!(converted, source_bytes);
    assert!(!converted.starts_with(b"GIF8"));
    let output = tools.probe(&destination).unwrap().metadata;
    assert_eq!(output.kind, captures_media::MediaKind::Video);
    assert_eq!((output.width, output.height), (32, 24));
    assert_eq!(fs::read(retained_source).unwrap(), source_bytes);
}

#[test]
fn preserved_independent_audio_tracks_reopen_without_inventing_reencoded_tracks() {
    let Some(tools) = tools() else {
        return;
    };
    let data = tempfile::tempdir().unwrap();
    let source = data.path().join("independent-audio.mp4");
    create_independent_audio_recording(&source);
    let entry = entry(&uuid::Uuid::new_v4().to_string(), &source);
    let history = data.path().join("history");
    captures_history::save_recording(&history, &entry, b"poster", &source).unwrap();
    let retained_source = entry.recording_media_path(&history).unwrap();
    let source_bytes = fs::read(&retained_source).unwrap();
    let mut session = open(&data, &entry, tools.clone());
    assert!(session.snapshot().has_system_audio);
    assert!(session.snapshot().has_microphone_audio);

    let preserved_path = data.path().join("preserved.mp4");
    let preserved = session
        .save_new(
            RecordingSaveRequest {
                destination: preserved_path.clone(),
                export: export_spec(),
            },
            &CancelToken::default(),
            |_| {},
        )
        .unwrap();
    let SavedRecording::Saved {
        artifact: preserved,
        ..
    } = preserved
    else {
        panic!("preserved copy should publish History")
    };
    assert_eq!(fs::read(&preserved_path).unwrap(), source_bytes);
    assert_eq!(tools.probe(&preserved_path).unwrap().audio_stream_count, 3);
    assert!(preserved.entry.has_system_audio);
    assert!(preserved.entry.has_microphone_audio);
    let reopened = open(&data, &preserved.entry, tools.clone());
    assert!(reopened.snapshot().has_system_audio);
    assert!(reopened.snapshot().has_microphone_audio);
    assert!(reopened.snapshot().edit.audio.source_has_system_audio);
    assert!(reopened.snapshot().edit.audio.source_has_microphone_audio);

    let trimmed = EditSpec {
        trim_start_ms: 100,
        trim_end_ms: Some(2_900),
        ..EditSpec::default()
    };
    session
        .execute(RecordingEditorRequest::UpdateEdit {
            edit: trimmed.clone(),
        })
        .unwrap();
    let mixed_path = data.path().join("mixed.mp4");
    let mixed = session
        .save_new(
            RecordingSaveRequest {
                destination: mixed_path.clone(),
                export: export_spec(),
            },
            &CancelToken::default(),
            |_| {},
        )
        .unwrap();
    let SavedRecording::Saved {
        artifact: mixed, ..
    } = mixed
    else {
        panic!("mixed re-encode should publish History")
    };
    assert_eq!(tools.probe(&mixed_path).unwrap().audio_stream_count, 1);
    assert!(mixed.entry.has_system_audio);
    assert!(!mixed.entry.has_microphone_audio);
    let reopened = open(&data, &mixed.entry, tools.clone());
    assert!(reopened.snapshot().has_system_audio);
    assert!(!reopened.snapshot().has_microphone_audio);

    let mut microphone_only = trimmed;
    microphone_only.audio.mute_system_audio = true;
    session
        .execute(RecordingEditorRequest::UpdateEdit {
            edit: microphone_only,
        })
        .unwrap();
    let microphone_path = data.path().join("microphone.mp4");
    let microphone = session
        .save_new(
            RecordingSaveRequest {
                destination: microphone_path.clone(),
                export: export_spec(),
            },
            &CancelToken::default(),
            |_| {},
        )
        .unwrap();
    let SavedRecording::Saved {
        artifact: microphone,
        ..
    } = microphone
    else {
        panic!("microphone-only re-encode should publish History")
    };
    assert_eq!(tools.probe(&microphone_path).unwrap().audio_stream_count, 1);
    assert!(!microphone.entry.has_system_audio);
    assert!(microphone.entry.has_microphone_audio);
    let reopened = open(&data, &microphone.entry, tools);
    assert!(!reopened.snapshot().has_system_audio);
    assert!(reopened.snapshot().has_microphone_audio);
    assert_eq!(fs::read(retained_source).unwrap(), source_bytes);
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
