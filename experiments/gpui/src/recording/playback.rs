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
    generation: Arc<AtomicU64>,
    finished: Arc<AtomicBool>,
    latest: Arc<Mutex<Option<DecodedFrame>>>,
    child: Option<Arc<Mutex<Child>>>,
    reader: Option<JoinHandle<()>>,
}

impl VideoDecoder {
    pub fn new(source: &Path, width: u32, height: u32, duration_ms: u64) -> Self {
        Self {
            ffmpeg: PathBuf::from("ffmpeg"),
            source: source.to_owned(),
            width,
            height,
            duration_ms: duration_ms.max(1),
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

    pub fn restart(&mut self, timestamp_ms: u64, playing: bool) -> anyhow::Result<u64> {
        self.stop();
        let timestamp_ms = timestamp_ms.min(self.duration_ms.saturating_sub(1));
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        *self.latest.lock().unwrap() = None;
        self.finished.store(false, Ordering::Release);
        let mut command = Command::new(&self.ffmpeg);
        command
            .args(["-hide_banner", "-loglevel", "error", "-re", "-ss"])
            .arg(format!("{:.3}", timestamp_ms as f64 / 1000.0))
            .arg("-i")
            .arg(&self.source);
        // A paused seek must retain its exact first frame even if the UI thread
        // is late polling. Otherwise the latest-frame slot can race past it.
        if !playing {
            command.args(["-frames:v", "1"]);
        }
        let mut child = command
            .args([
                "-an", "-vf", "fps=30", "-pix_fmt", "bgra", "-f", "rawvideo", "pipe:1",
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
                    timestamp_ms: timestamp_ms + index * 1000 / 30,
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

    #[test]
    fn decoder_stop_seek_latest_frame_and_duration() {
        let (_dir, path) = fixture();
        let mut decoder = VideoDecoder::new(&path, 64, 48, 2_000);
        let first = decoder.restart(0, true).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while decoder.latest_at_or_before(first, 1_000).is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let (_, before) = decoder
            .latest_at_or_before(first, 1_000)
            .expect("first decoded frame");
        let second = decoder.restart(1_500, false).unwrap();
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
}
