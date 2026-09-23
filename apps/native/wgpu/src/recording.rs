use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
    thread,
};

use captures_capture::DisplayDescriptor;
use captures_media::{CancelToken, MediaToolchain};
use captures_recording::{AudioDevice, RecordingOptions, RecordingSessionSnapshot};
use captures_recording_platform::{FinalizedRecording, RecordingSession};
use eframe::egui;

pub enum Command {
    VerifyToolchain {
        generation: u64,
    },
    ListMicrophones {
        generation: u64,
    },
    Snapshot {
        generation: u64,
    },
    Prepare {
        generation: u64,
        recovery_root: PathBuf,
        options: RecordingOptions,
        display: DisplayDescriptor,
    },
    Start {
        generation: u64,
        exclude_captures_app: bool,
    },
    Pause {
        generation: u64,
    },
    SetMicrophoneMuted {
        generation: u64,
        muted: bool,
        exclude_captures_app: bool,
    },
    Restart {
        generation: u64,
    },
    Resume {
        generation: u64,
        exclude_captures_app: bool,
    },
    Finish {
        generation: u64,
        history_root: PathBuf,
    },
    Discard {
        generation: u64,
    },
    /// Release a terminal owner without deleting its retained recovery media.
    Retire,
    Shutdown,
}

pub enum Event {
    ToolchainVerified {
        generation: u64,
        result: Result<(), String>,
    },
    Microphones {
        generation: u64,
        devices: Vec<AudioDevice>,
    },
    Snapshot {
        generation: u64,
        result: Result<RecordingSessionSnapshot, String>,
    },
    Prepared {
        generation: u64,
        result: Result<RecordingSessionSnapshot, String>,
    },
    Started {
        generation: u64,
        result: Result<RecordingSessionSnapshot, String>,
    },
    Paused {
        generation: u64,
        result: Result<RecordingSessionSnapshot, String>,
    },
    MicrophoneMuted {
        generation: u64,
        result: Result<RecordingSessionSnapshot, MutationFailure>,
    },
    Restarted {
        generation: u64,
        result: Result<RecordingSessionSnapshot, String>,
    },
    Finished {
        generation: u64,
        result: Result<FinalizedRecording, String>,
    },
    Discarded {
        generation: u64,
        result: Result<RecordingSessionSnapshot, String>,
    },
    Retired,
}

pub struct MutationFailure {
    pub error: String,
    pub snapshot: Option<Box<RecordingSessionSnapshot>>,
}

pub struct Worker {
    tx: Sender<Command>,
    rx: Receiver<Event>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Worker {
    pub fn new(ctx: egui::Context) -> Self {
        let (tx, commands) = mpsc::channel();
        let (events, rx) = mpsc::channel();
        let thread = thread::spawn(move || {
            let mut session: Option<RecordingSession> = None;
            let tools = MediaToolchain::from_command_names();
            while let Ok(command) = commands.recv() {
                let event = match command {
                    Command::VerifyToolchain { generation } => Event::ToolchainVerified {
                        generation,
                        result: tools.verify().map_err(|error| {
                            format!("Native recording requires FFmpeg and ffprobe on PATH: {error}")
                        }),
                    },
                    Command::ListMicrophones { generation } => Event::Microphones {
                        generation,
                        devices: captures_recording_platform::microphone_devices(),
                    },
                    Command::Snapshot { generation } => Event::Snapshot {
                        generation,
                        result: session.as_ref().map_or_else(
                            || Err("Recording session is unavailable".into()),
                            |session| Ok(session.snapshot()),
                        ),
                    },
                    Command::Prepare {
                        generation,
                        recovery_root,
                        options,
                        display,
                    } => {
                        let result = RecordingSession::prepare(recovery_root, options, display)
                            .map(|prepared| {
                                let snapshot = prepared.snapshot();
                                session = Some(prepared);
                                snapshot
                            });
                        Event::Prepared { generation, result }
                    }
                    Command::Start {
                        generation,
                        exclude_captures_app,
                    }
                    | Command::Resume {
                        generation,
                        exclude_captures_app,
                    } => Event::Started {
                        generation,
                        result: session.as_mut().map_or_else(
                            || Err("Recording session is unavailable".into()),
                            |session| {
                                session.start(exclude_captures_app, || {
                                    captures_app::capture_flow::is_current(generation)
                                })
                            },
                        ),
                    },
                    Command::Pause { generation } => Event::Paused {
                        generation,
                        result: session.as_mut().map_or_else(
                            || Err("Recording session is unavailable".into()),
                            RecordingSession::pause,
                        ),
                    },
                    Command::SetMicrophoneMuted {
                        generation,
                        muted,
                        exclude_captures_app,
                    } => Event::MicrophoneMuted {
                        generation,
                        result: session.as_mut().map_or_else(
                            || {
                                Err(MutationFailure {
                                    error: "Recording session is unavailable".into(),
                                    snapshot: None,
                                })
                            },
                            |session| match session.set_microphone_muted(
                                muted,
                                exclude_captures_app,
                                || captures_app::capture_flow::is_current(generation),
                            ) {
                                Ok(snapshot) => Ok(snapshot),
                                Err(error) => Err(MutationFailure {
                                    error,
                                    snapshot: Some(Box::new(session.snapshot())),
                                }),
                            },
                        ),
                    },
                    Command::Restart { generation } => Event::Restarted {
                        generation,
                        result: session.as_mut().map_or_else(
                            || Err("Recording session is unavailable".into()),
                            RecordingSession::restart,
                        ),
                    },
                    Command::Finish {
                        generation,
                        history_root,
                    } => Event::Finished {
                        generation,
                        result: session.as_mut().map_or_else(
                            || Err("Recording session is unavailable".into()),
                            |session| {
                                session.stop()?;
                                session.finish(&history_root, &tools, &CancelToken::default())
                            },
                        ),
                    },
                    Command::Discard { generation } => Event::Discarded {
                        generation,
                        result: session.as_mut().map_or_else(
                            || Err("Recording session is unavailable".into()),
                            RecordingSession::discard,
                        ),
                    },
                    Command::Retire => {
                        session = None;
                        Event::Retired
                    }
                    Command::Shutdown => break,
                };
                if events.send(event).is_err() {
                    break;
                }
                ctx.request_repaint();
            }
        });
        Self {
            tx,
            rx,
            thread: Some(thread),
        }
    }

    pub fn send(&self, command: Command) {
        let _ = self.tx.send(command);
    }

    pub fn try_recv(&self) -> Option<Event> {
        self.rx.try_recv().ok()
    }

    pub fn shutdown(&mut self) {
        let _ = self.tx.send(Command::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }

    /// Request worker shutdown without waiting for a blocking device discovery
    /// call. The caller must ensure that the worker owns no recording media.
    pub fn shutdown_detached(&mut self) {
        let _ = self.tx.send(Command::Shutdown);
        drop(self.thread.take());
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use captures_recording_platform::RecordingRecovery;
    use std::time::Duration;

    #[test]
    fn terminal_owner_is_retired_before_recovery_or_next_prepare() {
        let root = tempfile::tempdir().unwrap();
        let recovery_root = root.path().join("recording-recovery");
        let history_root = root.path().join("history");
        let recovery =
            RecordingRecovery::new(history_root.clone(), MediaToolchain::from_command_names());
        let worker = Worker::new(egui::Context::default());
        let prepare = || Command::Prepare {
            generation: 4,
            recovery_root: recovery_root.clone(),
            display: DisplayDescriptor {
                id: "fixture".into(),
                name: "Fixture".into(),
                x: 0,
                y: 0,
                width: 640,
                height: 480,
                scale_factor: 1.,
                is_primary: true,
            },
            options: serde_json::from_value(serde_json::json!({
                "kind":"video", "target":{"type":"display","display_id":"fixture"},
                "frames_per_second":15, "max_resolution":"original", "countdown_seconds":0,
                "show_cursor":false
            }))
            .unwrap(),
        };
        worker.send(prepare());
        let Event::Prepared {
            result: Ok(snapshot),
            ..
        } = worker.rx.recv_timeout(Duration::from_secs(5)).unwrap()
        else {
            panic!("prepare failed")
        };
        let bundle = recovery_root.join(snapshot.id);
        let sentinel = bundle.join("keep.mp4");
        std::fs::write(&sentinel, b"accepted media must survive retirement").unwrap();
        let manifest = std::fs::read(bundle.join("manifest.json")).unwrap();
        assert!(recovery.list().is_err());
        // Finish before Start is an intentional deterministic terminal host error;
        // it must release ownership without treating retirement as discard.
        worker.send(Command::Finish {
            generation: 4,
            history_root,
        });
        assert!(matches!(
            worker.rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            Event::Finished { result: Err(_), .. }
        ));
        assert!(recovery.list().is_err());
        worker.send(Command::Retire);
        assert!(matches!(
            worker.rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            Event::Retired
        ));
        assert_eq!(
            std::fs::read(sentinel).unwrap(),
            b"accepted media must survive retirement"
        );
        assert_eq!(
            std::fs::read(bundle.join("manifest.json")).unwrap(),
            manifest
        );
        assert!(recovery.list().is_ok());
        worker.send(prepare());
        assert!(matches!(
            worker.rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            Event::Prepared { result: Ok(_), .. }
        ));
        worker.send(Command::Retire);
        assert!(matches!(
            worker.rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            Event::Retired
        ));
    }
}
