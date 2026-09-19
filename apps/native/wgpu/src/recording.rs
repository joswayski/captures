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
