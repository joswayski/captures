use std::{
    fs::File,
    io::{BufWriter, Cursor},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

use pipewire as pw;
use pw::{
    properties::properties,
    spa::{
        self,
        param::{
            ParamType,
            audio::{AudioFormat, AudioInfoRaw},
        },
        pod::{Pod, serialize::PodSerializer},
        utils::{Direction, SpaTypes},
    },
    stream::{StreamFlags, StreamState},
};

use crate::{
    XcapRecordingError, XcapRecordingResult,
    audio::{
        AudioSegmentInfo, WriterHandle, audio_error, elapsed_milliseconds, finalize_writer,
        prepare_audio_path, remove_audio_file, set_failure, usable_audio_path,
    },
};

const AUDIO_READY_TIMEOUT: Duration = Duration::from_secs(5);
const SAMPLE_RATE: u32 = 48_000;
const CHANNELS: u16 = 2;

type ReadySender = mpsc::SyncSender<Result<(), String>>;
type ReadySignal = Arc<Mutex<Option<ReadySender>>>;

struct PipeWireUserData {
    writer: WriterHandle,
    failure: Arc<Mutex<Option<String>>>,
    last_checkpoint: Instant,
}

pub struct PipeWireSystemAudioSegment {
    control: pw::channel::Sender<()>,
    thread: Option<thread::JoinHandle<XcapRecordingResult<()>>>,
    path: PathBuf,
    offset_ms: i64,
    failure: Arc<Mutex<Option<String>>>,
}

impl PipeWireSystemAudioSegment {
    pub fn start(path: &Path, video_started_at: Instant) -> XcapRecordingResult<Self> {
        prepare_audio_path(path)?;
        let path = path.to_path_buf();
        let thread_path = path.clone();
        let failure = Arc::new(Mutex::new(None));
        let thread_failure = failure.clone();
        let (control, control_receiver) = pw::channel::channel();
        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("captures-pipewire-system-audio".to_owned())
            .spawn(move || {
                run_pipewire_system_audio(
                    &thread_path,
                    thread_failure,
                    control_receiver,
                    ready_sender,
                )
            })
            .map_err(audio_error)?;
        match ready_receiver.recv_timeout(AUDIO_READY_TIMEOUT) {
            Ok(Ok(())) => Ok(Self {
                control,
                thread: Some(thread),
                path,
                offset_ms: elapsed_milliseconds(video_started_at),
                failure,
            }),
            Ok(Err(message)) => {
                let _ = control.send(());
                let _ = thread.join();
                let _ = remove_audio_file(&path);
                Err(XcapRecordingError::Audio(message))
            }
            Err(error) => {
                let _ = control.send(());
                let _ = thread.join();
                let _ = remove_audio_file(&path);
                Err(audio_error(error))
            }
        }
    }

    pub fn warning(&self) -> Option<String> {
        self.failure
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn draft_info(&self) -> (PathBuf, i64) {
        (self.path.clone(), self.offset_ms)
    }

    pub fn stop(mut self) -> XcapRecordingResult<AudioSegmentInfo> {
        self.finish_thread()?;
        Ok(AudioSegmentInfo {
            path: usable_audio_path(&self.path),
            offset_ms: self.offset_ms,
            warning: self.warning(),
        })
    }

    pub fn discard(mut self) -> XcapRecordingResult<()> {
        let result = self.finish_thread();
        result.and(remove_audio_file(&self.path))
    }

    fn finish_thread(&mut self) -> XcapRecordingResult<()> {
        let _ = self.control.send(());
        let Some(thread) = self.thread.take() else {
            return Ok(());
        };
        thread.join().map_err(|_| {
            XcapRecordingError::Audio("desktop audio capture thread panicked".to_owned())
        })?
    }
}

impl Drop for PipeWireSystemAudioSegment {
    fn drop(&mut self) {
        let _ = self.finish_thread();
    }
}

fn run_pipewire_system_audio(
    path: &Path,
    failure: Arc<Mutex<Option<String>>>,
    control: pw::channel::Receiver<()>,
    ready: mpsc::SyncSender<Result<(), String>>,
) -> XcapRecordingResult<()> {
    let ready = Arc::new(Mutex::new(Some(ready)));
    let result = run_pipewire_loop(path, failure, control, &ready);
    if let Err(error) = &result {
        signal_ready(&ready, Err(error.to_string()));
    }
    result
}

fn run_pipewire_loop(
    path: &Path,
    failure: Arc<Mutex<Option<String>>>,
    control: pw::channel::Receiver<()>,
    ready: &ReadySignal,
) -> XcapRecordingResult<()> {
    pw::init();
    let main_loop = pw::main_loop::MainLoopRc::new(None).map_err(audio_error)?;
    let context = pw::context::ContextRc::new(&main_loop, None).map_err(audio_error)?;
    let core = context.connect_rc(None).map_err(audio_error)?;
    let writer = Arc::new(Mutex::new(Some(
        hound::WavWriter::new(
            BufWriter::new(File::create(path)?),
            hound::WavSpec {
                channels: CHANNELS,
                sample_rate: SAMPLE_RATE,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            },
        )
        .map_err(audio_error)?,
    )));
    let user_data = PipeWireUserData {
        writer: writer.clone(),
        failure: failure.clone(),
        last_checkpoint: Instant::now(),
    };
    let stream = pw::stream::StreamRc::new(
        core,
        "Captures desktop audio",
        properties! {
            *pw::keys::MEDIA_TYPE => "Audio",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
            *pw::keys::STREAM_CAPTURE_SINK => "true",
        },
    )
    .map_err(audio_error)?;
    let state_ready = ready.clone();
    let state_failure = failure.clone();
    let _listener = stream
        .add_local_listener_with_user_data(user_data)
        .state_changed(move |_, _, _, state| match state {
            StreamState::Paused | StreamState::Streaming => {
                signal_ready(&state_ready, Ok(()));
            }
            StreamState::Error(message) => {
                let message = format!(
                    "PipeWire could not capture desktop audio. Video and microphone audio are still recording ({message})."
                );
                set_failure(&state_failure, message.clone());
                signal_ready(&state_ready, Err(message));
            }
            StreamState::Unconnected | StreamState::Connecting => {}
        })
        .process(|stream, user_data| {
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let Some(data) = buffer.datas_mut().first_mut() else {
                return;
            };
            let offset = usize::try_from(data.chunk().offset()).unwrap_or_default();
            let size = usize::try_from(data.chunk().size()).unwrap_or_default();
            let Some(bytes) = data.data() else {
                return;
            };
            let end = offset.saturating_add(size).min(bytes.len());
            if offset >= end {
                return;
            }
            let checkpoint = user_data.last_checkpoint.elapsed() >= Duration::from_secs(1);
            if let Ok(mut writer) = user_data.writer.try_lock()
                && let Some(writer) = writer.as_mut()
            {
                for sample in bytes[offset..end].chunks_exact(size_of::<f32>()) {
                    let sample = f32::from_le_bytes([sample[0], sample[1], sample[2], sample[3]]);
                    if let Err(error) = writer.write_sample(sample) {
                        set_failure(
                            &user_data.failure,
                            format!(
                                "Desktop audio could not be written. Video and microphone audio are still recording ({error})."
                            ),
                        );
                        break;
                    }
                }
                if checkpoint && let Err(error) = writer.flush() {
                    set_failure(
                        &user_data.failure,
                        format!(
                            "Desktop audio recovery data could not be checkpointed. Video and microphone audio are still recording ({error})."
                        ),
                    );
                }
            }
            if checkpoint {
                user_data.last_checkpoint = Instant::now();
            }
        })
        .register()
        .map_err(audio_error)?;

    let mut audio_info = AudioInfoRaw::new();
    audio_info.set_format(AudioFormat::F32LE);
    audio_info.set_rate(SAMPLE_RATE);
    audio_info.set_channels(u32::from(CHANNELS));
    let object = spa::pod::Object {
        type_: SpaTypes::ObjectParamFormat.as_raw(),
        id: ParamType::EnumFormat.as_raw(),
        properties: audio_info.into(),
    };
    let values =
        PodSerializer::serialize(Cursor::new(Vec::new()), &spa::pod::Value::Object(object))
            .map_err(audio_error)?
            .0
            .into_inner();
    let mut params = [Pod::from_bytes(&values)
        .ok_or_else(|| XcapRecordingError::Audio("invalid PipeWire audio format".to_owned()))?];
    stream
        .connect(
            Direction::Input,
            None,
            StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS,
            &mut params,
        )
        .map_err(audio_error)?;

    let loop_for_control = main_loop.clone();
    let _control = control.attach(main_loop.loop_(), move |()| loop_for_control.quit());
    main_loop.run();
    drop(_listener);
    drop(stream);
    finalize_writer(&writer)
}

fn signal_ready(ready: &ReadySignal, result: Result<(), String>) {
    let sender = ready
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take();
    if let Some(sender) = sender {
        let _ = sender.send(result);
    }
}
