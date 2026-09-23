#[cfg(unix)]
use std::sync::mpsc;
use std::{
    fs,
    io::{Seek, SeekFrom, Write},
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
    let tools = MediaToolchain::new(
        std::env::var_os("CAPTURES_TEST_FFMPEG")
            .unwrap_or_else(|| "ffmpeg".into())
            .into(),
        std::env::var_os("CAPTURES_TEST_FFPROBE")
            .unwrap_or_else(|| "ffprobe".into())
            .into(),
    );
    match tools.verify() {
        Ok(()) => Some(tools),
        Err(error) => {
            eprintln!("recording-editor FFmpeg tests skipped: {error}");
            None
        }
    }
}

fn create_recording(path: &Path) {
    let status = Command::new(
        std::env::var_os("CAPTURES_TEST_FFMPEG").unwrap_or_else(|| "ffmpeg".into()),
    )
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
            "[0:v][1:v][2:v]concat=n=3:v=1:a=0,drawbox=x=28:y=0:w=4:h=24:color=white:t=fill,format=yuv420p[v]",
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
fn replace_original_rebases_session_without_changing_identity_or_old_frame() {
    let Some((data, entry, tools)) = setup(true) else {
        return;
    };
    let permanent = data.path().join("source.mp4");
    let original = fs::read(&permanent).unwrap();
    let mut session = open(&data, &entry, tools);
    let retained = session.frame();
    let mut edit = EditSpec {
        trim_start_ms: 1_000,
        trim_end_ms: Some(2_000),
        output_width: Some(32),
        output_height: Some(16),
        ..EditSpec::default()
    };
    edit.audio.source_has_system_audio = true;
    session
        .execute(RecordingEditorRequest::UpdateEdit { edit })
        .unwrap();
    let revision = session.snapshot().revision;
    let replaced = session
        .replace_original(&CancelToken::default(), |_| {})
        .unwrap();
    assert!(matches!(
        replaced,
        captures_app::recording_editor::ReplacedRecording::Replaced { .. }
    ));
    assert_ne!(fs::read(&permanent).unwrap(), original);
    assert_eq!(
        fs::read(
            data.path()
                .join("history")
                .join(&entry.id)
                .join("media.mp4")
        )
        .unwrap(),
        fs::read(&permanent).unwrap()
    );
    let snapshot = session.snapshot_v2();
    assert_eq!(snapshot.editor.artifact_id, entry.id);
    assert_eq!(snapshot.editor.position_ms, 0);
    assert_eq!(snapshot.editor.revision, revision + 1);
    assert_eq!(
        (snapshot.editor.source.width, snapshot.editor.source.height),
        (32, 16)
    );
    assert_eq!(snapshot.editor.edit.trim_start_ms, 0);
    assert_eq!(snapshot.save_export.max_size_bytes, None);
    assert_eq!(retained.dimensions(), (32, 24));
    assert_dominant(retained.get_pixel(8, 8).0, 0);
    session
        .execute(RecordingEditorRequest::Seek { position_ms: 500 })
        .unwrap();
    assert_dominant(pixel(&session), 1);
    let mut playback = session.playback(0, &CancelToken::default()).unwrap();
    assert!(playback.next_frame().unwrap().is_some());
    let copy = data.path().join("copy.mp4");
    session
        .save_new(
            RecordingSaveRequest {
                destination: copy.clone(),
                export: export_spec(),
            },
            &CancelToken::default(),
            |_| {},
        )
        .unwrap();
    assert!(copy.is_file());
}

#[test]
fn replace_original_gif_preserves_format_and_disables_audio() {
    let Some((data, source_entry, tools)) = setup(true) else {
        return;
    };
    let permanent = data.path().join("source.gif");
    let status = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(data.path().join("source.mp4"))
        .args(["-an", "-vf", "fps=10"])
        .arg(&permanent)
        .status()
        .unwrap();
    assert!(status.success());
    let mut entry = entry(&uuid::Uuid::new_v4().to_string(), &permanent);
    entry.kind = ArtifactKind::Gif;
    entry.mime_type = Some("image/gif".into());
    entry.has_system_audio = false;
    entry.has_microphone_audio = false;
    captures_history::save_recording(&data.path().join("history"), &entry, b"poster", &permanent)
        .unwrap();
    let mut session = open(&data, &entry, tools);
    session
        .execute(RecordingEditorRequest::UpdatePreview {
            edit: EditSpec {
                trim_start_ms: 1_000,
                trim_end_ms: Some(2_000),
                ..EditSpec::default()
            },
            export: preview_spec(ExportFormat::Gif, QualityPreset::Preserve),
        })
        .unwrap();
    session
        .replace_original(&CancelToken::default(), |_| {})
        .unwrap();
    assert_eq!(session.snapshot_v2().save_export.format, ExportFormat::Gif);
    assert_eq!(session.snapshot().source.mime_type, "image/gif");
    assert!(!session.snapshot().has_system_audio);
    assert_eq!(session.snapshot().position_ms, 0);
    assert_eq!(session.snapshot().revision, 2);
    assert_eq!(
        fs::read(&permanent).unwrap(),
        fs::read(
            data.path()
                .join("history")
                .join(&entry.id)
                .join("media.gif")
        )
        .unwrap()
    );
    assert_eq!(
        fs::read(data.path().join("source.mp4")).unwrap().len() as u64,
        source_entry.size_bytes
    );
}

#[test]
fn replace_original_rejects_reference_and_cancel_without_publication() {
    let Some((data, entry, tools)) = setup(false) else {
        return;
    };
    let permanent = data.path().join("source.mp4");
    let before = fs::read(&permanent).unwrap();
    let mut session = open(&data, &entry, tools);
    let error = session
        .replace_original(&CancelToken::default(), |_| {})
        .unwrap_err();
    assert!(!error.requires_reopen);
    assert_eq!(fs::read(&permanent).unwrap(), before);

    let Some((data, entry, tools)) = setup(true) else {
        return;
    };
    let permanent = data.path().join("source.mp4");
    let before = fs::read(&permanent).unwrap();
    let mut session = open(&data, &entry, tools);
    let frame = session.frame();
    let cancel = CancelToken::default();
    cancel.cancel();
    let error = session.replace_original(&cancel, |_| {}).unwrap_err();
    assert!(!error.requires_reopen);
    assert_eq!(fs::read(&permanent).unwrap(), before);
    assert!(Arc::ptr_eq(&frame, &session.frame()));
    assert_eq!(session.snapshot().revision, 0);
    assert!(!session.requires_reopen());
}

#[test]
fn replace_original_rejects_history_only_permanent_hint() {
    let Some((data, mut entry, tools)) = setup(true) else {
        return;
    };
    let original = data.path().join("source.mp4");
    let before = fs::read(&original).unwrap();
    let recovery = data
        .path()
        .join("history")
        .join(&entry.id)
        .join("media.mp4");
    entry.saved_path = Some(recovery.to_string_lossy().into_owned());
    captures_history::update_metadata(&data.path().join("history"), &entry).unwrap();
    let mut session = open(&data, &entry, tools);
    let error = session
        .replace_original(&CancelToken::default(), |_| {})
        .unwrap_err();
    assert!(!error.requires_reopen);
    assert_eq!(fs::read(&original).unwrap(), before);
    assert_eq!(fs::read(&recovery).unwrap(), before);
    assert_eq!(session.snapshot().revision, 0);
}

#[test]
fn original_save_path_is_the_session_metadata_hint_not_a_replace_eligibility_claim() {
    let Some((data, mut entry, tools)) = setup(true) else {
        return;
    };
    let original = data.path().join("source.mp4");
    let session = open(&data, &entry, tools.clone());
    assert_eq!(session.original_save_path(), Some(original.as_path()));
    let snapshot_keys = serde_json::to_value(session.snapshot())
        .unwrap()
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    entry.saved_path = None;
    captures_history::update_metadata(&data.path().join("history"), &entry).unwrap();
    let mut no_path = open(&data, &entry, tools);
    assert_eq!(no_path.original_save_path(), None);
    assert_eq!(
        serde_json::to_value(no_path.snapshot())
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>(),
        snapshot_keys
    );
    assert!(
        !no_path
            .replace_original(&CancelToken::default(), |_| {})
            .unwrap_err()
            .requires_reopen
    );
}

#[test]
fn replace_original_rejects_changed_source_and_metadata_before_publication() {
    let Some((data, entry, tools)) = setup(true) else {
        return;
    };
    let permanent = data.path().join("source.mp4");
    let mut session = open(&data, &entry, tools);
    let metadata = data
        .path()
        .join("history")
        .join(&entry.id)
        .join("metadata.json");
    let original_metadata = fs::read(&metadata).unwrap();
    fs::write(&metadata, b"{}").unwrap();
    let error = session
        .replace_original(&CancelToken::default(), |_| {})
        .unwrap_err();
    assert!(!error.requires_reopen);
    fs::write(&metadata, original_metadata).unwrap();
    let old = fs::read(&permanent).unwrap();
    fs::write(&permanent, b"different bytes").unwrap();
    let error = session
        .replace_original(&CancelToken::default(), |_| {})
        .unwrap_err();
    assert!(!error.requires_reopen);
    fs::write(&permanent, old).unwrap();
    assert!(!session.requires_reopen());
}

#[test]
fn replace_original_cancellation_during_export_keeps_original_and_history() {
    let Some((data, entry, tools)) = setup(true) else {
        return;
    };
    let permanent = data.path().join("source.mp4");
    let recovery = data
        .path()
        .join("history")
        .join(&entry.id)
        .join("media.mp4");
    let old = fs::read(&permanent).unwrap();
    let metadata = fs::read(
        data.path()
            .join("history")
            .join(&entry.id)
            .join("metadata.json"),
    )
    .unwrap();
    let mut session = open(&data, &entry, tools);
    let cancel = CancelToken::default();
    let mut progress_count = 0;
    let error = session
        .replace_original(&cancel, |_| {
            progress_count += 1;
            cancel.cancel();
        })
        .unwrap_err();
    assert!(progress_count > 0);
    assert!(!error.requires_reopen);
    assert_eq!(fs::read(&permanent).unwrap(), old);
    assert_eq!(fs::read(&recovery).unwrap(), old);
    assert_eq!(
        fs::read(
            data.path()
                .join("history")
                .join(&entry.id)
                .join("metadata.json")
        )
        .unwrap(),
        metadata
    );
    assert_eq!(session.snapshot().revision, 0);
    assert!(fs::read_dir(data.path()).unwrap().all(|item| {
        !item
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".captures-replace-")
    }));
}

#[cfg(unix)]
#[test]
fn replace_original_cancellation_during_candidate_frame_kills_child_and_keeps_state() {
    let Some((data, entry, _)) = setup(true) else {
        return;
    };
    let permanent = data.path().join("source.mp4");
    let directory = data.path().join("history").join(&entry.id);
    let recovery = directory.join("media.mp4");
    let metadata = directory.join("metadata.json");
    let before_permanent = fs::read(&permanent).unwrap();
    let before_recovery = fs::read(&recovery).unwrap();
    let before_metadata = fs::read(&metadata).unwrap();
    let enabled = data.path().join("enable-candidate-gate");
    let marker = data.path().join("candidate-started");
    let release = data.path().join("release-candidate");
    let wrapper = data.path().join("ffmpeg-wrapper");
    let real_ffmpeg = std::env::var_os("CAPTURES_TEST_FFMPEG")
        .unwrap_or_else(|| "ffmpeg".into())
        .to_string_lossy()
        .into_owned();
    fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nif [ -e {enabled:?} ]; then\n  for arg in \"$@\"; do\n    case \"$arg\" in\n      *frame-*.png)\n        : > {marker:?}\n        while [ ! -e {release:?} ]; do :; done\n        ;;\n    esac\n  done\nfi\nexec {real_ffmpeg:?} \"$@\"\n",
            enabled = enabled.to_string_lossy().as_ref(),
            marker = marker.to_string_lossy().as_ref(),
            release = release.to_string_lossy().as_ref(),
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();

    let mut session = open(
        &data,
        &entry,
        MediaToolchain::new(
            wrapper,
            std::env::var_os("CAPTURES_TEST_FFPROBE")
                .unwrap_or_else(|| "ffprobe".into())
                .into(),
        ),
    );
    let before_snapshot = serde_json::to_value(session.snapshot_v2()).unwrap();
    let before_frame = session.frame();
    fs::write(&enabled, b"").unwrap();
    let cancel = CancelToken::default();
    let worker_cancel = cancel.clone();
    let (tx, rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        let result = session.replace_original(&worker_cancel, |_| {});
        tx.send(()).unwrap();
        (session, result)
    });
    let deadline = Instant::now() + Duration::from_secs(15);
    while !marker.exists() && Instant::now() < deadline {
        if rx.try_recv().is_ok() {
            let (_, result) = worker.join().unwrap();
            panic!("replacement ended before candidate decode: {result:?}");
        }
        thread::sleep(Duration::from_millis(10));
    }
    if !marker.exists() {
        fs::write(&release, b"").unwrap();
        worker.join().unwrap();
        panic!("candidate-frame child did not start");
    }
    let started = Instant::now();
    cancel.cancel();
    let prompt = rx.recv_timeout(Duration::from_secs(2)).is_ok();
    // Even a broken non-cancellable child must be released before joining it.
    fs::write(&release, b"").unwrap();
    let (session, result) = worker.join().unwrap();
    assert!(prompt, "candidate-frame child did not stop promptly");
    assert!(started.elapsed() < Duration::from_secs(3));
    let error = result.unwrap_err();
    assert!(!error.requires_reopen);
    assert!(error.message.to_lowercase().contains("cancel"));
    assert_eq!(fs::read(&permanent).unwrap(), before_permanent);
    assert_eq!(fs::read(&recovery).unwrap(), before_recovery);
    assert_eq!(fs::read(&metadata).unwrap(), before_metadata);
    assert_eq!(
        serde_json::to_value(session.snapshot_v2()).unwrap(),
        before_snapshot
    );
    assert_eq!(session.frame().as_raw(), before_frame.as_raw());
    assert!(fs::read_dir(data.path()).unwrap().all(|item| {
        !item
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".captures-replace-")
    }));
}

#[test]
fn replace_original_detects_in_place_permanent_write_during_encode() {
    let Some((data, entry, tools)) = setup(true) else {
        return;
    };
    let permanent = data.path().join("source.mp4");
    let recovery = data
        .path()
        .join("history")
        .join(&entry.id)
        .join("media.mp4");
    let old = fs::read(&permanent).unwrap();
    let metadata = fs::read(
        data.path()
            .join("history")
            .join(&entry.id)
            .join("metadata.json"),
    )
    .unwrap();
    let mut session = open(&data, &entry, tools);
    let frame = session.frame();
    let mut mutated = false;
    let error = session
        .replace_original(&CancelToken::default(), |_| {
            if !mutated {
                let mut file = fs::OpenOptions::new().write(true).open(&permanent).unwrap();
                file.seek(SeekFrom::Start(100)).unwrap();
                file.write_all(&[old[100] ^ 0xff]).unwrap();
                mutated = true;
            }
        })
        .unwrap_err();
    assert!(mutated);
    assert!(!error.requires_reopen);
    assert_eq!(fs::read(&permanent).unwrap()[100], old[100] ^ 0xff);
    assert_eq!(fs::read(&recovery).unwrap(), old);
    assert_eq!(
        fs::read(
            data.path()
                .join("history")
                .join(&entry.id)
                .join("metadata.json")
        )
        .unwrap(),
        metadata
    );
    assert_eq!(session.snapshot().revision, 0);
    assert!(Arc::ptr_eq(&frame, &session.frame()));
}

#[test]
fn replace_original_rejects_identical_rewrite_of_opened_source() {
    let Some((data, entry, tools)) = setup(true) else {
        return;
    };
    let permanent = data.path().join("source.mp4");
    let recovery = data
        .path()
        .join("history")
        .join(&entry.id)
        .join("media.mp4");
    let mut session = open(&data, &entry, tools);
    let original = fs::read(&recovery).unwrap();
    // Retains bytes and inode, but changes the source's modification/change
    // metadata after the preview was accepted.
    thread::sleep(Duration::from_millis(20));
    let mut file = fs::OpenOptions::new().write(true).open(&recovery).unwrap();
    file.seek(SeekFrom::Start(100)).unwrap();
    file.write_all(&original[100..101]).unwrap();
    file.sync_all().unwrap();
    let mut file = fs::OpenOptions::new().write(true).open(&permanent).unwrap();
    file.seek(SeekFrom::Start(100)).unwrap();
    file.write_all(&original[100..101]).unwrap();
    file.sync_all().unwrap();
    let error = session
        .replace_original(&CancelToken::default(), |_| {})
        .unwrap_err();
    assert!(!error.requires_reopen);
    assert_eq!(fs::read(&permanent).unwrap(), original);
    assert_eq!(fs::read(&recovery).unwrap(), original);
    assert_eq!(session.snapshot().revision, 0);
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
fn source_frame_ignores_spatial_preview_and_retains_session_state() {
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
                    x: 0,
                    y: 2,
                    width: 24,
                    height: 18,
                }),
                output_width: Some(16),
                output_height: Some(12),
                ..EditSpec::default()
            },
            export: preview_spec(ExportFormat::Gif, QualityPreset::Standard),
        })
        .unwrap();
    session
        .execute(RecordingEditorRequest::Seek { position_ms: 1_500 })
        .unwrap();
    assert_eq!(session.frame().dimensions(), (16, 12));
    let snapshot = serde_json::to_value(session.snapshot()).unwrap();
    let accepted = session.frame();
    let history_entries = fs::read_dir(&history).unwrap().count();

    let source_frame = session.source_frame(&CancelToken::default()).unwrap();

    assert_eq!(source_frame.dimensions(), (32, 24));
    assert_dominant(source_frame.get_pixel(8, 8).0, 1);
    let outside_crop = source_frame.get_pixel(30, 8).0;
    assert!(
        outside_crop[0] > 180 && outside_crop[1] > 180 && outside_crop[2] > 180,
        "expected the source-only white strip: {outside_crop:?}"
    );
    assert_eq!(serde_json::to_value(session.snapshot()).unwrap(), snapshot);
    assert!(Arc::ptr_eq(&accepted, &session.frame()));
    assert_eq!(fs::read_dir(&history).unwrap().count(), history_entries);
    assert_eq!(fs::read(&source).unwrap(), source_bytes);

    let retained = source_frame.clone();
    drop(session);
    assert_eq!(retained.dimensions(), (32, 24));
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
    let gif_playback = session.playback_with_audio(2_999, &cancel).unwrap();
    assert!(
        !gif_playback.audio_enabled(),
        "GIF preview bypasses the output device even when sound is requested"
    );
    drop(gif_playback);
    let mut playback = session.playback(2_999, &cancel).unwrap();
    assert!(!playback.audio_enabled(), "v1 playback remains silent");
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
