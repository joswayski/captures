use std::{
    collections::BTreeSet,
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

use captures_recording::{AudioDevice, AudioDeviceKind};
use cpal::{
    FromSample, I24, Sample, SampleFormat, SizedSample, Stream,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};

use crate::{XcapRecordingError, XcapRecordingResult};

#[cfg(target_os = "linux")]
use crate::system_audio_linux::PipeWireSystemAudioSegment;

const DEVICE_ID_PREFIX: &str = "microphone:";
const AUDIO_READY_TIMEOUT: Duration = Duration::from_secs(5);
const FEEDBACK_VOLUME: f32 = 0.12;

type Writer = hound::WavWriter<BufWriter<File>>;
pub(crate) type WriterHandle = Arc<Mutex<Option<Writer>>>;

/// Plays a short confirmation tone after a screenshot is captured.
pub fn play_capture_sound() {
    play_feedback_tone(880.0, Duration::from_millis(90), "captures-capture-sound");
}

/// Plays a short tone when a recording begins.
pub fn play_start_chime() {
    play_feedback_tone(
        660.0,
        Duration::from_millis(140),
        "captures-recording-chime",
    );
}

fn play_feedback_tone(frequency_hz: f32, duration: Duration, thread_name: &str) {
    let spawn = thread::Builder::new()
        .name(thread_name.to_owned())
        .spawn(move || {
            if let Err(error) = run_feedback_tone(frequency_hz, duration) {
                eprintln!("failed to play audio feedback: {error}");
            }
        });
    if let Err(error) = spawn {
        eprintln!("failed to start audio feedback: {error}");
    }
}

fn run_feedback_tone(frequency_hz: f32, duration: Duration) -> XcapRecordingResult<()> {
    let host = cpal::default_host();
    let device = host.default_output_device().ok_or_else(|| {
        XcapRecordingError::Audio("no audio output device is available".to_owned())
    })?;
    let config = device.default_output_config().map_err(audio_error)?;
    let stream = match config.sample_format() {
        SampleFormat::I8 => build_feedback_stream::<i8>(&device, &config, frequency_hz, duration),
        SampleFormat::I16 => build_feedback_stream::<i16>(&device, &config, frequency_hz, duration),
        SampleFormat::I24 => build_feedback_stream::<I24>(&device, &config, frequency_hz, duration),
        SampleFormat::I32 => build_feedback_stream::<i32>(&device, &config, frequency_hz, duration),
        SampleFormat::I64 => build_feedback_stream::<i64>(&device, &config, frequency_hz, duration),
        SampleFormat::U8 => build_feedback_stream::<u8>(&device, &config, frequency_hz, duration),
        SampleFormat::U16 => build_feedback_stream::<u16>(&device, &config, frequency_hz, duration),
        SampleFormat::U32 => build_feedback_stream::<u32>(&device, &config, frequency_hz, duration),
        SampleFormat::U64 => build_feedback_stream::<u64>(&device, &config, frequency_hz, duration),
        SampleFormat::F32 => build_feedback_stream::<f32>(&device, &config, frequency_hz, duration),
        SampleFormat::F64 => build_feedback_stream::<f64>(&device, &config, frequency_hz, duration),
        format => Err(XcapRecordingError::Audio(format!(
            "the audio output device uses unsupported {format} samples"
        ))),
    }?;
    stream.play().map_err(audio_error)?;
    thread::sleep(duration + Duration::from_millis(30));
    Ok(())
}

fn build_feedback_stream<T>(
    device: &cpal::Device,
    config: &cpal::SupportedStreamConfig,
    frequency_hz: f32,
    duration: Duration,
) -> XcapRecordingResult<Stream>
where
    T: FromSample<f32> + SizedSample,
{
    let sample_rate = config.sample_rate().0 as f32;
    let channels = usize::from(config.channels());
    let total_frames = (duration.as_secs_f32() * sample_rate) as usize;
    let fade_frames = (sample_rate * 0.008) as usize;
    let mut frame_index = 0_usize;
    device
        .build_output_stream(
            &config.clone().into(),
            move |output: &mut [T], _| {
                for frame in output.chunks_mut(channels) {
                    let sample = feedback_sample(
                        frame_index,
                        total_frames,
                        fade_frames,
                        sample_rate,
                        frequency_hz,
                    );
                    frame.fill(T::from_sample(sample));
                    frame_index = frame_index.saturating_add(1);
                }
            },
            |error| eprintln!("audio feedback device disconnected: {error}"),
            None,
        )
        .map_err(audio_error)
}

fn feedback_sample(
    frame: usize,
    total_frames: usize,
    fade_frames: usize,
    sample_rate: f32,
    frequency_hz: f32,
) -> f32 {
    if frame >= total_frames || total_frames == 0 {
        return 0.0;
    }
    let fade_in = frame.min(fade_frames) as f32 / fade_frames.max(1) as f32;
    let fade_out =
        total_frames.saturating_sub(frame).min(fade_frames) as f32 / fade_frames.max(1) as f32;
    let phase = std::f32::consts::TAU * frequency_hz * frame as f32 / sample_rate;
    phase.sin() * FEEDBACK_VOLUME * fade_in.min(fade_out)
}

pub struct AudioSegmentInfo {
    pub path: Option<PathBuf>,
    pub offset_ms: i64,
    pub warning: Option<String>,
}

pub struct AudioSegment {
    inner: AudioSegmentInner,
}

enum AudioSegmentInner {
    Cpal(CpalAudioSegment),
    #[cfg(target_os = "linux")]
    PipeWire(PipeWireSystemAudioSegment),
}

impl AudioSegment {
    pub fn start_microphone(
        device_id: &str,
        path: &Path,
        video_started_at: Instant,
    ) -> XcapRecordingResult<Self> {
        let device_name = microphone_name_for_id(device_id)?;
        CpalAudioSegment::start(
            CpalSource::Microphone(device_name),
            path,
            video_started_at,
            "captures-microphone",
        )
        .map(|inner| Self {
            inner: AudioSegmentInner::Cpal(inner),
        })
    }

    pub fn start_system(path: &Path, video_started_at: Instant) -> XcapRecordingResult<Self> {
        #[cfg(target_os = "windows")]
        {
            CpalAudioSegment::start(
                CpalSource::SystemAudio,
                path,
                video_started_at,
                "captures-system-audio",
            )
            .map(|inner| Self {
                inner: AudioSegmentInner::Cpal(inner),
            })
        }
        #[cfg(target_os = "linux")]
        {
            PipeWireSystemAudioSegment::start(path, video_started_at).map(|inner| Self {
                inner: AudioSegmentInner::PipeWire(inner),
            })
        }
    }

    pub fn warning(&self) -> Option<String> {
        match &self.inner {
            AudioSegmentInner::Cpal(segment) => segment.warning(),
            #[cfg(target_os = "linux")]
            AudioSegmentInner::PipeWire(segment) => segment.warning(),
        }
    }

    pub fn level(&self) -> f32 {
        match &self.inner {
            AudioSegmentInner::Cpal(segment) => segment.level(),
            #[cfg(target_os = "linux")]
            AudioSegmentInner::PipeWire(_) => 0.0,
        }
    }

    pub fn draft_info(&self) -> (PathBuf, i64) {
        match &self.inner {
            AudioSegmentInner::Cpal(segment) => segment.draft_info(),
            #[cfg(target_os = "linux")]
            AudioSegmentInner::PipeWire(segment) => segment.draft_info(),
        }
    }

    pub fn stop(self) -> XcapRecordingResult<AudioSegmentInfo> {
        match self.inner {
            AudioSegmentInner::Cpal(segment) => segment.stop(),
            #[cfg(target_os = "linux")]
            AudioSegmentInner::PipeWire(segment) => segment.stop(),
        }
    }

    pub fn discard(self) -> XcapRecordingResult<()> {
        match self.inner {
            AudioSegmentInner::Cpal(segment) => segment.discard(),
            #[cfg(target_os = "linux")]
            AudioSegmentInner::PipeWire(segment) => segment.discard(),
        }
    }
}

enum CpalSource {
    Microphone(Option<String>),
    #[cfg(target_os = "windows")]
    SystemAudio,
}

impl CpalSource {
    const fn label(&self) -> &'static str {
        match self {
            Self::Microphone(_) => "microphone",
            #[cfg(target_os = "windows")]
            Self::SystemAudio => "desktop audio",
        }
    }

    const fn other_audio(&self) -> &'static str {
        match self {
            Self::Microphone(_) => "Video and desktop audio are still recording",
            #[cfg(target_os = "windows")]
            Self::SystemAudio => "Video and microphone audio are still recording",
        }
    }
}

struct CpalAudioSegment {
    control: mpsc::Sender<()>,
    thread: Option<thread::JoinHandle<XcapRecordingResult<()>>>,
    path: PathBuf,
    offset_ms: i64,
    failure: Arc<Mutex<Option<String>>>,
    level_bits: Arc<AtomicU32>,
}

impl CpalAudioSegment {
    fn start(
        source: CpalSource,
        path: &Path,
        video_started_at: Instant,
        thread_name: &str,
    ) -> XcapRecordingResult<Self> {
        prepare_audio_path(path)?;
        let path = path.to_path_buf();
        let thread_path = path.clone();
        let failure = Arc::new(Mutex::new(None));
        let thread_failure = failure.clone();
        let level_bits = Arc::new(AtomicU32::new(0));
        let thread_level = level_bits.clone();
        let (control, control_receiver) = mpsc::channel();
        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name(thread_name.to_owned())
            .spawn(move || {
                run_cpal_audio(
                    source,
                    &thread_path,
                    thread_failure,
                    thread_level,
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
                level_bits,
            }),
            Ok(Err(message)) => {
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

    fn warning(&self) -> Option<String> {
        self.failure
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn level(&self) -> f32 {
        f32::from_bits(self.level_bits.load(Ordering::Acquire))
    }

    fn draft_info(&self) -> (PathBuf, i64) {
        (self.path.clone(), self.offset_ms)
    }

    fn stop(mut self) -> XcapRecordingResult<AudioSegmentInfo> {
        self.finish_thread()?;
        let warning = self.warning();
        let path = usable_audio_path(&self.path);
        Ok(AudioSegmentInfo {
            path,
            offset_ms: self.offset_ms,
            warning,
        })
    }

    fn discard(mut self) -> XcapRecordingResult<()> {
        let result = self.finish_thread();
        let remove_result = remove_audio_file(&self.path);
        result.and(remove_result)
    }

    fn finish_thread(&mut self) -> XcapRecordingResult<()> {
        let _ = self.control.send(());
        let Some(thread) = self.thread.take() else {
            return Ok(());
        };
        thread
            .join()
            .map_err(|_| XcapRecordingError::Audio("audio capture thread panicked".to_owned()))?
    }
}

impl Drop for CpalAudioSegment {
    fn drop(&mut self) {
        let _ = self.finish_thread();
    }
}

fn run_cpal_audio(
    source: CpalSource,
    path: &Path,
    failure: Arc<Mutex<Option<String>>>,
    level_bits: Arc<AtomicU32>,
    control: mpsc::Receiver<()>,
    ready: mpsc::SyncSender<Result<(), String>>,
) -> XcapRecordingResult<()> {
    let initialized = initialize_cpal_stream(&source, path, &failure, &level_bits);
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

fn initialize_cpal_stream(
    source: &CpalSource,
    path: &Path,
    failure: &Arc<Mutex<Option<String>>>,
    level_bits: &Arc<AtomicU32>,
) -> XcapRecordingResult<(Stream, WriterHandle)> {
    let host = cpal::default_host();
    let device = match source {
        CpalSource::Microphone(Some(expected_name)) => host
            .input_devices()
            .map_err(audio_error)?
            .find(|device| device.name().is_ok_and(|name| name == *expected_name)),
        CpalSource::Microphone(None) => host.default_input_device(),
        #[cfg(target_os = "windows")]
        CpalSource::SystemAudio => host.default_output_device(),
    }
    .ok_or_else(|| {
        XcapRecordingError::Audio(format!("no {} device is available", source.label()))
    })?;
    let config = match source {
        CpalSource::Microphone(_) => device.default_input_config(),
        #[cfg(target_os = "windows")]
        CpalSource::SystemAudio => device.default_output_config(),
    }
    .map_err(audio_error)?;
    let writer = Arc::new(Mutex::new(Some(
        hound::WavWriter::create(
            path,
            hound::WavSpec {
                channels: config.channels(),
                sample_rate: config.sample_rate().0,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            },
        )
        .map_err(audio_error)?,
    )));
    let stream = match config.sample_format() {
        SampleFormat::I8 => {
            build_cpal_stream::<i8>(&device, &config, &writer, failure, level_bits, source)
        }
        SampleFormat::I16 => {
            build_cpal_stream::<i16>(&device, &config, &writer, failure, level_bits, source)
        }
        SampleFormat::I24 => {
            build_cpal_stream::<I24>(&device, &config, &writer, failure, level_bits, source)
        }
        SampleFormat::I32 => {
            build_cpal_stream::<i32>(&device, &config, &writer, failure, level_bits, source)
        }
        SampleFormat::I64 => {
            build_cpal_stream::<i64>(&device, &config, &writer, failure, level_bits, source)
        }
        SampleFormat::U8 => {
            build_cpal_stream::<u8>(&device, &config, &writer, failure, level_bits, source)
        }
        SampleFormat::U16 => {
            build_cpal_stream::<u16>(&device, &config, &writer, failure, level_bits, source)
        }
        SampleFormat::U32 => {
            build_cpal_stream::<u32>(&device, &config, &writer, failure, level_bits, source)
        }
        SampleFormat::U64 => {
            build_cpal_stream::<u64>(&device, &config, &writer, failure, level_bits, source)
        }
        SampleFormat::F32 => {
            build_cpal_stream::<f32>(&device, &config, &writer, failure, level_bits, source)
        }
        SampleFormat::F64 => {
            build_cpal_stream::<f64>(&device, &config, &writer, failure, level_bits, source)
        }
        format => Err(XcapRecordingError::Audio(format!(
            "the selected {} device uses unsupported {format} samples",
            source.label()
        ))),
    }?;
    stream.play().map_err(audio_error)?;
    Ok((stream, writer))
}

fn build_cpal_stream<T>(
    device: &cpal::Device,
    config: &cpal::SupportedStreamConfig,
    writer: &WriterHandle,
    failure: &Arc<Mutex<Option<String>>>,
    level_bits: &Arc<AtomicU32>,
    source: &CpalSource,
) -> XcapRecordingResult<Stream>
where
    T: Sample + SizedSample,
    f32: FromSample<T>,
{
    let writer = writer.clone();
    let callback_level = level_bits.clone();
    let write_failure = failure.clone();
    let stream_failure = failure.clone();
    let label = source.label();
    let other_audio = source.other_audio();
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
                    label,
                    other_audio,
                );
                if checkpoint {
                    last_checkpoint = Instant::now();
                }
            },
            move |error| {
                set_failure(
                    &stream_failure,
                    format!("The {label} device disconnected. {other_audio} ({error})."),
                );
            },
            None,
        )
        .map_err(audio_error)
}

fn write_samples<T>(
    samples: &[T],
    writer: &WriterHandle,
    level_bits: &AtomicU32,
    failure: &Arc<Mutex<Option<String>>>,
    checkpoint: bool,
    label: &str,
    other_audio: &str,
) where
    T: Sample,
    f32: FromSample<T>,
{
    let mut peak = 0.0_f32;
    if let Ok(mut guard) = writer.try_lock()
        && let Some(writer) = guard.as_mut()
    {
        for &sample in samples {
            let sample = f32::from_sample(sample);
            peak = peak.max(sample.abs().min(1.0));
            if let Err(error) = writer.write_sample(sample) {
                set_failure(
                    failure,
                    format!("{label} could not be written. {other_audio} ({error})."),
                );
                break;
            }
        }
        if checkpoint && let Err(error) = writer.flush() {
            set_failure(
                failure,
                format!(
                    "{label} recovery data could not be checkpointed. {other_audio} ({error})."
                ),
            );
        }
    }
    level_bits.store(peak.to_bits(), Ordering::Release);
}

pub fn microphone_devices() -> Vec<AudioDevice> {
    let host = cpal::default_host();
    let default_name = host
        .default_input_device()
        .and_then(|device| device.name().ok());
    let mut names = host
        .input_devices()
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|device| device.name().ok())
        .collect::<BTreeSet<_>>();
    let mut devices = Vec::new();
    if let Some(default_name) = default_name {
        names.remove(&default_name);
        devices.push(AudioDevice {
            id: "default".to_owned(),
            name: format!("Default — {default_name}"),
            kind: AudioDeviceKind::Default,
            is_default: true,
        });
    }
    devices.extend(names.into_iter().map(|name| AudioDevice {
        id: format!("{DEVICE_ID_PREFIX}{name}"),
        name,
        kind: AudioDeviceKind::Microphone,
        is_default: false,
    }));
    devices
}

fn microphone_name_for_id(device_id: &str) -> XcapRecordingResult<Option<String>> {
    if device_id == "default" {
        return Ok(None);
    }
    let name = device_id
        .strip_prefix(DEVICE_ID_PREFIX)
        .filter(|name| !name.is_empty())
        .ok_or(XcapRecordingError::TargetUnavailable)?;
    let available = cpal::default_host()
        .input_devices()
        .map_err(audio_error)?
        .filter_map(|device| device.name().ok())
        .any(|candidate| candidate == name);
    if available {
        Ok(Some(name.to_owned()))
    } else {
        Err(XcapRecordingError::TargetUnavailable)
    }
}

pub(crate) fn prepare_audio_path(path: &Path) -> XcapRecordingResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

pub(crate) fn usable_audio_path(path: &Path) -> Option<PathBuf> {
    fs::metadata(path)
        .ok()
        .filter(|metadata| metadata.len() > 44)
        .map(|_| path.to_path_buf())
}

pub(crate) fn remove_audio_file(path: &Path) -> XcapRecordingResult<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn set_failure(failure: &Arc<Mutex<Option<String>>>, message: String) {
    let mut current = failure
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if current.is_none() {
        *current = Some(message);
    }
}

pub(crate) fn finalize_writer(writer: &WriterHandle) -> XcapRecordingResult<()> {
    let writer = writer
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take();
    if let Some(writer) = writer {
        writer.finalize().map_err(audio_error)?;
    }
    Ok(())
}

pub(crate) fn audio_error(error: impl std::fmt::Display) -> XcapRecordingError {
    XcapRecordingError::Audio(error.to_string())
}

pub(crate) fn elapsed_milliseconds(started_at: Instant) -> i64 {
    i64::try_from(started_at.elapsed().as_millis()).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::{FEEDBACK_VOLUME, feedback_sample};

    #[test]
    fn feedback_tone_fades_in_and_stops_at_its_duration() {
        assert_eq!(feedback_sample(0, 4_800, 384, 48_000.0, 880.0), 0.0);
        assert_eq!(feedback_sample(4_800, 4_800, 384, 48_000.0, 880.0), 0.0);
        assert!(feedback_sample(2_400, 4_800, 384, 48_000.0, 880.0).abs() <= FEEDBACK_VOLUME);
    }
}
