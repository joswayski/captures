use std::{
    fs::{self, File},
    io::BufWriter,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU32, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use cpal::{
    FromSample, Sample, SampleFormat, SizedSample, Stream,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};

use crate::{MacRecordingError, MacRecordingResult, native};

type Writer = hound::WavWriter<BufWriter<File>>;
type WriterHandle = Arc<Mutex<Option<Writer>>>;

/// CPAL's CoreAudio stream is intentionally kept on its owning thread.
/// The public handle contains only Send-safe controls and telemetry so it can
/// live inside Tauri's shared application state without unsafe trait impls.
pub struct MicrophoneSegment {
    control: mpsc::Sender<()>,
    thread: Option<thread::JoinHandle<MacRecordingResult<()>>>,
    path: PathBuf,
    offset_ms: i64,
    failure: Arc<Mutex<Option<String>>>,
    level_bits: Arc<AtomicU32>,
}

pub struct MicrophoneSegmentInfo {
    pub path: Option<PathBuf>,
    pub offset_ms: i64,
    pub warning: Option<String>,
}

impl MicrophoneSegment {
    pub fn start(device_id: &str, path: &Path, offset_ms: i64) -> MacRecordingResult<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        if path.exists() {
            fs::remove_file(path)?;
        }

        let device_name = if device_id == "default" {
            None
        } else {
            Some(
                native::microphone_name_for_id(device_id)
                    .ok_or(MacRecordingError::TargetUnavailable)?,
            )
        };
        let path = path.to_path_buf();
        let thread_path = path.clone();
        let failure = Arc::new(Mutex::new(None));
        let thread_failure = failure.clone();
        let level_bits = Arc::new(AtomicU32::new(0));
        let thread_level = level_bits.clone();
        let (control, control_receiver) = mpsc::channel();
        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("captures-microphone".to_owned())
            .spawn(move || {
                run_microphone(
                    device_name,
                    &thread_path,
                    thread_failure,
                    thread_level,
                    control_receiver,
                    ready_sender,
                )
            })
            .map_err(microphone_error)?;
        match ready_receiver.recv() {
            Ok(Ok(())) => Ok(Self {
                control,
                thread: Some(thread),
                path,
                offset_ms,
                failure,
                level_bits,
            }),
            Ok(Err(message)) => {
                let _ = thread.join();
                Err(MacRecordingError::Microphone(message))
            }
            Err(error) => {
                let _ = thread.join();
                Err(microphone_error(error))
            }
        }
    }

    pub fn warning(&self) -> Option<String> {
        self.failure
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn level(&self) -> f32 {
        f32::from_bits(self.level_bits.load(Ordering::Acquire))
    }

    pub fn draft_info(&self) -> (PathBuf, i64) {
        (self.path.clone(), self.offset_ms)
    }

    pub fn stop(mut self) -> MacRecordingResult<MicrophoneSegmentInfo> {
        self.finish_thread()?;
        let warning = self.warning();
        let path = fs::metadata(&self.path)
            .ok()
            .filter(|metadata| metadata.len() > 44)
            .map(|_| self.path);
        Ok(MicrophoneSegmentInfo {
            path,
            offset_ms: self.offset_ms,
            warning,
        })
    }

    pub fn discard(mut self) -> MacRecordingResult<()> {
        let result = self.finish_thread();
        let remove_result = match fs::remove_file(self.path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        };
        result.and(remove_result)
    }

    fn finish_thread(&mut self) -> MacRecordingResult<()> {
        let _ = self.control.send(());
        let Some(thread) = self.thread.take() else {
            return Ok(());
        };
        thread
            .join()
            .map_err(|_| MacRecordingError::Microphone("capture thread panicked".to_owned()))?
    }
}

fn run_microphone(
    device_name: Option<String>,
    path: &Path,
    failure: Arc<Mutex<Option<String>>>,
    level_bits: Arc<AtomicU32>,
    control: mpsc::Receiver<()>,
    ready: mpsc::SyncSender<Result<(), String>>,
) -> MacRecordingResult<()> {
    let initialized = initialize_stream(device_name.as_deref(), path, &failure, &level_bits);
    let (stream, writer) = match initialized {
        Ok(initialized) => {
            let _ = ready.send(Ok(()));
            initialized
        }
        Err(error) => {
            let _ = ready.send(Err(error.to_string()));
            return Err(error);
        }
    };
    let _ = control.recv();
    drop(stream);
    finalize_writer(&writer)
}

fn initialize_stream(
    device_name: Option<&str>,
    path: &Path,
    failure: &Arc<Mutex<Option<String>>>,
    level_bits: &Arc<AtomicU32>,
) -> MacRecordingResult<(Stream, WriterHandle)> {
    let host = cpal::default_host();
    let device = if let Some(expected_name) = device_name {
        host.input_devices()
            .map_err(microphone_error)?
            .find(|device| device.name().is_ok_and(|name| name == expected_name))
    } else {
        host.default_input_device()
    }
    .ok_or(MacRecordingError::TargetUnavailable)?;
    let config = device.default_input_config().map_err(microphone_error)?;
    let spec = hound::WavSpec {
        channels: config.channels(),
        sample_rate: config.sample_rate().0,
        bits_per_sample: u16::try_from(config.sample_format().sample_size() * 8).unwrap_or(32),
        sample_format: if config.sample_format().is_float() {
            hound::SampleFormat::Float
        } else {
            hound::SampleFormat::Int
        },
    };
    let writer = Arc::new(Mutex::new(Some(
        hound::WavWriter::create(path, spec).map_err(microphone_error)?,
    )));
    let stream = match config.sample_format() {
        SampleFormat::F32 => build_stream::<f32>(&device, &config, &writer, failure, level_bits),
        SampleFormat::I16 => build_stream::<i16>(&device, &config, &writer, failure, level_bits),
        SampleFormat::I32 => build_stream::<i32>(&device, &config, &writer, failure, level_bits),
        format => Err(MacRecordingError::Microphone(format!(
            "the selected device uses unsupported {format} samples"
        ))),
    }?;
    stream.play().map_err(microphone_error)?;
    Ok((stream, writer))
}

fn build_stream<T>(
    device: &cpal::Device,
    config: &cpal::SupportedStreamConfig,
    writer: &WriterHandle,
    failure: &Arc<Mutex<Option<String>>>,
    level_bits: &Arc<AtomicU32>,
) -> MacRecordingResult<Stream>
where
    T: Sample + SizedSample + hound::Sample,
    f32: FromSample<T>,
{
    let writer = writer.clone();
    let callback_level = level_bits.clone();
    let write_failure = failure.clone();
    let stream_failure = failure.clone();
    let mut last_checkpoint = Instant::now();
    device
        .build_input_stream(
            &config.clone().into(),
            move |samples: &[T], _| {
                let checkpoint = last_checkpoint.elapsed() >= Duration::from_secs(1);
                write_samples(
                    samples,
                    &writer,
                    &callback_level,
                    &write_failure,
                    checkpoint,
                );
                if checkpoint {
                    last_checkpoint = Instant::now();
                }
            },
            move |error| {
                set_failure(
                    &stream_failure,
                    format!(
                        "The selected microphone disconnected. Video and desktop audio are still recording ({error})."
                    ),
                );
            },
            None,
        )
        .map_err(microphone_error)
}

fn write_samples<T>(
    samples: &[T],
    writer: &WriterHandle,
    level_bits: &AtomicU32,
    failure: &Arc<Mutex<Option<String>>>,
    checkpoint: bool,
) where
    T: Sample + hound::Sample,
    f32: FromSample<T>,
{
    let mut peak = 0.0_f32;
    if let Ok(mut guard) = writer.try_lock()
        && let Some(writer) = guard.as_mut()
    {
        for &sample in samples {
            peak = peak.max(f32::from_sample(sample).abs().min(1.0));
            if let Err(error) = writer.write_sample(sample) {
                set_failure(
                    failure,
                    format!(
                        "Microphone audio could not be written. Video and desktop audio are still recording ({error})."
                    ),
                );
                break;
            }
        }
        if checkpoint && let Err(error) = writer.flush() {
            set_failure(
                failure,
                format!(
                    "Microphone recovery data could not be checkpointed. Video and desktop audio are still recording ({error})."
                ),
            );
        }
    }
    level_bits.store(peak.to_bits(), Ordering::Release);
}

fn set_failure(failure: &Arc<Mutex<Option<String>>>, message: String) {
    let mut current = failure
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if current.is_none() {
        *current = Some(message);
    }
}

fn finalize_writer(writer: &WriterHandle) -> MacRecordingResult<()> {
    let writer = writer
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take();
    if let Some(writer) = writer {
        writer.finalize().map_err(microphone_error)?;
    }
    Ok(())
}

fn microphone_error(error: impl std::fmt::Display) -> MacRecordingError {
    MacRecordingError::Microphone(error.to_string())
}
