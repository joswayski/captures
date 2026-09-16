//! Durable native recording journal and recovery, using the shipping manifest.
use anyhow::{Context, Result, bail};
use captures_media::{CancelToken, MediaToolchain, RecordingAudioLayout, RecordingSegmentInput};
use captures_recording::{
    DraftStore, RecordingDraftManifest, RecordingKind, RecordingOptions, RecordingSegmentInfo,
    RecordingSegmentManifest, RecordingState,
};
use std::{
    fs,
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone)]
pub struct Journal {
    store: DraftStore,
    pub manifest: RecordingDraftManifest,
}

pub fn store(profile: &Path) -> DraftStore {
    DraftStore::new(profile.join("recording-drafts"))
}

pub fn prune_gif_sources(profile: &Path) -> Result<()> {
    store(profile).prune_gif_sources(now_ms(), 30 * 24 * 60 * 60 * 1_000)?;
    Ok(())
}

pub fn recoverable(state: RecordingState) -> bool {
    matches!(
        state,
        RecordingState::Recording
            | RecordingState::Paused
            | RecordingState::Finalizing
            | RecordingState::Failed
    )
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

impl Journal {
    pub fn create(profile: &Path, options: RecordingOptions) -> Result<Self> {
        let journal = Self {
            store: store(profile),
            manifest: RecordingDraftManifest::new(
                uuid::Uuid::new_v4().to_string(),
                options,
                now_ms(),
            ),
        };
        journal.store.create(&journal.manifest)?;
        Ok(journal)
    }

    fn directory(&self) -> Result<PathBuf> {
        Ok(self.store.session_directory(&self.manifest.session_id)?)
    }

    fn save(&mut self) -> Result<()> {
        self.manifest.updated_at_ms = now_ms();
        Ok(self.store.save(&self.manifest)?)
    }

    /// Commit the intended file names BEFORE a native writer starts. A process
    /// exit after file finalization but before completion is still recoverable.
    pub fn begin_segment(&mut self, options: &RecordingOptions) -> Result<PathBuf> {
        let index = self.manifest.segments.len() as u32;
        let stem = format!("segment-{index:03}");
        let relative_path = format!("{stem}.mp4");
        self.manifest.segments.push(RecordingSegmentManifest {
            index,
            relative_path: relative_path.clone(),
            system_audio_relative_path: options
                .audio
                .capture_system_audio
                .then(|| format!("{stem}.system.wav")),
            system_audio_offset_ms: 0,
            system_audio_warning: None,
            microphone_relative_path: options
                .audio
                .microphone_device_id
                .as_ref()
                .filter(|_| !options.audio.microphone_muted)
                .map(|_| format!("{stem}.mic.wav")),
            microphone_offset_ms: 0,
            microphone_warning: None,
            started_at_ms: now_ms(),
            duration_ms: 0,
            width: 0,
            height: 0,
            size_bytes: 0,
            dropped_frames: 0,
            complete: false,
        });
        self.manifest.state = RecordingState::Recording;
        self.manifest.last_error = None;
        self.save()?;
        Ok(self.directory()?.join(relative_path))
    }

    pub fn complete_segment(&mut self, info: &RecordingSegmentInfo) -> Result<()> {
        let directory = self.directory()?;
        let name = relative_name(&directory, &info.path)?;
        let segment = self
            .manifest
            .segments
            .iter_mut()
            .find(|segment| segment.relative_path == name)
            .context("completed segment is not in the recording journal")?;
        segment.system_audio_relative_path = info
            .system_audio_path
            .as_deref()
            .map(|path| relative_name(&directory, path))
            .transpose()?;
        segment.system_audio_offset_ms = info.system_audio_offset_ms;
        segment.system_audio_warning = info.system_audio_warning.clone();
        segment.microphone_relative_path = info
            .microphone_path
            .as_deref()
            .map(|path| relative_name(&directory, path))
            .transpose()?;
        segment.microphone_offset_ms = info.microphone_offset_ms;
        segment.microphone_warning = info.microphone_warning.clone();
        segment.width = info.width;
        segment.height = info.height;
        segment.duration_ms = info.duration_ms;
        segment.size_bytes = info.size_bytes;
        segment.dropped_frames = info.dropped_frames;
        segment.complete = true;
        self.save()
    }

    pub fn state(&mut self, state: RecordingState) -> Result<()> {
        self.manifest.state = state;
        self.save()
    }

    pub fn fail(&mut self, error: &anyhow::Error) {
        self.manifest.state = RecordingState::Failed;
        self.manifest.last_error = Some(format!("{error:#}"));
        if let Err(error) = self.save() {
            eprintln!("Could not persist recording failure: {error:#}");
        }
    }

    pub fn discard(&self) -> Result<()> {
        Ok(self.store.remove(&self.manifest.session_id)?)
    }
}

fn relative_name(directory: &Path, path: &Path) -> Result<String> {
    if path.parent() != Some(directory) {
        bail!("recording segment is outside its recovery directory");
    }
    Ok(path
        .file_name()
        .and_then(|name| name.to_str())
        .context("invalid recording segment name")?
        .to_owned())
}

fn child(directory: &Path, relative: &str) -> Result<PathBuf> {
    let mut components = Path::new(relative).components();
    if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
        bail!("invalid recovery file name");
    }
    let path = directory.join(relative);
    match fs::symlink_metadata(&path) {
        Ok(meta) if !meta.file_type().is_file() => bail!("recovery media is not a regular file"),
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error.into()),
        _ => {}
    }
    Ok(path)
}

/// Both ordinary Stop and recovery use this assembly path. Only a verified,
/// indexed private artifact permits the draft to enter its terminal state.
pub fn recover(profile: &Path, id: &str) -> Result<PathBuf> {
    let store = store(profile);
    if !fs::symlink_metadata(store.root())?.file_type().is_dir()
        || !fs::symlink_metadata(store.session_directory(id)?)?
            .file_type()
            .is_dir()
    {
        bail!("recovery directory is not a real directory");
    }
    let mut journal = Journal {
        manifest: store.load(id)?,
        store,
    };
    if !recoverable(journal.manifest.state) {
        bail!("this recording draft is not recoverable");
    }
    let result = assemble(profile, &mut journal);
    if let Err(error) = &result {
        journal.fail(error);
    }
    result
}

fn assemble(profile: &Path, journal: &mut Journal) -> Result<PathBuf> {
    let directory = journal.directory()?;
    if !fs::symlink_metadata(&directory)?.file_type().is_dir() {
        bail!("recovery directory is not a real directory");
    }
    journal.state(RecordingState::Finalizing)?;
    let tools = MediaToolchain::from_command_names();
    let mut inputs = Vec::new();
    let mut dropped_frames = 0;
    for segment in &mut journal.manifest.segments {
        let video_path = child(&directory, &segment.relative_path)?;
        if !video_path.is_file() {
            if segment.complete {
                bail!("completed recovery segment {} is missing", segment.index);
            }
            continue;
        }
        let probe = match tools.probe(&video_path) {
            Ok(probe) if probe.metadata.duration_ms.unwrap_or(0) > 0 => probe,
            _ if !segment.complete => continue,
            _ => bail!(
                "completed recovery segment {} is not playable",
                segment.index
            ),
        };
        segment.complete = true;
        segment.width = probe.metadata.width;
        segment.height = probe.metadata.height;
        segment.duration_ms = probe.metadata.duration_ms.unwrap_or(0);
        segment.size_bytes = probe.metadata.size_bytes;
        dropped_frames += segment.dropped_frames;
        let sidecar = |relative: &Option<String>| -> Result<Option<PathBuf>> {
            Ok(relative
                .as_deref()
                .map(|name| child(&directory, name))
                .transpose()?
                .filter(|path| path.is_file()))
        };
        inputs.push(RecordingSegmentInput {
            video_path,
            system_audio_path: sidecar(&segment.system_audio_relative_path)?,
            system_audio_offset_ms: segment.system_audio_offset_ms,
            microphone_path: sidecar(&segment.microphone_relative_path)?,
            microphone_offset_ms: segment.microphone_offset_ms,
            duration_ms: segment.duration_ms,
        });
    }
    if inputs.is_empty() {
        bail!("no playable media segments could be recovered");
    }
    journal.save()?;
    let gif = journal.manifest.options.kind == RecordingKind::Gif;
    let extension = if gif { "gif" } else { "mp4" };
    // The workspace is disposable; the journal and source segments are not.
    let staging = tempfile::tempdir_in(&directory)?;
    let output = staging.path().join(format!("assembled.{extension}"));
    let cancel = CancelToken::default();
    if gif {
        let master = staging.path().join("master.mp4");
        tools.concatenate_segments(
            &inputs
                .iter()
                .map(|s| s.video_path.clone())
                .collect::<Vec<_>>(),
            &master,
            &cancel,
        )?;
        tools.create_gif(
            &master,
            &output,
            journal.manifest.options.frames_per_second,
            journal.manifest.options.gif.max_width,
            journal.manifest.options.gif.max_colors,
            &cancel,
        )?;
    } else {
        tools.assemble_recording_segments(
            &inputs,
            &output,
            RecordingAudioLayout {
                system_audio: journal.manifest.options.audio.capture_system_audio,
                microphone_audio: inputs.iter().any(|s| s.microphone_path.is_some()),
            },
            &cancel,
        )?;
    }
    tools.probe(&output)?;
    let captures = profile.join("captures");
    fs::create_dir_all(&captures)?;
    if !fs::symlink_metadata(&captures)?.file_type().is_dir() {
        bail!("private capture directory is not a real directory");
    }
    // Stable identity makes retries after a crash during promotion idempotent.
    let destination = captures.join(format!("{}.{}", journal.manifest.session_id, extension));
    fs::OpenOptions::new()
        .write(true)
        .open(&output)?
        .sync_all()?;
    fs::rename(output, &destination)?;
    crate::preferences::history::record_dropped_frames(profile, &destination, dropped_frames)?;
    journal.manifest.final_path = Some(destination.to_string_lossy().into_owned());
    journal.manifest.last_error = None;
    journal.state(RecordingState::Ready)?;
    drop(staging);
    if !gif {
        // Failed cleanup must not turn a successfully saved recording back into
        // a recoverable one. Terminal manifests are omitted by History.
        if let Err(error) = journal.discard() {
            eprintln!("Could not retire completed recording draft: {error:#}");
        }
    }
    Ok(destination)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(kind: RecordingKind) -> RecordingOptions {
        super::super::model::Settings::default().options(
            kind,
            captures_recording::RecordingTarget::Display {
                display_id: "fixture".into(),
            },
        )
    }

    fn fixture(path: &Path, color: &str, duration: &str) {
        let output = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-f", "lavfi", "-i"])
            .arg(format!("color={color}:s=96x64:r=30"))
            .args(["-t", duration, "-c:v", "libx264", "-pix_fmt", "yuv420p"])
            .arg(path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn interrupted_pending_segments_are_probed_ordered_and_promoted_once() {
        let profile = tempfile::tempdir().unwrap();
        let options = options(RecordingKind::Video);
        let mut journal = Journal::create(profile.path(), options.clone()).unwrap();
        let first = journal.begin_segment(&options).unwrap();
        fixture(&first, "red", "0.4");
        let second = journal.begin_segment(&options).unwrap();
        fixture(&second, "blue", "0.7");
        let broken = journal.begin_segment(&options).unwrap();
        fs::write(&broken, b"interrupted unplayable media").unwrap();
        journal.manifest.segments[0].dropped_frames = 3;
        journal.manifest.segments[1].dropped_frames = 7;
        journal.manifest.segments[2].dropped_frames = 123;
        journal.save().unwrap();
        let id = journal.manifest.session_id.clone();
        // Reopen the disk journal, as after process loss. No in-memory completed
        // segment list participates in recovery.
        drop(journal);
        let result = recover(profile.path(), &id).unwrap();
        let probe = MediaToolchain::from_command_names().probe(&result).unwrap();
        assert_eq!((probe.metadata.width, probe.metadata.height), (96, 64));
        assert!((1_090..=1_180).contains(&probe.metadata.duration_ms.unwrap()));
        for (seconds, red) in [("0.1", true), ("0.8", false)] {
            let output = std::process::Command::new("ffmpeg")
                .args(["-v", "error", "-ss", seconds, "-i"])
                .arg(&result)
                .args(["-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
                .output()
                .unwrap();
            assert!(output.status.success());
            let pixel = &output.stdout[..3];
            assert!(if red {
                pixel[0] > 200 && pixel[2] < 30
            } else {
                pixel[2] > 200 && pixel[0] < 30
            });
        }
        assert_eq!(
            crate::preferences::history::load(profile.path())
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            crate::preferences::history::dropped_frames(profile.path(), &result).unwrap(),
            10
        );
        assert!(
            !store(profile.path())
                .session_directory(&id)
                .unwrap()
                .exists()
        );
        assert!(recover(profile.path(), &id).is_err());
    }

    #[test]
    fn missing_completed_segment_fails_without_losing_remaining_sources() {
        let profile = tempfile::tempdir().unwrap();
        let options = options(RecordingKind::Video);
        let mut journal = Journal::create(profile.path(), options.clone()).unwrap();
        let path = journal.begin_segment(&options).unwrap();
        fixture(&path, "green", "0.3");
        journal.begin_segment(&options).unwrap();
        journal.manifest.segments[1].complete = true;
        journal.save().unwrap();
        let id = &journal.manifest.session_id;
        assert!(
            recover(profile.path(), id)
                .unwrap_err()
                .to_string()
                .contains("missing")
        );
        assert!(path.exists());
        let failed = store(profile.path()).load(id).unwrap();
        assert_eq!(failed.state, RecordingState::Failed);
        assert!(failed.last_error.unwrap().contains("missing"));
        assert!(
            crate::preferences::history::load(profile.path())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn gif_recovery_retains_master_and_terminal_drafts_are_not_recovered() {
        let profile = tempfile::tempdir().unwrap();
        let options = options(RecordingKind::Gif);
        let mut journal = Journal::create(profile.path(), options.clone()).unwrap();
        let path = journal.begin_segment(&options).unwrap();
        fixture(&path, "yellow", "0.3");
        let id = &journal.manifest.session_id;
        let output = recover(profile.path(), id).unwrap();
        assert_eq!(output.extension().unwrap(), "gif");
        assert!(path.exists());
        assert_eq!(
            store(profile.path()).load(id).unwrap().state,
            RecordingState::Ready
        );
        assert!(recover(profile.path(), id).is_err());
        assert!(!recoverable(RecordingState::Ready));
        assert!(!recoverable(RecordingState::Discarded));
        assert!(recoverable(RecordingState::Paused));
    }

    #[test]
    fn completion_persists_native_sidecar_offsets_and_pause_state() {
        let profile = tempfile::tempdir().unwrap();
        let mut options = options(RecordingKind::Video);
        options.audio.capture_system_audio = true;
        options.audio.microphone_device_id = Some("mic".into());
        let mut journal = Journal::create(profile.path(), options.clone()).unwrap();
        let path = journal.begin_segment(&options).unwrap();
        let id = journal.manifest.session_id.clone();
        let pending = store(profile.path()).load(&id).unwrap();
        assert!(!pending.segments[0].complete);
        assert_eq!(
            pending.segments[0].microphone_relative_path.as_deref(),
            Some("segment-000.mic.wav")
        );
        journal
            .complete_segment(&RecordingSegmentInfo {
                system_audio_path: Some(path.with_file_name("segment-000.system.wav")),
                system_audio_offset_ms: -27,
                system_audio_warning: None,
                microphone_path: Some(path.with_file_name("segment-000.mic.wav")),
                microphone_offset_ms: 83,
                microphone_warning: Some("device interrupted".into()),
                path,
                width: 620,
                height: 320,
                duration_ms: 1234,
                size_bytes: 5678,
                dropped_frames: 9,
            })
            .unwrap();
        journal.state(RecordingState::Paused).unwrap();
        let persisted = store(profile.path()).load(&id).unwrap();
        assert_eq!(persisted.state, RecordingState::Paused);
        let segment = &persisted.segments[0];
        assert!(segment.complete);
        assert_eq!(
            (segment.system_audio_offset_ms, segment.microphone_offset_ms),
            (-27, 83)
        );
        assert_eq!(
            (
                segment.width,
                segment.height,
                segment.duration_ms,
                segment.size_bytes,
                segment.dropped_frames
            ),
            (620, 320, 1234, 5678, 9)
        );
        assert_eq!(
            segment.microphone_warning.as_deref(),
            Some("device interrupted")
        );
    }

    #[test]
    fn history_failure_preserves_sources_and_retry_keeps_one_artifact() {
        let profile = tempfile::tempdir().unwrap();
        let options = options(RecordingKind::Video);
        let mut journal = Journal::create(profile.path(), options.clone()).unwrap();
        let path = journal.begin_segment(&options).unwrap();
        fixture(&path, "purple", "0.3");
        let id = &journal.manifest.session_id;
        fs::write(profile.path().join("capture-history.json"), b"invalid").unwrap();
        assert!(recover(profile.path(), id).is_err());
        assert!(path.exists());
        let destination = profile.path().join("captures").join(format!("{id}.mp4"));
        assert!(destination.exists());
        assert_eq!(
            store(profile.path()).load(id).unwrap().state,
            RecordingState::Failed
        );
        fs::write(profile.path().join("capture-history.json"), b"{}").unwrap();
        assert_eq!(recover(profile.path(), id).unwrap(), destination);
        assert_eq!(
            fs::read_dir(profile.path().join("captures"))
                .unwrap()
                .count(),
            1
        );
        assert_eq!(
            crate::preferences::history::load(profile.path())
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn recovery_rejects_traversal_and_preserves_external_media() {
        let profile = tempfile::tempdir().unwrap();
        let options = options(RecordingKind::Video);
        let mut journal = Journal::create(profile.path(), options.clone()).unwrap();
        journal.begin_segment(&options).unwrap();
        journal.manifest.segments[0].relative_path = "../../external.mp4".into();
        journal.save().unwrap();
        let external = profile.path().join("external.mp4");
        fs::write(&external, b"permanent media").unwrap();
        assert!(recover(profile.path(), &journal.manifest.session_id).is_err());
        assert_eq!(fs::read(external).unwrap(), b"permanent media");
    }
}
