use std::{
    io::{self, Read},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering, fence},
    },
    thread,
    time::{Duration, Instant},
};

use cpal::{
    FromSample, Sample, SampleFormat, SizedSample, Stream, StreamConfig,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use ringbuf::{
    HeapRb,
    traits::{Consumer, Observer, Producer, Split},
};

use crate::{CancelToken, MediaToolError};

const BUFFER_MILLISECONDS: u64 = 250;
const PREROLL_MILLISECONDS: u64 = 100;
const PRODUCER_POLL_INTERVAL: Duration = Duration::from_millis(2);
const OUTPUT_ERROR_NONE: u8 = 0;
const OUTPUT_ERROR_DEVICE: u8 = 1;
const OUTPUT_ERROR_BACKEND: u8 = 2;
const OUTPUT_ERROR_UNDERRUN: u8 = 3;
const CLOCK_SEGMENT_CAPACITY: usize = 256;
const MAXIMUM_UNDERRUN_MILLISECONDS: u64 = 2_000;

struct AudioClockSegment {
    source_frame: AtomicU64,
    valid_frames: AtomicU64,
    playback_nanoseconds: AtomicU64,
}

impl AudioClockSegment {
    fn new() -> Self {
        Self {
            source_frame: AtomicU64::new(0),
            valid_frames: AtomicU64::new(0),
            playback_nanoseconds: AtomicU64::new(0),
        }
    }
}

/// Lock-free bounded history of source-sample/DAC-time segments. The callback
/// writes the sequence odd around updates so readers never combine segments
/// from different publications. Multiple queued device periods remain distinct.
pub(crate) struct AudioPlaybackClock {
    origin: Instant,
    sequence: AtomicU64,
    published_segments: AtomicU64,
    segments: [AudioClockSegment; CLOCK_SEGMENT_CAPACITY],
    sample_rate: u32,
}

impl AudioPlaybackClock {
    fn new(sample_rate: u32) -> Self {
        Self {
            origin: Instant::now(),
            sequence: AtomicU64::new(0),
            published_segments: AtomicU64::new(0),
            segments: std::array::from_fn(|_| AudioClockSegment::new()),
            sample_rate,
        }
    }

    fn publish(&self, source_frame: u64, valid_frames: u64, playback_at: Instant) {
        self.sequence.fetch_add(1, Ordering::AcqRel);
        let published = self.published_segments.load(Ordering::Relaxed);
        let segment = &self.segments[published as usize % CLOCK_SEGMENT_CAPACITY];
        segment.source_frame.store(source_frame, Ordering::Relaxed);
        segment.valid_frames.store(valid_frames, Ordering::Relaxed);
        let nanos = playback_at
            .saturating_duration_since(self.origin)
            .as_nanos()
            .min(u128::from(u64::MAX)) as u64;
        segment.playback_nanoseconds.store(nanos, Ordering::Relaxed);
        self.published_segments
            .store(published.saturating_add(1), Ordering::Relaxed);
        self.sequence.fetch_add(1, Ordering::Release);
    }

    pub(crate) fn position_ms(&self, start_position_ms: u64) -> Option<u64> {
        self.position_at(start_position_ms, Instant::now())
    }

    fn position_at(&self, start_position_ms: u64, now: Instant) -> Option<u64> {
        loop {
            let before = self.sequence.load(Ordering::Acquire);
            if !before.is_multiple_of(2) {
                std::hint::spin_loop();
                continue;
            }
            let published = self.published_segments.load(Ordering::Relaxed);
            if published == 0 {
                fence(Ordering::Acquire);
                if before == self.sequence.load(Ordering::Acquire) {
                    return None;
                }
                continue;
            }
            let first = published.saturating_sub(CLOCK_SEGMENT_CAPACITY as u64);
            let mut selected = None;
            for index in first..published {
                let segment = &self.segments[index as usize % CLOCK_SEGMENT_CAPACITY];
                let source_frame = segment.source_frame.load(Ordering::Relaxed);
                let valid_frames = segment.valid_frames.load(Ordering::Relaxed);
                let playback_nanoseconds = segment.playback_nanoseconds.load(Ordering::Relaxed);
                let playback_at = self
                    .origin
                    .checked_add(Duration::from_nanos(playback_nanoseconds))
                    .unwrap_or(self.origin);
                if selected.is_none() || playback_at <= now {
                    selected = Some((source_frame, valid_frames, playback_at));
                }
                if playback_at > now {
                    break;
                }
            }
            fence(Ordering::Acquire);
            let after = self.sequence.load(Ordering::Acquire);
            if before != after {
                continue;
            }
            let (source_frame, valid_frames, playback_at) = selected?;
            let elapsed_frames = if now > playback_at {
                (now.saturating_duration_since(playback_at).as_secs_f64()
                    * f64::from(self.sample_rate)) as u64
            } else {
                0
            };
            let frame = source_frame.saturating_add(elapsed_frames.min(valid_frames));
            return Some(
                start_position_ms
                    .saturating_add(frame.saturating_mul(1_000) / u64::from(self.sample_rate)),
            );
        }
    }

    fn published_buffer_finished(&self) -> bool {
        loop {
            let before = self.sequence.load(Ordering::Acquire);
            if !before.is_multiple_of(2) {
                std::hint::spin_loop();
                continue;
            }
            let published = self.published_segments.load(Ordering::Relaxed);
            if published == 0 {
                fence(Ordering::Acquire);
                if before == self.sequence.load(Ordering::Acquire) {
                    return false;
                }
                continue;
            }
            let segment = &self.segments[(published - 1) as usize % CLOCK_SEGMENT_CAPACITY];
            let valid_frames = segment.valid_frames.load(Ordering::Relaxed);
            let playback_nanoseconds = segment.playback_nanoseconds.load(Ordering::Relaxed);
            fence(Ordering::Acquire);
            let after = self.sequence.load(Ordering::Acquire);
            if before != after {
                continue;
            }
            let playback_at = self
                .origin
                .checked_add(Duration::from_nanos(playback_nanoseconds))
                .unwrap_or(self.origin);
            let duration =
                Duration::from_secs_f64(valid_frames as f64 / f64::from(self.sample_rate));
            return Instant::now() >= playback_at.checked_add(duration).unwrap_or(playback_at);
        }
    }
}

struct AudioShared {
    clock: AudioPlaybackClock,
    queued_samples: AtomicUsize,
    producer_eof: AtomicBool,
    producer_error: Mutex<Option<String>>,
    output_error: AtomicU8,
    stop: AtomicBool,
}

impl AudioShared {
    fn new(sample_rate: u32) -> Self {
        Self {
            clock: AudioPlaybackClock::new(sample_rate),
            queued_samples: AtomicUsize::new(0),
            producer_eof: AtomicBool::new(false),
            producer_error: Mutex::new(None),
            output_error: AtomicU8::new(OUTPUT_ERROR_NONE),
            stop: AtomicBool::new(false),
        }
    }
}

/// Prepared on and retained by the serialized playback worker. CPAL invokes
/// only the callbacks on backend threads; callers do not move or control the
/// stream from those callbacks.
pub(crate) struct PreparedAudioOutput {
    stream: Stream,
    producer: Option<ringbuf::HeapProd<f32>>,
    shared: Arc<AudioShared>,
    sample_rate: u32,
    channels: u16,
    preroll_samples: usize,
    started: bool,
}

impl PreparedAudioOutput {
    pub(crate) fn prepare(cancel: &CancelToken) -> Result<Self, MediaToolError> {
        if cancel.is_cancelled() {
            return Err(MediaToolError::Cancelled);
        }
        let host = cpal::default_host();
        let device = host.default_output_device().ok_or_else(|| {
            MediaToolError::Process("no default audio output device is available".to_owned())
        })?;
        if cancel.is_cancelled() {
            return Err(MediaToolError::Cancelled);
        }
        let supported = device.default_output_config().map_err(|error| {
            MediaToolError::Process(format!("failed to read the default audio output: {error}"))
        })?;
        if cancel.is_cancelled() {
            return Err(MediaToolError::Cancelled);
        }
        let sample_rate = supported.sample_rate().0;
        let channels = supported.channels();
        if sample_rate == 0 || channels == 0 {
            return Err(MediaToolError::Process(
                "the default audio output has an invalid configuration".to_owned(),
            ));
        }
        let buffer_samples = samples_for_milliseconds(sample_rate, channels, BUFFER_MILLISECONDS)?;
        let preroll_samples =
            samples_for_milliseconds(sample_rate, channels, PREROLL_MILLISECONDS)?;
        let (producer, consumer) = HeapRb::<f32>::new(buffer_samples).split();
        let shared = Arc::new(AudioShared::new(sample_rate));
        let stream = build_output_stream(
            &device,
            &supported.clone().into(),
            supported.sample_format(),
            channels,
            consumer,
            shared.clone(),
        )?;
        if cancel.is_cancelled() {
            return Err(MediaToolError::Cancelled);
        }
        Ok(Self {
            stream,
            producer: Some(producer),
            shared,
            sample_rate,
            channels,
            preroll_samples,
            started: false,
        })
    }

    pub(crate) const fn format(&self) -> AudioOutputFormat {
        AudioOutputFormat {
            sample_rate: self.sample_rate,
            channels: self.channels,
        }
    }

    pub(crate) fn take_producer(&mut self) -> Result<ringbuf::HeapProd<f32>, MediaToolError> {
        self.producer.take().ok_or_else(|| {
            MediaToolError::Process("audio playback producer was already taken".to_owned())
        })
    }

    pub(crate) fn producer_control(&self) -> AudioProducerControl {
        AudioProducerControl(self.shared.clone())
    }

    pub(crate) fn start_if_ready(
        &mut self,
        first_video_frame_ready: bool,
    ) -> Result<bool, MediaToolError> {
        self.check_error()?;
        if self.started {
            return Ok(true);
        }
        let ready = playback_preroll_ready(
            first_video_frame_ready,
            self.shared.queued_samples.load(Ordering::Acquire),
            self.preroll_samples,
            self.shared.producer_eof.load(Ordering::Acquire),
        );
        if !ready {
            return Ok(false);
        }
        self.stream.play().map_err(|error| {
            MediaToolError::Process(format!("failed to start audio playback: {error}"))
        })?;
        self.started = true;
        Ok(true)
    }

    pub(crate) fn position_ms(&self, start_position_ms: u64) -> Option<u64> {
        self.shared.clock.position_ms(start_position_ms)
    }

    pub(crate) fn drained(&self) -> bool {
        !self.started
            || (self.shared.producer_eof.load(Ordering::Acquire)
                && self.shared.queued_samples.load(Ordering::Acquire) == 0
                && self.shared.clock.published_buffer_finished())
    }

    pub(crate) fn check_error(&self) -> Result<(), MediaToolError> {
        let producer_error = self
            .shared
            .producer_error
            .lock()
            .map_err(|_| MediaToolError::Process("audio playback state was poisoned".to_owned()))?
            .clone();
        if let Some(error) = producer_error {
            return Err(MediaToolError::Process(error));
        }
        match self.shared.output_error.load(Ordering::Acquire) {
            OUTPUT_ERROR_NONE => Ok(()),
            OUTPUT_ERROR_DEVICE => Err(MediaToolError::Process(
                "the default audio output device became unavailable".to_owned(),
            )),
            OUTPUT_ERROR_UNDERRUN => Err(MediaToolError::Process(
                "audio playback stalled after a prolonged output underrun".to_owned(),
            )),
            _ => Err(MediaToolError::Process(
                "the default audio output stream failed".to_owned(),
            )),
        }
    }

    pub(crate) fn stop(&self) {
        self.shared.stop.store(true, Ordering::Release);
        let _ = self.stream.pause();
    }
}

fn playback_preroll_ready(
    first_video_frame_ready: bool,
    queued_samples: usize,
    preroll_samples: usize,
    producer_eof: bool,
) -> bool {
    first_video_frame_ready && (queued_samples >= preroll_samples || producer_eof)
}

#[derive(Clone)]
pub(crate) struct AudioProducerControl(Arc<AudioShared>);

#[derive(Clone, Copy)]
pub(crate) struct AudioOutputFormat {
    pub(crate) sample_rate: u32,
    pub(crate) channels: u16,
}

impl AudioProducerControl {
    fn stopped(&self) -> bool {
        self.0.stop.load(Ordering::Acquire)
    }

    fn pushed(&self, count: usize) {
        self.0.queued_samples.fetch_add(count, Ordering::Release);
    }

    fn eof(&self) {
        self.0.producer_eof.store(true, Ordering::Release);
    }

    pub(crate) fn fail(&self, error: String) {
        if let Ok(mut failure) = self.0.producer_error.lock() {
            *failure = Some(error);
        }
    }

    pub(crate) fn position_ms(&self, start_position_ms: u64) -> Option<u64> {
        self.0.clock.position_ms(start_position_ms)
    }
}

pub(crate) fn read_audio_samples(
    mut reader: impl Read,
    mut producer: ringbuf::HeapProd<f32>,
    control: &AudioProducerControl,
    cancel: &CancelToken,
    channels: u16,
) -> io::Result<()> {
    let channel_count = usize::from(channels);
    if channel_count == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "audio output must have at least one channel",
        ));
    }
    let mut bytes = [0_u8; 16 * 1024];
    let mut carry = Vec::with_capacity(3);
    let mut pending_samples = Vec::with_capacity(channel_count);
    loop {
        if cancel.is_cancelled() || control.stopped() {
            return Ok(());
        }
        let count = reader.read(&mut bytes)?;
        if count == 0 {
            if carry.is_empty() && pending_samples.is_empty() {
                control.eof();
                return Ok(());
            }
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "FFmpeg returned a partial interleaved raw audio frame",
            ));
        }
        let mut input = Vec::with_capacity(carry.len() + count);
        input.append(&mut carry);
        input.extend_from_slice(&bytes[..count]);
        let complete = input.len() / 4 * 4;
        carry.extend_from_slice(&input[complete..]);
        pending_samples.extend(
            input[..complete]
                .chunks_exact(4)
                .map(|sample| f32::from_le_bytes(sample.try_into().expect("four-byte sample"))),
        );
        let complete_samples = pending_samples.len() / channel_count * channel_count;
        let mut written = 0;
        while written < complete_samples {
            if cancel.is_cancelled() || control.stopped() {
                return Ok(());
            }
            control.pushed(1);
            if producer.try_push(pending_samples[written]).is_ok() {
                written += 1;
            } else {
                control.0.queued_samples.fetch_sub(1, Ordering::Release);
                thread::sleep(PRODUCER_POLL_INTERVAL);
            }
        }
        pending_samples.drain(..complete_samples);
    }
}

fn samples_for_milliseconds(
    sample_rate: u32,
    channels: u16,
    milliseconds: u64,
) -> Result<usize, MediaToolError> {
    usize::try_from(u64::from(sample_rate) * u64::from(channels) * milliseconds / 1_000)
        .ok()
        .filter(|samples| *samples > 0)
        .ok_or(MediaToolError::IncompleteMetadata)
}

fn build_output_stream(
    device: &cpal::Device,
    config: &StreamConfig,
    format: SampleFormat,
    channels: u16,
    consumer: ringbuf::HeapCons<f32>,
    shared: Arc<AudioShared>,
) -> Result<Stream, MediaToolError> {
    macro_rules! build {
        ($sample:ty) => {
            build_typed_output_stream::<$sample>(device, config, channels, consumer, shared)
        };
    }
    match format {
        SampleFormat::I8 => build!(i8),
        SampleFormat::I16 => build!(i16),
        SampleFormat::I24 => build!(cpal::I24),
        SampleFormat::I32 => build!(i32),
        SampleFormat::I64 => build!(i64),
        SampleFormat::U8 => build!(u8),
        SampleFormat::U16 => build!(u16),
        SampleFormat::U32 => build!(u32),
        SampleFormat::U64 => build!(u64),
        SampleFormat::F32 => build!(f32),
        SampleFormat::F64 => build!(f64),
        _ => Err(MediaToolError::Process(format!(
            "the default audio output uses unsupported {format} samples"
        ))),
    }
}

fn build_typed_output_stream<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    channels: u16,
    mut consumer: ringbuf::HeapCons<f32>,
    shared: Arc<AudioShared>,
) -> Result<Stream, MediaToolError>
where
    T: Sample + SizedSample + FromSample<f32>,
{
    let callback_shared = shared.clone();
    let error_shared = shared;
    let mut consumed_frames = 0_u64;
    let mut consecutive_underrun_frames = 0_u64;
    let maximum_underrun_frames =
        u64::from(config.sample_rate.0) * MAXIMUM_UNDERRUN_MILLISECONDS / 1_000;
    device
        .build_output_stream(
            config,
            move |output: &mut [T], info| {
                let requested_frames = output.len() / usize::from(channels);
                let popped = fill_audio_output(output, channels, &mut consumer);
                callback_shared
                    .queued_samples
                    .fetch_sub(popped, Ordering::Release);
                let valid_frames = u64::try_from(popped / usize::from(channels)).unwrap_or(0);
                let missing_frames = u64::try_from(requested_frames)
                    .unwrap_or(u64::MAX)
                    .saturating_sub(valid_frames);
                consecutive_underrun_frames = next_underrun_frames(
                    consecutive_underrun_frames,
                    missing_frames,
                    callback_shared.producer_eof.load(Ordering::Acquire),
                );
                if consecutive_underrun_frames >= maximum_underrun_frames {
                    callback_shared
                        .output_error
                        .store(OUTPUT_ERROR_UNDERRUN, Ordering::Release);
                }
                let timestamp = info.timestamp();
                let latency = timestamp
                    .playback
                    .duration_since(&timestamp.callback)
                    .unwrap_or_default();
                let now = Instant::now();
                let playback_at = now.checked_add(latency).unwrap_or(now);
                if valid_frames > 0 {
                    callback_shared
                        .clock
                        .publish(consumed_frames, valid_frames, playback_at);
                }
                consumed_frames = consumed_frames.saturating_add(valid_frames);
            },
            move |error| {
                let code = if matches!(error, cpal::StreamError::DeviceNotAvailable) {
                    OUTPUT_ERROR_DEVICE
                } else {
                    OUTPUT_ERROR_BACKEND
                };
                error_shared.output_error.store(code, Ordering::Release);
            },
            None,
        )
        .map_err(|error| {
            MediaToolError::Process(format!("failed to open the default audio output: {error}"))
        })
}

fn fill_audio_output<T>(
    output: &mut [T],
    channels: u16,
    consumer: &mut ringbuf::HeapCons<f32>,
) -> usize
where
    T: Sample + FromSample<f32> + Copy,
{
    fill_audio_output_with_hook(output, channels, consumer, || {})
}

fn fill_audio_output_with_hook<T>(
    output: &mut [T],
    channels: u16,
    consumer: &mut ringbuf::HeapCons<f32>,
    mut on_first_underrun: impl FnMut(),
) -> usize
where
    T: Sample + FromSample<f32> + Copy,
{
    let mut popped = 0_usize;
    let channel_count = usize::from(channels);
    let mut frames = output.chunks_exact_mut(channel_count);
    let mut underrun = false;
    for frame in &mut frames {
        if !underrun && consumer.occupied_len() >= channel_count {
            for sample in frame {
                *sample = T::from_sample(
                    consumer
                        .try_pop()
                        .expect("checked one complete audio frame"),
                );
                popped += 1;
            }
        } else {
            if !underrun {
                underrun = true;
                on_first_underrun();
            }
            frame.fill(T::from_sample(0.0));
        }
    }
    frames.into_remainder().fill(T::from_sample(0.0));
    popped
}

fn next_underrun_frames(current: u64, missing: u64, producer_eof: bool) -> u64 {
    if missing == 0 || producer_eof {
        0
    } else {
        current.saturating_add(missing)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_retains_queued_periods_and_freezes_across_device_latency_gaps() {
        let clock = AudioPlaybackClock::new(48_000);
        let origin = clock.origin;
        clock.publish(0, 480, origin + Duration::from_millis(40));
        clock.publish(480, 480, origin + Duration::from_millis(70));

        assert_eq!(clock.position_at(1_000, origin), Some(1_000));
        assert_eq!(
            clock.position_at(1_000, origin + Duration::from_millis(45)),
            Some(1_005)
        );
        assert_eq!(
            clock.position_at(1_000, origin + Duration::from_millis(60)),
            Some(1_010),
            "the clock freezes between queued valid device periods"
        );
        assert_eq!(
            clock.position_at(1_000, origin + Duration::from_millis(75)),
            Some(1_015),
            "a future callback publication cannot advance the active period"
        );
        assert!(!clock.published_buffer_finished());

        thread::sleep(Duration::from_millis(85));
        assert!(clock.published_buffer_finished());
    }

    #[test]
    fn raw_audio_reader_rejects_partial_samples() {
        let (producer, _consumer) = HeapRb::<f32>::new(8).split();
        let shared = Arc::new(AudioShared::new(48_000));
        let control = AudioProducerControl(shared);
        let error = read_audio_samples(
            &b"12345"[..],
            producer,
            &control,
            &CancelToken::default(),
            2,
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn raw_audio_reader_rejects_a_channel_incomplete_stereo_eof() {
        let (producer, mut consumer) = HeapRb::<f32>::new(8).split();
        let shared = Arc::new(AudioShared::new(48_000));
        let control = AudioProducerControl(shared.clone());
        let samples = [0.25_f32, -0.25, 0.75];
        let bytes = samples
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect::<Vec<_>>();
        let error = read_audio_samples(
            bytes.as_slice(),
            producer,
            &control,
            &CancelToken::default(),
            2,
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
        assert!(!shared.producer_eof.load(Ordering::Acquire));
        assert_eq!(
            consumer.occupied_len(),
            2,
            "only one complete frame is queued"
        );
        let mut complete_frame = [0.0_f32; 2];
        assert_eq!(consumer.pop_slice(&mut complete_frame), 2);
        assert_eq!(complete_frame, [0.25, -0.25]);
    }

    #[test]
    fn raw_audio_reader_cancels_while_the_bounded_ring_is_full() {
        let (producer, _consumer) = HeapRb::<f32>::new(2).split();
        let shared = Arc::new(AudioShared::new(48_000));
        let control = AudioProducerControl(shared);
        let cancel = CancelToken::default();
        let reader_cancel = cancel.clone();
        let reader_control = control.clone();
        let reader = thread::spawn(move || {
            read_audio_samples(
                &vec![0_u8; 4 * 1_024][..],
                producer,
                &reader_control,
                &reader_cancel,
                1,
            )
        });
        thread::sleep(Duration::from_millis(20));
        cancel.cancel();
        let started = Instant::now();
        reader.join().unwrap().unwrap();
        assert!(started.elapsed() < Duration::from_millis(100));
    }

    #[test]
    fn preroll_waits_for_both_video_and_pcm_without_starting_a_wall_clock() {
        let clock = AudioPlaybackClock::new(48_000);
        let preroll = 9_600;

        assert!(!playback_preroll_ready(false, preroll, preroll, false));
        assert!(!playback_preroll_ready(true, preroll - 1, preroll, false));
        assert!(playback_preroll_ready(true, preroll, preroll, false));
        assert!(playback_preroll_ready(true, 960, preroll, true));
        assert!(!playback_preroll_ready(false, 960, preroll, true));
        assert_eq!(clock.position_ms(7_500), None);

        let first_dac_time = Instant::now() + Duration::from_millis(25);
        clock.publish(0, 480, first_dac_time);
        assert_eq!(clock.position_ms(7_500), Some(7_500));
    }

    #[test]
    fn fake_sink_never_consumes_a_partial_multichannel_frame() {
        let (mut producer, mut consumer) = HeapRb::<f32>::new(8).split();
        assert_eq!(producer.push_slice(&[0.25, -0.25, 0.75]), 3);
        let mut output = [9.0_f32; 4];
        assert_eq!(fill_audio_output(&mut output, 2, &mut consumer), 2);
        assert_eq!(output, [0.25, -0.25, 0.0, 0.0]);
        assert_eq!(consumer.try_pop(), Some(0.75));
    }

    #[test]
    fn fake_sink_latches_silence_after_the_first_shortage_in_each_buffer() {
        let (mut producer, mut consumer) = HeapRb::<f32>::new(8).split();
        assert_eq!(producer.push_slice(&[0.25, -0.25]), 2);
        let mut output = [9.0_f32; 6];
        let popped = fill_audio_output_with_hook(&mut output, 2, &mut consumer, || {
            assert_eq!(producer.push_slice(&[0.75, -0.75]), 2);
        });
        assert_eq!(popped, 2);
        assert_eq!(output, [0.25, -0.25, 0.0, 0.0, 0.0, 0.0]);
        let mut refilled = [0.0_f32; 2];
        assert_eq!(consumer.pop_slice(&mut refilled), 2);
        assert_eq!(refilled, [0.75, -0.75]);
    }

    #[test]
    fn prolonged_underrun_counts_only_live_decoder_silence() {
        let threshold = 48_000 * MAXIMUM_UNDERRUN_MILLISECONDS / 1_000;
        assert_eq!(next_underrun_frames(0, 48_000, false), 48_000);
        assert_eq!(
            next_underrun_frames(48_000, threshold - 48_000, false),
            threshold
        );
        assert_eq!(next_underrun_frames(threshold, 0, false), 0);
        assert_eq!(next_underrun_frames(threshold, 4_800, true), 0);
    }
}
