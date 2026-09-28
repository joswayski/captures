//! Gapless Loop-preview scheduling shared by the video and audio playback
//! readers. One playback stream owns every lap: each reader pre-spawns the
//! next lap's decoder while the current lap plays, and both readers agree on
//! one continue/stop decision per lap boundary so presented video and the
//! mixed audio stay on one continuous timeline.

use std::{
    io,
    process::{Child, ChildStderr, ChildStdout, Command},
    sync::{
        Arc, Condvar, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use crate::{CancelToken, MediaToolError, toolchain::process_message};

const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Source-relative lap ranges and their offsets on the continuous playback
/// timeline. Lap zero starts at the requested position; every later lap
/// replays the accepted `[loop_start, end)` trim.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LapTiming {
    pub(crate) first_start_ms: u64,
    pub(crate) loop_start_ms: u64,
    pub(crate) end_ms: u64,
}

impl LapTiming {
    pub(crate) const fn start_ms(&self, lap: u64) -> u64 {
        if lap == 0 {
            self.first_start_ms
        } else {
            self.loop_start_ms
        }
    }

    pub(crate) const fn duration_ms(&self, lap: u64) -> u64 {
        self.end_ms.saturating_sub(self.start_ms(lap))
    }

    /// Milliseconds of the continuous timeline elapsed before `lap` begins.
    pub(crate) const fn timeline_base_ms(&self, lap: u64) -> u64 {
        if lap == 0 {
            0
        } else {
            self.duration_ms(0)
                .saturating_add((lap - 1).saturating_mul(self.duration_ms(1)))
        }
    }
}

#[derive(Default)]
struct LapGateState {
    /// Boundaries `0..continued` were decided to continue into the next lap.
    continued: u64,
    /// The boundary after which playback ends, once decided.
    stopped_after: Option<u64>,
    /// Laps `0..video_started` delivered at least one video frame.
    video_started: u64,
}

impl LapGateState {
    fn decided(&self, lap: u64) -> Option<bool> {
        if lap < self.continued {
            Some(true)
        } else if self.stopped_after.is_some_and(|stopped| stopped <= lap) {
            Some(false)
        } else {
            None
        }
    }

    fn decide(&mut self, lap: u64, continue_playback: bool) -> bool {
        if let Some(decided) = self.decided(lap) {
            return decided;
        }
        if continue_playback && lap == self.continued {
            self.continued = lap + 1;
            true
        } else {
            self.stopped_after = Some(lap);
            false
        }
    }
}

/// The single continue/stop agreement for each lap boundary. Loop preview is
/// read only when a boundary is decided, so turning it off finishes the
/// current lap. Video decides when its lap reaches EOF; audio, which decodes
/// ahead, may decide first only after the video lap presented a frame and its
/// own ring buffer is nearly drained. An empty video lap never restarts.
pub(crate) struct LapGate {
    looping: Option<Arc<AtomicBool>>,
    state: Mutex<LapGateState>,
    changed: Condvar,
}

impl LapGate {
    pub(crate) fn new(looping: Option<Arc<AtomicBool>>) -> Self {
        Self {
            looping,
            state: Mutex::new(LapGateState::default()),
            changed: Condvar::new(),
        }
    }

    /// Whether a later lap may be requested now; used only to pre-spawn.
    pub(crate) fn looping(&self) -> bool {
        self.looping
            .as_ref()
            .is_some_and(|looping| looping.load(Ordering::Relaxed))
            && self
                .state
                .lock()
                .map(|state| state.stopped_after.is_none())
                .unwrap_or(false)
    }

    #[cfg(test)]
    pub(crate) fn decided(&self, lap: u64) -> Option<bool> {
        self.state.lock().ok().and_then(|state| state.decided(lap))
    }

    pub(crate) fn video_frame(&self, lap: u64) {
        if let Ok(mut state) = self.state.lock()
            && state.video_started <= lap
        {
            state.video_started = lap + 1;
            self.changed.notify_all();
        }
    }

    /// Video reached a clean EOF for `lap`; returns whether the next lap plays.
    pub(crate) fn video_finished(&self, lap: u64, presented_frames: bool) -> bool {
        let looping = self.loop_enabled();
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        let decision = state.decide(lap, presented_frames && looping);
        self.changed.notify_all();
        decision
    }

    /// Audio reached a clean EOF for `lap`. It follows an earlier decision,
    /// or decides once the video lap is known to be nonempty and `due` says
    /// its buffered audio can no longer wait for the video EOF.
    pub(crate) fn audio_finished(&self, lap: u64, due: bool) -> Option<bool> {
        let looping = self.loop_enabled();
        let mut state = self.state.lock().ok()?;
        if let Some(decided) = state.decided(lap) {
            return Some(decided);
        }
        if !due || state.video_started <= lap {
            return None;
        }
        let decision = state.decide(lap, looping);
        self.changed.notify_all();
        Some(decision)
    }

    /// Wait briefly for another reader's progress.
    pub(crate) fn wait(&self, timeout: Duration) {
        if let Ok(state) = self.state.lock() {
            drop(self.changed.wait_timeout(state, timeout));
        }
    }

    fn loop_enabled(&self) -> bool {
        self.looping
            .as_ref()
            .is_some_and(|looping| looping.load(Ordering::Relaxed))
    }
}

#[derive(Default)]
struct ProcessesState {
    closed: bool,
    live: Vec<Weak<Mutex<Child>>>,
}

/// Every decoder process of one playback stream, so teardown can interrupt a
/// reader blocked on a pipe and no process is spawned after close.
#[derive(Default)]
pub(crate) struct PlaybackProcesses {
    state: Mutex<ProcessesState>,
}

impl PlaybackProcesses {
    pub(crate) fn spawn(
        self: &Arc<Self>,
        command: &mut Command,
        tool: &'static str,
        read_diagnostics: fn(&mut ChildStderr) -> io::Result<Vec<u8>>,
    ) -> Result<DecoderProcess, MediaToolError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| MediaToolError::Process("playback process state was poisoned".into()))?;
        if state.closed {
            return Err(MediaToolError::Cancelled);
        }
        let mut child = command.spawn().map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                MediaToolError::ToolUnavailable(tool)
            } else {
                MediaToolError::Io(error)
            }
        })?;
        let (Some(stdout), Some(mut stderr)) = (child.stdout.take(), child.stderr.take()) else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(MediaToolError::Process(
                "failed to read decoded playback output".to_owned(),
            ));
        };
        let stderr_reader = thread::spawn(move || read_diagnostics(&mut stderr));
        let child = Arc::new(Mutex::new(child));
        state.live.retain(|live| live.strong_count() > 0);
        state.live.push(Arc::downgrade(&child));
        Ok(DecoderProcess {
            child,
            stdout: Some(stdout),
            stderr_reader: Some(stderr_reader),
            finished: false,
        })
    }

    /// Refuse later spawns and kill every live decoder.
    pub(crate) fn close(&self) {
        let live = match self.state.lock() {
            Ok(mut state) => {
                state.closed = true;
                std::mem::take(&mut state.live)
            }
            Err(_) => return,
        };
        for child in live.iter().filter_map(Weak::upgrade) {
            if let Ok(mut child) = child.lock() {
                let _ = child.kill();
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn live_count(&self) -> usize {
        self.state
            .lock()
            .map(|state| {
                state
                    .live
                    .iter()
                    .filter(|live| live.strong_count() > 0)
                    .count()
            })
            .unwrap_or(0)
    }
}

/// One decoder process for one lap. Dropping it kills and reaps the process.
pub(crate) struct DecoderProcess {
    child: Arc<Mutex<Child>>,
    stdout: Option<ChildStdout>,
    stderr_reader: Option<thread::JoinHandle<io::Result<Vec<u8>>>>,
    finished: bool,
}

impl DecoderProcess {
    pub(crate) fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.stdout.take()
    }

    /// Wait for a clean exit after its output ended and report failures with
    /// bounded diagnostics. Cancellation or `stop` kills instead.
    pub(crate) fn finish(
        &mut self,
        cancel: &CancelToken,
        stop: &dyn Fn() -> bool,
    ) -> Result<(), MediaToolError> {
        self.stdout = None;
        let mut result = Ok(());
        let status = loop {
            let Ok(mut child) = self.child.lock() else {
                break None;
            };
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if cancel.is_cancelled() || stop() => {
                    let _ = child.kill();
                    result = Err(MediaToolError::Cancelled);
                    break child.wait().ok();
                }
                Ok(None) => {
                    drop(child);
                    thread::sleep(PROCESS_POLL_INTERVAL);
                }
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    result = Err(MediaToolError::Io(error));
                    break None;
                }
            }
        };
        let stderr = self.join_diagnostics(&mut result);
        self.finished = true;
        if result.is_ok()
            && let Some(status) = status
            && !status.success()
        {
            result = Err(MediaToolError::Process(process_message(&stderr)));
        }
        result
    }

    fn join_diagnostics(&mut self, result: &mut Result<(), MediaToolError>) -> Vec<u8> {
        match self.stderr_reader.take().map(thread::JoinHandle::join) {
            None => Vec::new(),
            Some(Ok(Ok(stderr))) => stderr,
            Some(Ok(Err(error))) => {
                if result.is_ok() {
                    *result = Err(MediaToolError::Io(error));
                }
                Vec::new()
            }
            Some(Err(_)) => {
                if result.is_ok() {
                    *result = Err(MediaToolError::Process(
                        "media tool error reader panicked".to_owned(),
                    ));
                }
                Vec::new()
            }
        }
    }
}

impl Drop for DecoderProcess {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        self.stdout = None;
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let mut ignored = Ok(());
        self.join_diagnostics(&mut ignored);
    }
}

/// A reader-facing error message without repeating the error-kind prefix.
pub(crate) fn error_message(error: MediaToolError) -> String {
    match error {
        MediaToolError::Process(message) => message,
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn looping_gate(enabled: bool) -> (LapGate, Arc<AtomicBool>) {
        let looping = Arc::new(AtomicBool::new(enabled));
        (LapGate::new(Some(looping.clone())), looping)
    }

    #[test]
    fn lap_timing_starts_later_laps_at_the_trim_and_keeps_one_timeline() {
        let timing = LapTiming {
            first_start_ms: 701,
            loop_start_ms: 233,
            end_ms: 977,
        };
        assert_eq!(timing.start_ms(0), 701);
        assert_eq!(timing.start_ms(1), 233);
        assert_eq!(timing.start_ms(7), 233);
        assert_eq!(timing.duration_ms(0), 276);
        assert_eq!(timing.duration_ms(1), 744);
        assert_eq!(timing.timeline_base_ms(0), 0);
        assert_eq!(timing.timeline_base_ms(1), 276);
        assert_eq!(timing.timeline_base_ms(2), 276 + 744);
        assert_eq!(timing.timeline_base_ms(3), 276 + 2 * 744);
        // Each lap begins exactly where the previous lap's range ends.
        for lap in 0..5 {
            assert_eq!(
                timing.timeline_base_ms(lap) + timing.duration_ms(lap),
                timing.timeline_base_ms(lap + 1)
            );
        }
    }

    #[test]
    fn video_eof_decides_from_the_loop_flag_and_never_restarts_an_empty_lap() {
        let (gate, looping) = looping_gate(true);
        gate.video_frame(0);
        assert!(gate.video_finished(0, true));
        assert_eq!(gate.decided(0), Some(true));
        assert_eq!(gate.decided(1), None);
        gate.video_frame(1);
        assert!(
            !gate.video_finished(1, false),
            "an empty lap never restarts"
        );
        assert_eq!(gate.decided(1), Some(false));
        assert_eq!(gate.decided(2), Some(false));
        assert!(!gate.looping(), "a stopped stream never pre-spawns");
        looping.store(true, Ordering::Relaxed);
        assert!(!gate.video_finished(1, true), "decisions never change");

        let (gate, looping) = gate_off_mid_lap();
        assert!(!gate.video_finished(0, true));
        assert!(!looping.load(Ordering::Relaxed));

        let silent = LapGate::new(None);
        silent.video_frame(0);
        assert!(!silent.looping());
        assert!(!silent.video_finished(0, true));
    }

    fn gate_off_mid_lap() -> (LapGate, Arc<AtomicBool>) {
        let (gate, looping) = looping_gate(true);
        gate.video_frame(0);
        assert!(gate.looping());
        // Turning Loop off during the lap finishes that lap.
        looping.store(false, Ordering::Relaxed);
        (gate, looping)
    }

    #[test]
    fn audio_waits_for_a_presented_video_lap_and_its_deadline_then_both_agree() {
        let (gate, looping) = looping_gate(true);
        assert_eq!(gate.audio_finished(0, true), None, "video lap not started");
        gate.video_frame(0);
        assert_eq!(gate.audio_finished(0, false), None, "buffer can still wait");
        assert_eq!(gate.audio_finished(0, true), Some(true));
        // Loop turned off after audio committed the next lap: video follows the
        // committed decision so audio and video never diverge.
        looping.store(false, Ordering::Relaxed);
        assert!(gate.video_finished(0, true));
        gate.video_frame(1);
        assert_eq!(gate.audio_finished(1, true), Some(false));
        assert!(!gate.video_finished(1, true));

        // Video EOF first: audio follows whatever video decided, without a due
        // deadline and even though the flag later changes.
        let (gate, looping) = looping_gate(true);
        gate.video_frame(0);
        assert!(gate.video_finished(0, true));
        looping.store(false, Ordering::Relaxed);
        assert_eq!(gate.audio_finished(0, false), Some(true));

        // An empty video lap stops audio too.
        let (gate, _looping) = looping_gate(true);
        assert!(!gate.video_finished(0, false));
        assert_eq!(gate.audio_finished(0, false), Some(false));
    }

    #[cfg(unix)]
    #[test]
    fn processes_report_diagnostics_and_close_kills_blocked_decoders() {
        use std::process::Stdio;

        fn diagnostics(stderr: &mut ChildStderr) -> io::Result<Vec<u8>> {
            let mut bytes = Vec::new();
            io::Read::read_to_end(stderr, &mut bytes)?;
            Ok(bytes)
        }
        fn shell(script: &str) -> Command {
            let mut command = Command::new("sh");
            command
                .args(["-c", script])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            command
        }
        let processes = Arc::new(PlaybackProcesses::default());
        let cancel = CancelToken::default();
        let mut failed = processes
            .spawn(
                &mut shell("echo broken-decoder >&2; exit 3"),
                "FFmpeg",
                diagnostics,
            )
            .unwrap();
        let error = failed.finish(&cancel, &|| false).unwrap_err().to_string();
        assert!(error.contains("broken-decoder"), "{error}");

        let mut clean = processes
            .spawn(&mut shell("exit 0"), "FFmpeg", diagnostics)
            .unwrap();
        clean.finish(&cancel, &|| false).unwrap();

        let mut blocked = processes
            .spawn(&mut shell("exec sleep 30"), "FFmpeg", diagnostics)
            .unwrap();
        let mut stdout = blocked.take_stdout().unwrap();
        assert_eq!(processes.live_count(), 3);
        let reader = thread::spawn(move || {
            let mut bytes = Vec::new();
            io::Read::read_to_end(&mut stdout, &mut bytes).map(|_| bytes.len())
        });
        let started = std::time::Instant::now();
        processes.close();
        assert_eq!(reader.join().unwrap().unwrap(), 0);
        assert!(started.elapsed() < Duration::from_secs(5));
        drop(blocked);
        assert!(matches!(
            processes.spawn(&mut shell("exit 0"), "FFmpeg", diagnostics),
            Err(MediaToolError::Cancelled)
        ));
    }
}
