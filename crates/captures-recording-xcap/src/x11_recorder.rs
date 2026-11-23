//! Display recording frames on Linux, paced to the recording on X11.
//!
//! xcap 0.9.8's X11 recorder grabs back to back with a 1 ms sleep whatever
//! the recording rate, each grab over three new X connections with a
//! per-pixel conversion: about 86% of a core and 67 grabs a second at 1080p
//! under Xvfb, for a 30 fps recording. This reader grabs over one
//! connection no sooner than three quarters of the recording interval after
//! the last grab, as the Windows reader does, so the encoder still finds a
//! new frame at every tick. Frames match xcap's byte for byte. Sessions
//! xcap treats as Wayland, and pixel formats the one-connection grab does
//! not handle, keep xcap's recorder.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use captures_capture::X11OutputGrabber;
use image::RgbaImage;
use xcap::{Frame, Monitor, VideoRecorder, XCapError};

/// How long to wait before grabbing again after a failed grab, as xcap.
const RETRY: Duration = Duration::from_millis(10);

pub(crate) enum LinuxRecorder {
    X11(X11Recorder),
    Xcap(VideoRecorder),
}

impl LinuxRecorder {
    /// Starts recording `monitor`, whose xcap id is `display_id`.
    pub(crate) fn start(
        monitor: &Monitor,
        display_id: &str,
        frames_per_second: u16,
    ) -> Result<(Self, Receiver<Frame>), XCapError> {
        if let Some((recorder, receiver)) = X11Recorder::start(display_id, frames_per_second) {
            return Ok((Self::X11(recorder), receiver));
        }
        let (recorder, receiver) = monitor.video_recorder()?;
        recorder.start()?;
        Ok((Self::Xcap(recorder), receiver))
    }

    pub(crate) fn stop(&self) -> Result<(), XCapError> {
        match self {
            Self::X11(recorder) => {
                recorder.stop();
                Ok(())
            }
            Self::Xcap(recorder) => recorder.stop(),
        }
    }
}

pub(crate) struct X11Recorder {
    stop: Arc<AtomicBool>,
    thread: JoinHandle<()>,
}

impl X11Recorder {
    /// `None` where the one-connection grab is unavailable or its first
    /// grab fails.
    fn start(display_id: &str, frames_per_second: u16) -> Option<(Self, Receiver<Frame>)> {
        let output = display_id.parse().ok()?;
        let grabber = X11OutputGrabber::connect()?;
        let first = grabber.grab(output)?;
        let min_gap = Duration::from_secs_f64(0.75 / f64::from(frames_per_second.max(1)));
        let (sender, receiver) = sync_channel(1);
        let stop = Arc::new(AtomicBool::new(false));
        let thread = thread::Builder::new()
            .name("captures-x11-recorder".into())
            .spawn({
                let stop = stop.clone();
                move || run(&grabber, output, first, min_gap, &sender, &stop)
            })
            .ok()?;
        Some((Self { stop, thread }, receiver))
    }

    /// Ends the grab loop without waiting for it; it closes its connection
    /// on the way out.
    fn stop(&self) {
        self.stop.store(true, Ordering::Release);
        self.thread.thread().unpark();
    }
}

impl Drop for X11Recorder {
    fn drop(&mut self) {
        self.stop();
    }
}

fn run(
    grabber: &X11OutputGrabber,
    output: u32,
    first: RgbaImage,
    min_gap: Duration,
    sender: &SyncSender<Frame>,
    stop: &AtomicBool,
) {
    let mut image = Some(first);
    let mut last = Instant::now();
    while !stop.load(Ordering::Acquire) {
        if let Some(image) = image.take() {
            let frame = Frame::new(image.width(), image.height(), image.into_raw());
            // The receiver takes each frame as it arrives, so the slot is
            // free in practice; when it is not, this frame is skipped
            // rather than blocking a stop.
            if let Err(TrySendError::Disconnected(_)) = sender.try_send(frame) {
                return;
            }
        }
        let now = Instant::now();
        let next = last + min_gap;
        if now < next {
            thread::park_timeout(next - now);
            continue;
        }
        last = now;
        image = grabber.grab(output);
        if image.is_none() {
            thread::park_timeout(RETRY);
        }
    }
}
