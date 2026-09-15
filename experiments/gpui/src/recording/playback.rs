use captures_media::{AudioEdit, RecordingAudioLayout, audio_mix_filter};
use gpui::RenderImage;
use image::{Frame, RgbaImage};
use std::{
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::JoinHandle,
};

/// Audio is decoded and clocked by FFplay while GPUI renders FFmpeg video
/// frames. Both processes are restarted from the same media timestamp after a
/// seek, rate, or volume change, avoiding a second independently accumulated
/// application clock. `-nodisp` keeps playback embedded in the editor.
pub struct AudioPlayer {
    ffmpeg: PathBuf,
    ffplay: PathBuf,
    source: PathBuf,
    stream_count: usize,
    decoder: Option<Child>,
    output: Option<Child>,
}

impl AudioPlayer {
    pub fn new(source: &Path, stream_count: usize) -> Self {
        Self {
            ffmpeg: PathBuf::from("ffmpeg"),
            ffplay: PathBuf::from("ffplay"),
            source: source.to_owned(),
            stream_count,
            decoder: None,
            output: None,
        }
    }

    pub fn play(
        &mut self,
        timestamp_ms: u64,
        rate: f32,
        edit: &AudioEdit,
        layout: RecordingAudioLayout,
    ) -> anyhow::Result<()> {
        self.stop();
        let rate = rate.clamp(0.5, 2.0);
        let channels = if edit.mono_output { "1" } else { "2" };
        let mix = audio_mix_filter(edit, layout, self.stream_count, false)?;
        let tempo = if rate == 1. {
            "anull".to_owned()
        } else {
            format!("atempo={rate:.3}")
        };
        let filter = format!("{mix};[audio_out]{tempo}[audio_preview]");
        let mut decoder = Command::new(&self.ffmpeg)
            .args(["-hide_banner", "-loglevel", "error", "-ss"])
            .arg(format!("{:.3}", timestamp_ms as f64 / 1000.))
            .arg("-i")
            .arg(&self.source)
            .args(["-filter_complex", &filter, "-map", "[audio_preview]"])
            .args([
                "-f",
                "s16le",
                "-acodec",
                "pcm_s16le",
                "-ar",
                "48000",
                "-ac",
                channels,
                "pipe:1",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let stdout = decoder
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("FFmpeg audio output unavailable"))?;
        let output = match Command::new(&self.ffplay)
            .args([
                "-nodisp",
                "-autoexit",
                "-loglevel",
                "error",
                "-f",
                "s16le",
                "-ar",
                "48000",
                "-ac",
                channels,
                "-i",
                "pipe:0",
            ])
            .stdin(Stdio::from(stdout))
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(child) => child,
            Err(error) => {
                let _ = decoder.kill();
                let _ = decoder.wait();
                return Err(error.into());
            }
        };
        self.decoder = Some(decoder);
        self.output = Some(output);
        Ok(())
    }

    /// Detect an output device disappearing after playback started.
    pub fn check(&mut self) -> anyhow::Result<()> {
        let mut failure = None;
        for (name, slot) in [("FFmpeg", &mut self.decoder), ("ffplay", &mut self.output)] {
            let Some(child) = slot.as_mut() else { continue };
            let Some(status) = child.try_wait()? else {
                continue;
            };
            let mut detail = String::new();
            if let Some(mut stderr) = child.stderr.take() {
                let _ = stderr.read_to_string(&mut detail);
            }
            *slot = None;
            if !status.success() {
                failure = Some(anyhow::anyhow!(
                    "{name} exited {status}: {}",
                    detail
                        .trim()
                        .lines()
                        .last()
                        .unwrap_or("audio output unavailable")
                ));
                break;
            }
        }
        if let Some(error) = failure {
            self.stop();
            Err(error)
        } else {
            Ok(())
        }
    }

    pub fn stop(&mut self) {
        for slot in [&mut self.decoder, &mut self.output] {
            if let Some(mut child) = slot.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

impl Drop for AudioPlayer {
    fn drop(&mut self) {
        self.stop();
    }
}

#[derive(Clone)]
struct DecodedFrame {
    generation: u64,
    timestamp_ms: u64,
    image: Arc<RenderImage>,
}

/// A bounded decoder: FFmpeg's pipe plus this single latest-frame slot are the
/// only video buffers retained. Killing the separately-owned child closes a
/// blocked stdout read, so shutdown never depends on the reader making progress.
pub struct VideoDecoder {
    ffmpeg: PathBuf,
    source: PathBuf,
    width: u32,
    height: u32,
    duration_ms: u64,
    frame_rate: f64,
    generation: Arc<AtomicU64>,
    finished: Arc<AtomicBool>,
    latest: Arc<Mutex<Option<DecodedFrame>>>,
    child: Option<Arc<Mutex<Child>>>,
    reader: Option<JoinHandle<()>>,
}

impl VideoDecoder {
    pub fn new(source: &Path, width: u32, height: u32, duration_ms: u64, frame_rate: f64) -> Self {
        Self {
            ffmpeg: PathBuf::from("ffmpeg"),
            source: source.to_owned(),
            width,
            height,
            duration_ms: duration_ms.max(1),
            frame_rate: frame_rate.clamp(1.0, 240.0),
            generation: Arc::new(AtomicU64::new(0)),
            finished: Arc::new(AtomicBool::new(false)),
            latest: Arc::new(Mutex::new(None)),
            child: None,
            reader: None,
        }
    }

    pub fn finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }

    pub fn restart(&mut self, timestamp_ms: u64, playing: bool, rate: f32) -> anyhow::Result<u64> {
        self.stop();
        let timestamp_ms = timestamp_ms.min(self.duration_ms.saturating_sub(1));
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        *self.latest.lock().unwrap() = None;
        self.finished.store(false, Ordering::Release);
        let mut command = Command::new(&self.ffmpeg);
        command.args(["-hide_banner", "-loglevel", "error"]);
        if playing {
            command.args(["-readrate", &format!("{:.3}", rate.clamp(0.5, 2.0))]);
        }
        command
            .arg("-ss")
            .arg(format!("{:.3}", timestamp_ms as f64 / 1000.0))
            .arg("-i")
            .arg(&self.source);
        // A paused seek must retain its exact first frame even if the UI thread
        // is late polling. Otherwise the latest-frame slot can race past it.
        if !playing {
            command.args(["-frames:v", "1"]);
        }
        let mut child = command
            // rawvideo has no timestamp channel. Normalize VFR input to the
            // probed average cadence so emitted frames and the labels below
            // share one deterministic CFR timeline instead of mislabeling the
            // source's irregular frame sequence as constant-rate.
            .arg("-vf")
            .arg(format!("fps={:.6}", self.frame_rate))
            .args([
                "-an",
                "-fps_mode",
                "cfr",
                "-pix_fmt",
                "bgra",
                "-f",
                "rawvideo",
                "pipe:1",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("FFmpeg stdout unavailable"))?;
        let child = Arc::new(Mutex::new(child));
        let child_for_reader = child.clone();
        let latest = self.latest.clone();
        let live_generation = self.generation.clone();
        let finished = self.finished.clone();
        let (width, height) = (self.width, self.height);
        let frame_rate = self.frame_rate;
        self.reader = Some(std::thread::spawn(move || {
            let frame_len = width as usize * height as usize * 4;
            let mut index = 0u64;
            loop {
                let mut bytes = vec![0; frame_len];
                if stdout.read_exact(&mut bytes).is_err() {
                    break;
                }
                if live_generation.load(Ordering::Acquire) != generation {
                    break;
                }
                // RenderImage is explicitly documented as BGRA. RgbaImage is
                // only the image crate's four-byte storage container here.
                let Some(buffer) = RgbaImage::from_raw(width, height, bytes) else {
                    break;
                };
                let frame = DecodedFrame {
                    generation,
                    timestamp_ms: timestamp_ms + (index as f64 * 1000. / frame_rate).round() as u64,
                    image: Arc::new(RenderImage::new(vec![Frame::new(buffer)])),
                };
                *latest.lock().unwrap() = Some(frame);
                index += 1;
            }
            let _ = child_for_reader.lock().unwrap().wait();
            finished.store(true, Ordering::Release);
        }));
        self.child = Some(child);
        Ok(generation)
    }

    pub fn latest_at_or_before(
        &self,
        generation: u64,
        clock_ms: u64,
    ) -> Option<(u64, Arc<RenderImage>)> {
        self.latest
            .lock()
            .unwrap()
            .as_ref()
            .filter(|frame| frame.generation == generation && frame.timestamp_ms <= clock_ms)
            .map(|frame| (frame.timestamp_ms, frame.image.clone()))
    }

    pub fn stop(&mut self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        if let Some(child) = self.child.take() {
            let mut child = child.lock().unwrap();
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

impl Drop for VideoDecoder {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        process::Command,
        time::{Duration, Instant},
    };

    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("moving.mp4");
        let status = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=64x48:rate=30:duration=2",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(&path)
            .status()
            .expect("FFmpeg is required for GPUI playback tests");
        assert!(status.success(), "FFmpeg must generate the moving fixture");
        (dir, path)
    }

    #[cfg(unix)]
    #[test]
    fn preview_pcm_respects_independent_tracks_volume_mute_and_channel_count() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("three-tracks.mka");
        assert!(
            Command::new("ffmpeg")
                .args([
                    "-v",
                    "error",
                    "-y",
                    "-f",
                    "lavfi",
                    "-i",
                    "anullsrc=r=48000:cl=stereo",
                    "-f",
                    "lavfi",
                    "-i",
                    "aevalsrc=0.2*sin(2*PI*440*t)|0.1*sin(2*PI*660*t):s=48000",
                    "-f",
                    "lavfi",
                    "-i",
                    "aevalsrc=0.4*sin(2*PI*880*t):s=48000",
                    "-map",
                    "0:a",
                    "-map",
                    "1:a",
                    "-map",
                    "2:a",
                    "-t",
                    "0.25",
                    "-c:a",
                    "pcm_s16le",
                ])
                .arg(&source)
                .status()
                .unwrap()
                .success()
        );
        let pcm = directory.path().join("preview.raw");
        let sink = directory.path().join("audio-sink");
        std::fs::write(
            &sink,
            format!("#!/bin/sh\nexec cat > '{}'\n", pcm.display()),
        )
        .unwrap();
        std::fs::set_permissions(&sink, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut player = AudioPlayer::new(&source, 3);
        player.ffplay = sink;
        let mut render = |edit: &AudioEdit| {
            player
                .play(
                    0,
                    1.,
                    edit,
                    RecordingAudioLayout {
                        system_audio: true,
                        microphone_audio: true,
                    },
                )
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            while player.decoder.is_some() || player.output.is_some() {
                player.check().unwrap();
                assert!(Instant::now() < deadline, "audio children did not finish");
                std::thread::sleep(Duration::from_millis(10));
            }
            std::fs::read(&pcm)
                .unwrap()
                .chunks_exact(2)
                .map(|v| f64::from(i16::from_le_bytes([v[0], v[1]])) / 32768.)
                .collect::<Vec<_>>()
        };
        let amplitude = |samples: &[f64], channel: usize, frequency: f64| {
            let frames = samples.len() / 2;
            2. * samples
                .iter()
                .skip(channel)
                .step_by(2)
                .enumerate()
                .map(|(i, sample)| {
                    sample * (2. * std::f64::consts::PI * frequency * i as f64 / 48000.).sin()
                })
                .sum::<f64>()
                / frames as f64
        };
        let microphone = render(&AudioEdit {
            system_volume: 0.25,
            microphone_volume: 0.75,
            mute_system_audio: true,
            ..Default::default()
        });
        assert_eq!(microphone.len(), 24_000);
        // Mono microphone is centered into stereo with the FFmpeg -3 dB
        // coefficient: 0.4 × 0.75 / sqrt(2), not the mixed track's silence.
        assert!((amplitude(&microphone, 0, 880.) - 0.3 / 2_f64.sqrt()).abs() < 0.001);
        assert!(amplitude(&microphone, 0, 440.).abs() < 0.001);
        assert!((amplitude(&microphone, 0, 880.) - amplitude(&microphone, 1, 880.)).abs() < 0.001);
        let edit = AudioEdit {
            system_volume: 0.25,
            microphone_volume: 0.75,
            mute_microphone: true,
            ..Default::default()
        };
        let system = render(&edit);
        assert!((amplitude(&system, 0, 440.) - 0.05).abs() < 0.001);
        assert!((amplitude(&system, 1, 660.) - 0.025).abs() < 0.001);
        assert!(amplitude(&system, 0, 880.).abs() < 0.001);
        let mono = render(&AudioEdit {
            mono_output: true,
            ..edit
        });
        assert_eq!(
            mono.len(),
            12_000,
            "mono PCM must not be interpreted as stereo"
        );
    }

    #[test]
    fn decoder_stop_seek_latest_frame_and_duration() {
        let (_dir, path) = fixture();
        let mut decoder = VideoDecoder::new(&path, 64, 48, 2_000, 30.0);
        let first = decoder.restart(0, true, 1.0).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while decoder.latest_at_or_before(first, 1_000).is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let (_, before) = decoder
            .latest_at_or_before(first, 1_000)
            .expect("first decoded frame");
        let second = decoder.restart(1_500, false, 1.0).unwrap();
        assert_ne!(first, second);
        assert!(
            decoder.latest_at_or_before(first, 2_000).is_none(),
            "old generation fenced"
        );
        let deadline = Instant::now() + Duration::from_secs(2);
        while decoder.latest_at_or_before(second, 2_000).is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let (at, after) = decoder
            .latest_at_or_before(second, 2_000)
            .expect("seek frame");
        assert_eq!(at, 1_500);
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(decoder.latest_at_or_before(second, 1_500).unwrap().0, 1_500);
        assert_ne!(
            before.as_bytes(0),
            after.as_bytes(0),
            "moving fixture changed pixels"
        );
        let started = Instant::now();
        decoder.stop();
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "stop reaps promptly"
        );
    }

    #[test]
    fn audio_player_reports_early_backend_exit() {
        let mut player = AudioPlayer::new(Path::new("missing-media"), 1);
        player.ffmpeg = PathBuf::from("false");
        player.ffplay = PathBuf::from("cat");
        player
            .play(
                0,
                1.0,
                &AudioEdit::default(),
                RecordingAudioLayout {
                    system_audio: true,
                    microphone_audio: false,
                },
            )
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        let error = loop {
            match player.check() {
                Err(error) => break error.to_string(),
                Ok(()) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Ok(()) => panic!("fake decoder did not exit"),
            }
        };
        assert!(error.contains("FFmpeg exited"), "{error}");
        assert!(player.decoder.is_none() && player.output.is_none());
    }

    #[test]
    fn asymmetric_vfr_fixture_is_normalized_to_average_cfr_labels() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("vfr.mkv");
        let status = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=64x48:rate=10:duration=1",
                "-vf",
                "setpts=if(lt(N\\,3)\\,N/(30*TB)\\,(N-2)/(6*TB))",
                "-vsync",
                "vfr",
            ])
            .arg(&source)
            .status()
            .unwrap();
        assert!(status.success());
        let mut decoder = VideoDecoder::new(&source, 64, 48, 1_400, 8.0);
        let generation = decoder.restart(0, true, 1.0).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while decoder.latest_at_or_before(generation, 500).is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let (timestamp, _) = decoder
            .latest_at_or_before(generation, 500)
            .expect("normalized frame");
        assert_eq!(timestamp % 125, 0, "labels follow normalized 8 fps cadence");
        assert_ne!(
            timestamp, 333,
            "irregular source PTS must not be labeled as CFR index timing"
        );
    }
}
