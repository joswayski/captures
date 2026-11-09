//! The recording session belongs to one worker. The HWND stays alive until
//! Closed, including cancellation and replacement's history compensation.
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, atomic::AtomicBool};
use std::thread::JoinHandle;

use captures_app::recording_editor::{
    RecordingEditorOpenRequest, RecordingEditorRequestV2, RecordingEditorSession,
    RecordingSaveRequest, ReplacedRecording, SavedRecording,
};
use captures_app::recording_editor_ui::{self as ui, SaveDefaults};
use captures_media::{
    CancelToken, EditSpec, ExportFormat, ExportProgress, ExportSpec, MediaToolchain,
};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

use super::output::Values;
use super::preview::Frame;

pub(super) struct Presented {
    pub mime_type: String,
    pub dropped_frames: u64,
    pub export: ExportSpec,
    pub original: Option<PathBuf>,
    pub defaults: SaveDefaults,
    pub frame: Arc<Frame>,
    pub duration_ms: u64,
    pub edit: EditSpec,
    pub position_ms: u64,
    pub width: u32,
    pub height: u32,
    pub output: Values,
    pub full_source: bool,
    pub revision: u64,
    pub size_bytes: u64,
}

impl Presented {
    fn of(session: &RecordingEditorSession, directory: &Path, output: &Values) -> Self {
        let snapshot = session.snapshot_v2();
        Self {
            mime_type: snapshot.editor.source.mime_type.clone(),
            dropped_frames: snapshot.editor.dropped_frames,
            export: snapshot.save_export.clone(),
            original: session.original_save_path().map(Path::to_owned),
            defaults: session.save_defaults(directory),
            frame: Arc::new(Frame::new(&session.frame())),
            duration_ms: snapshot.editor.source.duration_ms.unwrap_or(0),
            edit: snapshot.editor.edit.clone(),
            position_ms: snapshot.editor.position_ms,
            width: snapshot.editor.source.width,
            height: snapshot.editor.source.height,
            output: output.clone(),
            full_source: false,
            revision: snapshot.editor.revision,
            size_bytes: snapshot.editor.source.size_bytes,
        }
    }

    fn source(
        session: &RecordingEditorSession,
        directory: &Path,
        output: &Values,
        cache: &mut Option<(u64, Arc<Frame>)>,
        cancel: &CancelToken,
    ) -> Result<Self, String> {
        let mut presented = Self::of(session, directory, output);
        let position = presented.position_ms;
        if cache.as_ref().is_none_or(|(at, _)| *at != position) {
            let frame = session.source_frame(cancel)?;
            *cache = Some((position, Arc::new(Frame::new(&frame))));
        }
        presented.frame = cache.as_ref().expect("source frame").1.clone();
        presented.full_source = true;
        Ok(presented)
    }
}

pub(super) struct PreviewRequest {
    pub generation: u64,
    pub edit: EditSpec,
    pub position_ms: u64,
    pub export: ExportSpec,
    pub output: Values,
}

pub(super) enum Job {
    Save(RecordingSaveRequest, bool, CancelToken),
    Preview,
    Compare(u64, CancelToken),
    Thumbnails(CancelToken),
    Play {
        generation: u64,
        position_ms: u64,
        cancel: CancelToken,
    },
    Reveal(PathBuf),
    Close,
}

pub(super) struct PlaybackFrame {
    pub generation: u64,
    pub position_ms: u64,
    pub frame: Arc<Frame>,
}

pub(super) struct Saved {
    pub path: PathBuf,
    pub size: u64,
    pub warning: Option<String>,
    pub replaced: Option<Presented>,
}

pub(super) enum Outcome {
    Opened(Result<Presented, String>),
    Preview(u64, Box<Presented>, Option<String>),
    Comparison(u64, Result<super::compare::Encoded, String>),
    Thumbnails(Result<Arc<Frame>, String>),
    Progress(ExportProgress),
    Saved(Result<Saved, String>),
    Revealed(Result<(), String>),
    PlaybackEnded(u64, Result<(), String>),
    Closed,
}

/// Explicit overrides, then packaged/repository Tauri sidecars, then PATH.
/// A Windows GNU workbench still uses the shipped MSVC sidecar executable.
fn toolchain() -> MediaToolchain {
    let target = format!("{}-pc-windows-msvc", std::env::consts::ARCH);
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../desktop/src-tauri/binaries");
    let executable = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_owned));
    let tool = |name: &str, variable: &str| {
        std::env::var_os(variable)
            .map(PathBuf::from)
            .or_else(|| {
                executable.as_ref().and_then(|dir| {
                    [format!("{name}.exe"), format!("{name}-{target}.exe")]
                        .into_iter()
                        .map(|name| dir.join(name))
                        .find(|path| path.is_file())
                })
            })
            .or_else(|| {
                let path = repository.join(format!("{name}-{target}.exe"));
                path.is_file().then_some(path)
            })
            .unwrap_or_else(|| PathBuf::from(format!("{name}.exe")))
    };
    MediaToolchain::new(
        tool("ffmpeg", "CAPTURES_FFMPEG"),
        tool("ffprobe", "CAPTURES_FFPROBE"),
    )
}

pub(super) struct Worker {
    pub jobs: Sender<Job>,
    pub outcomes: Receiver<Outcome>,
    thread: Option<JoinHandle<()>>,
    /// Also cancelled if teardown bypasses WM_CLOSE.
    pub cancel: Option<CancelToken>,
    pub playback_cancel: Option<CancelToken>,
    pub thumbnail_cancel: Option<CancelToken>,
    pub comparison_cancel: Option<CancelToken>,
    source_cancel: CancelToken,
    /// One pending scrub/trim request, rather than a queue of stale decodes.
    pending_preview: Arc<Mutex<Option<PreviewRequest>>>,
    /// Bounded to one frame even while a modal dialog blocks the UI loop.
    pub latest: Arc<Mutex<Option<PlaybackFrame>>>,
    pub looping: Arc<AtomicBool>,
}

impl Worker {
    pub fn start(
        hwnd: HWND,
        root: PathBuf,
        id: String,
        directory: PathBuf,
        settings: captures_settings::RecordingSettings,
    ) -> std::io::Result<Self> {
        let (jobs, inbox) = channel();
        let (out, outcomes) = channel();
        let hwnd = hwnd.0 as isize;
        let latest = Arc::new(Mutex::new(None));
        let frames = latest.clone();
        let pending_preview: Arc<Mutex<Option<PreviewRequest>>> = Arc::new(Mutex::new(None));
        let requests = pending_preview.clone();
        let looping = Arc::new(AtomicBool::new(false));
        let loop_flag = looping.clone();
        let source_cancel = CancelToken::default();
        let source_decode = source_cancel.clone();
        let thread = std::thread::Builder::new()
            .name("captures-recording-editor".into())
            .spawn(move || {
                let send = |outcome| {
                    let sent = out.send(outcome).is_ok();
                    if hwnd != 0 {
                        unsafe {
                            let _ = PostMessageW(
                                Some(HWND(hwnd as *mut _)),
                                super::WM_OUTCOME,
                                WPARAM(0),
                                LPARAM(0),
                            );
                        }
                    }
                    sent
                };
                let mut output = Values::new(2, 2, &settings);
                let mut source_frame = None;
                let mut session = match RecordingEditorSession::open(
                    RecordingEditorOpenRequest {
                        history_root: root,
                        artifact_id: id,
                    },
                    toolchain(),
                )
                .and_then(|mut session| {
                    // Shipping starts GIF sources as GIF and honors the saved
                    // output preference for videos, including unsupported WebM.
                    let snapshot = session.snapshot_v2();
                    output = Values::new(
                        snapshot.editor.source.width,
                        snapshot.editor.source.height,
                        &settings,
                    );
                    output.format = ui::initial_output_format(
                        &snapshot.editor.source.mime_type,
                        settings.video_format,
                    );
                    if output.format != captures_settings::VideoFormat::Mp4 {
                        output.mode = ui::QualityMode::Compress;
                        let (edit, export) = output.specs(
                            snapshot.editor.source.width,
                            snapshot.editor.source.height,
                            snapshot.editor.edit.clone(),
                            snapshot.save_export.clone(),
                        );
                        session
                            .execute_v2(RecordingEditorRequestV2::UpdatePreview { edit, export })?;
                    }
                    Ok(session)
                }) {
                    Ok(session) => {
                        send(Outcome::Opened(Presented::source(
                            &session,
                            &directory,
                            &output,
                            &mut source_frame,
                            &source_decode,
                        )));
                        Some(session)
                    }
                    Err(error) => {
                        send(Outcome::Opened(Err(error)));
                        None
                    }
                };
                while let Ok(job) = inbox.recv() {
                    let outcome = match job {
                        Job::Close => break,
                        Job::Compare(generation, cancel) => {
                            let Some(session) = session.as_ref() else {
                                continue;
                            };
                            let result = (|| {
                                let sample = session.export_comparison(&cancel)?;
                                // Shipping displays the hard cap in Maximum mode,
                                // not a second sampled size estimate.
                                let bytes = match session.snapshot_v2().save_export.max_size_bytes {
                                    Some(cap) => cap,
                                    None => session.estimate_export(&cancel)?.size_bytes,
                                };
                                Ok(super::compare::Encoded {
                                    revision: sample.revision,
                                    position_ms: sample.position_ms,
                                    before: Arc::new(Frame::new(&sample.before_frame())),
                                    after: Arc::new(Frame::new(&sample.after_frame())),
                                    bytes,
                                })
                            })();
                            Outcome::Comparison(generation, result)
                        }
                        Job::Preview => {
                            let Some(request) = requests.lock().expect("preview mailbox").take()
                            else {
                                continue;
                            };
                            let Some(session) = session.as_mut() else {
                                continue;
                            };
                            let result =
                                session.execute_v2(RecordingEditorRequestV2::UpdatePreviewAt {
                                    edit: request.edit,
                                    export: request.export,
                                    position_ms: request.position_ms,
                                });
                            if result.is_ok() {
                                output = request.output;
                            }
                            let mut error = result.err();
                            let presented = Presented::source(
                                session,
                                &directory,
                                &output,
                                &mut source_frame,
                                &source_decode,
                            )
                            .unwrap_or_else(|message| {
                                error = Some(format!("Source preview unavailable: {message}"));
                                Presented::of(session, &directory, &output)
                            });
                            Outcome::Preview(request.generation, Box::new(presented), error)
                        }
                        Job::Thumbnails(cancel) => {
                            let Some(session) = session.as_ref() else {
                                continue;
                            };
                            Outcome::Thumbnails(
                                session
                                    .timeline_thumbnails(&cancel)
                                    .map(|strip| Arc::new(Frame::new(strip.pixels()))),
                            )
                        }
                        Job::Reveal(path) => {
                            Outcome::Revealed(crate::history::reveal(&path.to_string_lossy()))
                        }
                        Job::Play {
                            generation,
                            position_ms,
                            cancel,
                        } => {
                            let Some(session) = session.as_ref() else {
                                continue;
                            };
                            let result = (|| {
                                let mut playback = session.looping_source_playback(
                                    position_ms,
                                    true,
                                    loop_flag.clone(),
                                    &cancel,
                                )?;
                                while let Some(frame) = playback.next_frame()? {
                                    if cancel.is_cancelled() {
                                        break;
                                    }
                                    let frame = PlaybackFrame {
                                        generation,
                                        position_ms: frame.position_ms,
                                        frame: Arc::new(Frame::new(&frame.pixels())),
                                    };
                                    let wake = frames
                                        .lock()
                                        .expect("playback mailbox")
                                        .replace(frame)
                                        .is_none();
                                    if wake && hwnd != 0 {
                                        unsafe {
                                            let _ = PostMessageW(
                                                Some(HWND(hwnd as *mut _)),
                                                super::WM_FRAME,
                                                WPARAM(0),
                                                LPARAM(0),
                                            );
                                        }
                                    }
                                }
                                Ok(())
                            })();
                            Outcome::PlaybackEnded(
                                generation,
                                if cancel.is_cancelled() {
                                    Ok(())
                                } else {
                                    result
                                },
                            )
                        }
                        Job::Save(request, copy, cancel) => {
                            let Some(session) = session.as_mut() else {
                                continue;
                            };
                            let progress = |progress| {
                                send(Outcome::Progress(progress));
                            };
                            let saved = if copy {
                                session.save_new(request, &cancel, progress).map(
                                    |saved| match saved {
                                        SavedRecording::Saved { path, .. } => (path, None, None),
                                        SavedRecording::SavedWithoutHistory { path, warning } => {
                                            (path, Some(warning), None)
                                        }
                                    },
                                )
                            } else {
                                session
                                    .replace_original_at(&request.destination, &cancel, progress)
                                    .map(|ReplacedRecording::Replaced { path, .. }| {
                                        source_frame = None;
                                        let snapshot = session.snapshot_v2();
                                        output = Values::new(
                                            snapshot.editor.source.width,
                                            snapshot.editor.source.height,
                                            &settings,
                                        );
                                        output.format = ui::initial_output_format(
                                            &snapshot.editor.source.mime_type,
                                            captures_settings::VideoFormat::Mp4,
                                        );
                                        if snapshot.editor.source.mime_type == "image/gif" {
                                            output.mode = ui::QualityMode::Compress;
                                        }
                                        (
                                            path,
                                            None,
                                            Some(Presented::of(session, &directory, &output)),
                                        )
                                    })
                                    .map_err(|error| error.message)
                            };
                            Outcome::Saved(saved.map(|(path, warning, replaced)| Saved {
                                size: path.metadata().map_or(0, |m| m.len()),
                                path,
                                warning,
                                replaced,
                            }))
                        }
                    };
                    if !send(outcome) {
                        break;
                    }
                }
                send(Outcome::Closed);
            })?;
        Ok(Self {
            jobs,
            outcomes,
            thread: Some(thread),
            cancel: None,
            playback_cancel: None,
            thumbnail_cancel: None,
            comparison_cancel: None,
            source_cancel,
            pending_preview,
            latest,
            looping,
        })
    }

    pub fn preview(&self, request: PreviewRequest) {
        if self
            .pending_preview
            .lock()
            .expect("preview mailbox")
            .replace(request)
            .is_none()
        {
            let _ = self.jobs.send(Job::Preview);
        }
    }

    pub fn close(&mut self) {
        self.source_cancel.cancel();
        if let Some(cancel) = self.comparison_cancel.take() {
            cancel.cancel();
        }
        self.pending_preview.lock().expect("preview mailbox").take();
        if let Some(cancel) = self.thumbnail_cancel.take() {
            cancel.cancel();
        }
        if let Some(cancel) = &self.cancel {
            cancel.cancel();
        }
        if let Some(cancel) = self.playback_cancel.take() {
            cancel.cancel();
        }
        let _ = self.jobs.send(Job::Close);
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.close();
        // Normally Closed already arrived. Unexpected native teardown must
        // not terminate an export halfway through its history compensation.
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub(super) fn progress_text(progress: &ExportProgress) -> String {
    progress
        .message
        .clone()
        .unwrap_or_else(|| ui::export_stage_label(progress.stage).into())
}
