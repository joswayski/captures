//! Hardware H.264 on Linux: I420 frames are piped to the system FFmpeg's
//! VA-API encoder (Intel, AMD and NVIDIA through nvidia-vaapi-driver). The
//! bundled LGPL FFmpeg is built without VA-API so it keeps starting on
//! machines without libva; recording probes the system one and otherwise
//! keeps the openh264 software encoder.

use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::Mutex,
    thread,
    time::{Duration, Instant},
};

use crate::{H264Mp4Error, H264Mp4Info, rgb_to_i420};

const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// An H.264 MP4 encoded by an FFmpeg child process. Frames are placed on the
/// constant frame-rate grid by timestamp: a late frame fills the slots it
/// missed and `finish` holds the last frame until the end timestamp, so the
/// track lasts as long as the recording, like the software writer's.
pub struct FfmpegH264Writer {
    child: Child,
    stdin: Option<ChildStdin>,
    stderr: Option<thread::JoinHandle<String>>,
    output_path: PathBuf,
    width: usize,
    frame_rate: u16,
    /// Contiguous I420 planes for the newest frame.
    yuv: Vec<u8>,
    frames: u64,
}

impl FfmpegH264Writer {
    /// The VA-API writer when the system FFmpeg encoded a test clip of this
    /// size on a render node. The probe result is cached per size and rate.
    pub fn vaapi(
        output_path: &Path,
        width: u32,
        height: u32,
        frame_rate: u16,
        bitrate_bps: u32,
    ) -> Option<Self> {
        let device = vaapi_device(width, height, frame_rate)?;
        Self::create(
            Path::new("ffmpeg"),
            &vaapi_device_args(&device),
            &vaapi_encoder_args(bitrate_bps, frame_rate),
            output_path,
            width,
            height,
            frame_rate,
        )
        .ok()
    }

    fn create(
        ffmpeg: &Path,
        device_args: &[String],
        encoder_args: &[String],
        output_path: &Path,
        width: u32,
        height: u32,
        frame_rate: u16,
    ) -> Result<Self, H264Mp4Error> {
        if width == 0 || height == 0 || !width.is_multiple_of(2) || !height.is_multiple_of(2) {
            return Err(H264Mp4Error::InvalidDimensions);
        }
        if !(1..=60).contains(&frame_rate) {
            return Err(H264Mp4Error::InvalidFrameRate);
        }
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut child = Command::new(ffmpeg)
            .args(["-hide_banner", "-nostdin", "-loglevel", "error"])
            .args(device_args)
            .args(["-f", "rawvideo", "-pix_fmt", "yuv420p"])
            .args(["-color_range", "tv", "-colorspace", "smpte170m"])
            .arg("-video_size")
            .arg(format!("{width}x{height}"))
            .arg("-framerate")
            .arg(frame_rate.to_string())
            .args(["-i", "pipe:0"])
            .args(encoder_args)
            // The same BT.601 limited-range label the software writer uses.
            .args(["-color_range", "tv", "-colorspace", "smpte170m"])
            .args(["-color_primaries", "smpte170m", "-color_trc", "smpte170m"])
            .args(["-an", "-f", "mp4", "-y"])
            .arg(output_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()?;
        let stdin = child.stdin.take();
        let stderr = child.stderr.take().map(|mut stderr| {
            thread::spawn(move || {
                let mut text = String::new();
                let _ = stderr.read_to_string(&mut text);
                text
            })
        });
        let pixels = width as usize * height as usize;
        Ok(Self {
            child,
            stdin,
            stderr,
            output_path: output_path.to_path_buf(),
            width: width as usize,
            frame_rate,
            yuv: vec![0; pixels + pixels / 2],
            frames: 0,
        })
    }

    pub fn encode_rgb(&mut self, rgb: &[u8], timestamp_ms: u64) -> Result<bool, H264Mp4Error> {
        let pixels = self.yuv.len() / 3 * 2;
        if rgb.len() != pixels * 3 {
            return Err(H264Mp4Error::InvalidFrame);
        }
        let slot = self.slot(timestamp_ms);
        if slot < self.frames {
            return Ok(false);
        }
        let (y, chroma) = self.yuv.split_at_mut(pixels);
        let (u, v) = chroma.split_at_mut(pixels / 4);
        rgb_to_i420(rgb, self.width, y, u, v);
        for _ in self.frames..=slot {
            self.write_frame()?;
        }
        Ok(true)
    }

    pub fn finish(mut self, end_timestamp_ms: u64) -> Result<H264Mp4Info, H264Mp4Error> {
        if self.frames == 0 {
            return Err(H264Mp4Error::EmptyRecording);
        }
        let end = self.slot(end_timestamp_ms);
        while self.frames < end {
            self.write_frame()?;
        }
        drop(self.stdin.take());
        let status = self.child.wait()?;
        let stderr = self
            .stderr
            .take()
            .and_then(|reader| reader.join().ok())
            .unwrap_or_default();
        if !status.success() {
            return Err(H264Mp4Error::Encoder(format!(
                "FFmpeg exited with {status}: {}",
                stderr.trim()
            )));
        }
        Ok(H264Mp4Info {
            duration_ms: end_timestamp_ms.max(1),
            encoded_frames: self.frames,
            skipped_frames: 0,
        })
    }

    /// The nearest frame-grid slot. Rounding, because a tick's millisecond
    /// timestamp truncates (60 FPS tick 1 arrives at 16 ms, slot 0.96).
    fn slot(&self, timestamp_ms: u64) -> u64 {
        ((u128::from(timestamp_ms) * u128::from(self.frame_rate) + 500) / 1_000)
            .try_into()
            .unwrap_or(u64::MAX)
    }

    fn write_frame(&mut self) -> Result<(), H264Mp4Error> {
        let stdin = self.stdin.as_mut().ok_or(H264Mp4Error::EmptyRecording)?;
        if let Err(error) = stdin.write_all(&self.yuv) {
            drop(self.stdin.take());
            let _ = self.child.wait();
            let stderr = self
                .stderr
                .take()
                .and_then(|reader| reader.join().ok())
                .unwrap_or_default();
            return Err(H264Mp4Error::Encoder(format!(
                "FFmpeg stopped reading frames ({error}): {}",
                stderr.trim()
            )));
        }
        self.frames += 1;
        Ok(())
    }
}

impl Drop for FfmpegH264Writer {
    fn drop(&mut self) {
        if self.stdin.is_some() {
            // Abandoned before `finish`: discard the partial file.
            drop(self.stdin.take());
            let _ = self.child.kill();
            let _ = self.child.wait();
            let _ = fs::remove_file(&self.output_path);
        }
    }
}

fn vaapi_device_args(device: &Path) -> Vec<String> {
    vec!["-vaapi_device".into(), device.display().to_string()]
}

fn vaapi_encoder_args(bitrate_bps: u32, frame_rate: u16) -> Vec<String> {
    let bitrate = bitrate_bps.max(250_000).to_string();
    vec![
        "-vf".into(),
        "format=nv12,hwupload".into(),
        "-c:v".into(),
        "h264_vaapi".into(),
        "-b:v".into(),
        bitrate.clone(),
        "-maxrate".into(),
        bitrate,
        "-g".into(),
        (u32::from(frame_rate) * 2).to_string(),
        "-bf".into(),
        "0".into(),
    ]
}

/// The first render node whose VA-API driver encoded a test clip of this
/// size with the system FFmpeg, cached per size and rate for the process.
fn vaapi_device(width: u32, height: u32, frame_rate: u16) -> Option<PathBuf> {
    type Probe = ((u32, u32, u16), Option<PathBuf>);
    static PROBES: Mutex<Vec<Probe>> = Mutex::new(Vec::new());
    let key = (width, height, frame_rate);
    let mut probes = PROBES.lock().unwrap_or_else(|error| error.into_inner());
    if let Some((_, device)) = probes.iter().find(|(probed, _)| *probed == key) {
        return device.clone();
    }
    let mut nodes = fs::read_dir("/dev/dri")
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("renderD"))
        })
        .collect::<Vec<_>>();
    nodes.sort();
    let device = nodes
        .into_iter()
        .find(|node| probe_vaapi(node, width, height, frame_rate));
    probes.push((key, device.clone()));
    device
}

fn probe_vaapi(device: &Path, width: u32, height: u32, frame_rate: u16) -> bool {
    let Ok(mut child) = Command::new("ffmpeg")
        .args(["-hide_banner", "-nostdin", "-loglevel", "error"])
        .args(vaapi_device_args(device))
        .args(["-f", "lavfi", "-i"])
        .arg(format!("color=c=black:s={width}x{height}:r={frame_rate}"))
        .args(["-frames:v", "3"])
        .args(vaapi_encoder_args(1_000_000, frame_rate))
        .args(["-f", "null", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if started.elapsed() < PROBE_TIMEOUT => {
                thread::sleep(Duration::from_millis(20));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::FfmpegH264Writer;

    fn ffprobe(path: &std::path::Path, entries: &str) -> String {
        let output = Command::new("ffprobe")
            .args(["-v", "error", "-select_streams", "v:0", "-count_frames"])
            .args(["-show_entries", entries, "-of", "default=nw=1:nk=1"])
            .arg(path)
            .output()
            .unwrap();
        String::from_utf8(output.stdout).unwrap()
    }

    /// The child-process pipeline, with libx264 standing in for VA-API so it
    /// runs without a GPU. Requires an FFmpeg with libx264 on PATH.
    #[test]
    #[ignore = "requires ffmpeg and ffprobe with libx264 on PATH"]
    fn late_frames_fill_the_grid_and_finish_holds_the_last_frame() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("out.mp4");
        let encoder = ["-c:v", "libx264", "-preset", "ultrafast", "-bf", "0"].map(String::from);
        let mut writer = FfmpegH264Writer::create(
            std::path::Path::new("ffmpeg"),
            &[],
            &encoder,
            &path,
            64,
            48,
            10,
        )
        .unwrap();
        let red = [255_u8, 0, 0].repeat(64 * 48);
        let blue = [0_u8, 0, 255].repeat(64 * 48);
        // Slots 0, then 5 (late: fills 1..=5), a repeat of slot 5, then the
        // end at 1.24 s (slot 12) holds blue through slot 11.
        assert!(writer.encode_rgb(&red, 0).unwrap());
        assert!(writer.encode_rgb(&blue, 520).unwrap());
        assert!(!writer.encode_rgb(&red, 540).unwrap());
        let info = writer.finish(1_240).unwrap();
        assert_eq!(info.encoded_frames, 12);
        assert_eq!(info.duration_ms, 1_240);
        assert_eq!(ffprobe(&path, "stream=nb_read_frames").trim(), "12");
        assert_eq!(ffprobe(&path, "stream=color_range").trim(), "tv");

        // Frame 1 is the late blue frame filling the missed slot, not red.
        let pixel = Command::new("ffmpeg")
            .args(["-v", "error", "-i"])
            .arg(&path)
            .args(["-vf", "select=eq(n\\,1)", "-frames:v", "1"])
            .args(["-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
            .output()
            .unwrap()
            .stdout;
        let center = (24 * 64 + 32) * 3;
        let [r, _, b] = [pixel[center], pixel[center + 1], pixel[center + 2]];
        assert!(b > 200 && r < 50, "frame 1 was not blue: {r} {b}");
    }
}
