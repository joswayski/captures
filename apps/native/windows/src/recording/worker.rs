//! The recording session belongs to one worker. The HWND stays alive until
//! Closed, including cancellation and replacement's history compensation.
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;

use captures_app::recording_editor::{
    RecordingEditorOpenRequest, RecordingEditorRequestV2, RecordingEditorSession,
    RecordingSaveRequest, ReplacedRecording, SavedRecording,
};
use captures_app::recording_editor_ui::{self as ui, SaveDefaults};
use captures_media::{CancelToken, ExportFormat, ExportProgress, ExportSpec, MediaToolchain};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

pub(super) struct Presented {
    pub mime_type: String,
    pub dropped_frames: u64,
    pub export: ExportSpec,
    pub original: Option<PathBuf>,
    pub defaults: SaveDefaults,
}

impl Presented {
    fn of(session: &RecordingEditorSession, directory: &Path) -> Self {
        let snapshot = session.snapshot_v2();
        Self {
            mime_type: snapshot.editor.source.mime_type.clone(),
            dropped_frames: snapshot.editor.dropped_frames,
            export: snapshot.save_export.clone(),
            original: session.original_save_path().map(Path::to_owned),
            defaults: session.save_defaults(directory),
        }
    }
}

pub(super) enum Job {
    Save(RecordingSaveRequest, bool, CancelToken),
    Reveal(PathBuf),
    Close,
}

pub(super) struct Saved {
    pub path: PathBuf,
    pub size: u64,
    pub warning: Option<String>,
    pub replaced: Option<Presented>,
}

pub(super) enum Outcome {
    Opened(Result<Presented, String>),
    Progress(ExportProgress),
    Saved(Result<Saved, String>),
    Revealed(Result<(), String>),
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
}

impl Worker {
    pub fn start(
        hwnd: HWND,
        root: PathBuf,
        id: String,
        directory: PathBuf,
    ) -> std::io::Result<Self> {
        let (jobs, inbox) = channel();
        let (out, outcomes) = channel();
        let hwnd = hwnd.0 as isize;
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
                let mut session = match RecordingEditorSession::open(
                    RecordingEditorOpenRequest {
                        history_root: root,
                        artifact_id: id,
                    },
                    toolchain(),
                )
                .and_then(|mut session| {
                    // The shared session starts with MP4 preview/save defaults.
                    // Until the format picker lands, preserve GIF sources as GIF
                    // for both save-copy and replacement, never MP4 in a .gif file.
                    let snapshot = session.snapshot_v2();
                    if snapshot.editor.source.mime_type == "image/gif" {
                        let edit = snapshot.editor.edit.clone();
                        let export = ExportSpec {
                            format: ExportFormat::Gif,
                            ..snapshot.save_export.clone()
                        };
                        session
                            .execute_v2(RecordingEditorRequestV2::UpdatePreview { edit, export })?;
                    }
                    Ok(session)
                }) {
                    Ok(session) => {
                        send(Outcome::Opened(Ok(Presented::of(&session, &directory))));
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
                        Job::Reveal(path) => {
                            Outcome::Revealed(crate::history::reveal(&path.to_string_lossy()))
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
                                        (path, None, Some(Presented::of(session, &directory)))
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
        })
    }

    pub fn close(&mut self) {
        if let Some(cancel) = &self.cancel {
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
