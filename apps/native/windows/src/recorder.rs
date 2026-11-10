//! A live recording: New Capture's Record hands its flow, display and target
//! here. The shared `captures_recording_platform` worker owns the session and
//! media off the UI thread; this module runs the recording countdown
//! (`.recording-countdown`, "RECORDING IN"), then the floating controls
//! (`hud`) at the bottom of the display, and publishes the take to History
//! with a mini preview, like the GTK host's `recording_controls`.
//!
//! The controls are topmost and never take focus; dragging the card moves
//! them. They stay out of recordings (`WDA_EXCLUDEFROMCAPTURE`) unless
//! Preferences includes them. Restart and Delete confirm with a system
//! message box. Not ported yet: the HUD tooltips, its region Screenshot, the
//! "Recording controls hidden" notice (New Capture or the tray brings hidden
//! controls back) and recovery of a take left by a crash.

use std::cell::RefCell;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use captures_app::capture_flow::{self, CaptureFlow};
use captures_app::recording_hud::{self as hud, Control};
use captures_capture::DisplayDescriptor;
use captures_recording::{
    AudioOptions, GifOptions, RecordingKind, RecordingOptions, RecordingSessionSnapshot,
    RecordingState, RecordingTarget,
};
use captures_recording_platform::worker::{Command, Event, Worker};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::DirectComposition::*;
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint, ScreenToClient,
    ValidateRect,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

use crate::countdown::{Countdown, Tick};
use crate::gfx::{self, Gfx};
use crate::hud::{self as controls, Hud};
use crate::tokens::Tokens;

/// Shipping `CAPTURE_HUD_HIDE_SETTLE_MS`: the countdown leaves the
/// compositor before the first frame.
const SETTLE_MS: u32 = 40;
const FRAME_TIMER: usize = 1;
const START_TIMER: usize = 2;
const WM_MOUSELEAVE: u32 = 0x02A3;
/// `wparam` 1 asks "Restart recording?", 0 "Delete recording?".
const WM_APP_CONFIRM: u32 = WM_APP + 41;
/// The HUD's region Screenshot, opened once the click handler returns.
const WM_APP_SCREENSHOT: u32 = WM_APP + 42;
/// Shipping `.recording-hud` window: centered, 20 px above the work area.
const BOTTOM: f32 = 20.;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pending {
    Prepare,
    Countdown,
    Start,
    None,
    Change,
    Refresh,
    Restart,
    Finish,
    Discard,
    Confirm,
}

struct Recorder {
    options: crate::Options,
    flow: CaptureFlow,
    worker: Worker,
    display: DisplayDescriptor,
    target: RecordingTarget,
    settings: captures_settings::AppSettings,
    tokens: Tokens,
    root: PathBuf,
    g: Gfx,
    hwnd: HWND,
    scale: f32,
    px: (u32, u32),
    _target: IDCompositionTarget,
    _visual: IDCompositionVisual2,
    surface: IDCompositionSurface,
    hud: Hud,
    snapshot: Option<RecordingSessionSnapshot>,
    pending: Pending,
    accepted: bool,
    polled: Instant,
    poll_pending: bool,
    /// When the running segment's elapsed time was last read.
    clock: Instant,
    errors: hud::ErrorLine,
    countdown: Option<Countdown>,
    shown: bool,
    /// Kept out of New Capture's snapshot even when the controls are
    /// included in captures, like shipping's temporary content protection.
    snapshot_protected: bool,
    pulse: Instant,
    tracking: bool,
    /// Messages to show once the recorder is no longer borrowed.
    notices: Vec<String>,
}

impl Drop for Recorder {
    fn drop(&mut self) {
        crate::live::recording_hidden(false);
        self.countdown = None;
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

thread_local! {
    static RECORDER: RefCell<Option<Recorder>> = const { RefCell::new(None) };
    static REGISTERED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// A recording owns the capture flow from Record's confirmation until the
/// take is saved or deleted.
pub fn active() -> bool {
    RECORDER.with(|r| r.try_borrow().map_or(true, |r| r.is_some()))
}

/// Start the take New Capture handed off. Errors are shown to the user.
pub fn start(
    options: &crate::Options,
    handoff: crate::capture::Handoff,
    wake: impl Fn() + Send + Sync + Clone + 'static,
) {
    match Recorder::new(options, handoff, wake) {
        Ok(recorder) => RECORDER.with(|r| *r.borrow_mut() = Some(recorder)),
        Err(error) => crate::live::show_error(&format!(
            "Could not open the recording controls: {}",
            error.message()
        )),
    }
}

/// Bring hidden controls back (New Capture and the tray do this while a
/// recording runs).
pub fn restore() {
    RECORDER.with(|r| {
        if let Ok(mut r) = r.try_borrow_mut()
            && let Some(r) = r.as_mut()
        {
            r.restore();
        }
    });
}

/// Keep included controls out of a screenshot snapshot while New Capture
/// prepares it; excluded or hidden controls are already out of it.
pub fn protect_for_snapshot(protect: bool) {
    RECORDER.with(|r| {
        if let Ok(mut r) = r.try_borrow_mut()
            && let Some(r) = r.as_mut()
            && r.snapshot_protected != protect
        {
            r.snapshot_protected = protect;
            if r.shown && !r.excluded() {
                let affinity = if protect {
                    WDA_EXCLUDEFROMCAPTURE
                } else {
                    WDA_NONE
                };
                unsafe {
                    let _ = SetWindowDisplayAffinity(r.hwnd, affinity);
                }
            }
        }
    });
}

/// Handle worker events. Call from the host loop outside any window borrow.
pub fn service() {
    let notices = RECORDER.with(|cell| {
        let Ok(mut slot) = cell.try_borrow_mut() else {
            return Vec::new();
        };
        let Some(recorder) = slot.as_mut() else {
            return Vec::new();
        };
        let ended = recorder.drain();
        let notices = std::mem::take(&mut recorder.notices);
        if ended {
            *slot = None;
        }
        notices
    });
    for notice in notices {
        crate::live::show_error(&notice);
    }
}

/// Quit saves an accepted take and keeps a failed one for recovery.
pub fn shutdown() {
    let Some(mut recorder) = RECORDER.with(|r| r.borrow_mut().take()) else {
        return;
    };
    recorder.flow.cancel();
    let generation = recorder.flow.generation();
    let state = recorder.input().state;
    if !matches!(recorder.pending, Pending::Finish | Pending::Discard) {
        if recorder.accepted && state != RecordingState::Failed {
            recorder.worker.send(Command::Finish {
                generation,
                history_root: recorder.root.clone(),
            });
        } else if !recorder.accepted && state != RecordingState::Failed {
            recorder.worker.send(Command::Discard { generation });
        }
    }
    recorder.worker.shutdown();
    while let Some(event) = recorder.worker.try_recv() {
        if let Event::Finished {
            result: Err(error), ..
        } = event
        {
            eprintln!("{error}\nThe recording recovery files have been retained.");
        }
    }
}

impl Recorder {
    fn new(
        options: &crate::Options,
        handoff: crate::capture::Handoff,
        wake: impl Fn() + Send + Sync + Clone + 'static,
    ) -> Result<Self> {
        let crate::capture::Handoff {
            flow,
            display,
            target,
            settings,
            tokens,
        } = handoff;
        let g = match crate::host::with_host(|h| h.g.clone()) {
            Some(g) => g,
            None => Gfx::new(&tokens, true)?,
        };
        let dcomp = g.dcomp.clone().ok_or_else(Error::empty)?;
        let (hwnd, scale, px) = unsafe { controls_window(&display)? };
        let (target_c, visual, surface) = unsafe {
            let target = dcomp.CreateTargetForHwnd(hwnd, true)?;
            let visual = dcomp.CreateVisual()?;
            let surface = g.surface(px.0, px.1)?;
            visual.SetContent(&surface)?;
            target.SetRoot(&visual)?;
            (target, visual, surface)
        };
        let worker = Worker::new(crate::recording::toolchain, wake, capture_flow::is_current);
        let generation = flow.generation();
        worker.send(Command::VerifyToolchain { generation });
        let excluded = !settings.include_recording_controls_in_captures;
        unsafe { SetTimer(Some(hwnd), FRAME_TIMER, 50, None) };
        Ok(Self {
            options: options.clone(),
            flow,
            worker,
            display,
            target,
            settings,
            tokens,
            root: options
                .history_root
                .clone()
                .unwrap_or_else(captures_app::default_history_root),
            g,
            hwnd,
            scale,
            px,
            _target: target_c,
            _visual: visual,
            surface,
            hud: Hud::new(excluded),
            snapshot: None,
            pending: Pending::Prepare,
            accepted: false,
            polled: Instant::now(),
            poll_pending: false,
            clock: Instant::now(),
            errors: hud::ErrorLine::default(),
            countdown: None,
            shown: false,
            snapshot_protected: false,
            pulse: Instant::now(),
            tracking: false,
            notices: Vec::new(),
        })
    }

    fn generation(&self) -> u64 {
        self.flow.generation()
    }

    fn excluded(&self) -> bool {
        !self.settings.include_recording_controls_in_captures
    }

    fn input(&self) -> hud::Input {
        let snapshot = self.snapshot.as_ref();
        hud::Input {
            state: if self.pending == Pending::Finish {
                RecordingState::Finalizing
            } else {
                snapshot.map_or(RecordingState::Countdown, |s| s.state)
            },
            busy: self.pending != Pending::None,
            has_microphone: snapshot
                .is_some_and(|s| s.options.audio.microphone_device_id.is_some()),
            microphone_muted: snapshot.is_some_and(|s| s.options.audio.microphone_muted),
            hide_available: crate::live::resident(),
        }
    }

    /// Pull the HUD's state from the session and repaint it when shown.
    fn refresh(&mut self) {
        self.hud.input = self.input();
        let view = hud::present(&self.hud.input);
        self.hud.elapsed_ms = self.snapshot.as_ref().map_or(0, |s| s.elapsed_ms)
            + if view.timer_running && self.pending != Pending::Finish {
                self.clock.elapsed().as_millis() as u64
            } else {
                0
            };
        self.hud.error = self
            .errors
            .line(self.snapshot.as_ref().and_then(|s| s.error.as_deref()));
        if self.shown {
            let _ = self.render();
        }
    }

    fn render(&mut self) -> Result<()> {
        let reduced = crate::appearance::reduced_motion();
        let pulse = if reduced {
            0.
        } else {
            self.pulse.elapsed().as_secs_f32() / 1.6 % 1.
        };
        let Self {
            hud,
            g,
            tokens,
            surface,
            px,
            scale,
            ..
        } = self;
        gfx::draw_surface(surface, *px, (*scale, *scale), 0., |dc| {
            hud.paint(dc, g, tokens, pulse)
        })?;
        unsafe { g.dcomp.as_ref().ok_or_else(Error::empty)?.Commit() }
    }

    /// Show the controls once the take is accepted or failed.
    fn restore(&mut self) {
        if !(self.accepted || self.input().state == RecordingState::Failed) {
            return;
        }
        self.shown = true;
        crate::live::recording_hidden(false);
        self.refresh();
        unsafe {
            if self.excluded() || self.snapshot_protected {
                let _ = SetWindowDisplayAffinity(self.hwnd, WDA_EXCLUDEFROMCAPTURE);
            }
            let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
        }
    }

    fn hide(&mut self) {
        self.shown = false;
        // Only a user's Hide makes New Capture a restore chord; the countdown
        // hides the controls while Escape still owns the take.
        crate::live::recording_hidden(self.accepted);
        self.hud.hover = None;
        self.hud.pressed = None;
        unsafe {
            // NVIDIA Instant Replay treats an excluded HWND as protected
            // content even while hidden, as the shipping host notes.
            let _ = SetWindowDisplayAffinity(self.hwnd, WDA_NONE);
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
    }

    fn snapshot(&mut self, snapshot: RecordingSessionSnapshot) {
        self.clock = Instant::now();
        self.errors.apply(hud::ErrorEvent::Warning {
            warning: snapshot.warning.clone(),
        });
        self.snapshot = Some(snapshot);
    }

    /// Process queued events; true when the recorder ended.
    fn drain(&mut self) -> bool {
        while let Some(event) = self.worker.try_recv() {
            if self.event(event) {
                return true;
            }
        }
        if !self.flow.is_current()
            && !matches!(
                self.pending,
                Pending::Finish | Pending::Discard | Pending::Countdown
            )
            && self.snapshot.is_some()
        {
            // A session lock saves an accepted take; Escape before
            // acceptance cancels only the pending one.
            self.stop_cancelled();
        }
        if self.pending == Pending::None
            && self.accepted
            && !self.poll_pending
            && self.polled.elapsed() >= Duration::from_millis(500)
        {
            self.poll_pending = true;
            self.polled = Instant::now();
            self.worker.send(Command::Snapshot {
                generation: self.generation(),
            });
        }
        self.refresh();
        false
    }

    fn stop_cancelled(&mut self) {
        if self.accepted {
            self.finish();
        } else {
            self.pending = Pending::Discard;
            self.worker.send(Command::Discard {
                generation: self.generation(),
            });
        }
    }

    fn finish(&mut self) {
        if let Some(snapshot) = self.snapshot.as_mut()
            && snapshot.state == RecordingState::Recording
        {
            snapshot.elapsed_ms += self.clock.elapsed().as_millis() as u64;
        }
        self.clock = Instant::now();
        self.pending = Pending::Finish;
        self.worker.send(Command::Finish {
            generation: self.generation(),
            history_root: self.root.clone(),
        });
    }

    fn options_for_take(&self) -> RecordingOptions {
        let settings = &self.settings.recording;
        RecordingOptions {
            kind: RecordingKind::Video,
            target: self.target.clone(),
            frames_per_second: settings.video_fps,
            max_resolution: settings.video_max_resolution,
            countdown_seconds: settings.countdown_seconds,
            show_cursor: settings.show_cursor,
            highlight_clicks: settings.highlight_clicks,
            show_keystrokes: settings.show_keystrokes,
            audio: AudioOptions {
                capture_system_audio: settings.capture_system_audio,
                microphone_device_id: settings.microphone_device_id.clone(),
                mono_output: settings.mono_audio,
                ..AudioOptions::default()
            },
            gif: GifOptions::default(),
        }
    }

    /// One worker event; true when the recorder ended.
    fn event(&mut self, event: Event) -> bool {
        // A queued start/pause/snapshot reply must not reopen the controls
        // after Stop or Delete was submitted.
        if matches!(self.pending, Pending::Finish | Pending::Discard)
            && !matches!(event, Event::Finished { .. } | Event::Discarded { .. })
        {
            return false;
        }
        match event {
            Event::ToolchainVerified { result, .. } => match result {
                Ok(()) if self.flow.is_current() => {
                    self.worker.send(Command::Prepare {
                        generation: self.generation(),
                        options: self.options_for_take(),
                        recovery_root: self.root.with_file_name("recording-recovery"),
                        display: Some(self.display.clone()),
                    });
                }
                Ok(()) => return self.end(),
                Err(error) => {
                    self.notices.push(error);
                    return self.end();
                }
            },
            Event::Prepared { result, .. } => match result {
                Ok(snapshot) => {
                    self.snapshot(snapshot);
                    self.countdown(false);
                }
                Err(error) => {
                    self.notices.push(error);
                    return self.end();
                }
            },
            Event::Restarted { result, .. } => match result {
                Ok(snapshot) => {
                    self.errors.apply(hud::ErrorEvent::SessionChanged);
                    self.snapshot(snapshot);
                    self.accepted = false;
                    self.countdown(true);
                }
                Err(error) => self.failed(error),
            },
            Event::Started { result, .. } => match result {
                Ok(snapshot) => {
                    self.snapshot(snapshot);
                    if self.flow.disarm_escape().is_ok() {
                        self.accepted = true;
                        self.pending = Pending::None;
                        self.restore();
                    } else {
                        self.stop_cancelled();
                    }
                }
                Err(failure) => {
                    if let Some(snapshot) = failure.snapshot {
                        self.snapshot(*snapshot);
                    }
                    if self.flow.is_current() {
                        self.failed(failure.error);
                    } else {
                        self.stop_cancelled();
                    }
                }
            },
            Event::Paused { result, .. } => match result {
                Ok(snapshot) => {
                    self.snapshot(snapshot);
                    self.pending = Pending::None;
                }
                Err(error) => self.failed(error),
            },
            Event::MicrophoneMuted { result, .. } => match result {
                Ok(snapshot) => {
                    self.snapshot(snapshot);
                    self.pending = Pending::None;
                }
                Err(failure) => {
                    if let Some(snapshot) = failure.snapshot {
                        self.snapshot(*snapshot);
                    }
                    self.failed(failure.error);
                }
            },
            Event::Snapshot {
                result,
                microphone_peak,
                ..
            } => {
                self.poll_pending = false;
                self.hud.level = microphone_peak.clamp(0., 1.);
                match result {
                    Ok(snapshot) => {
                        let failed = snapshot.state == RecordingState::Failed
                            && self
                                .snapshot
                                .as_ref()
                                .is_none_or(|s| s.state != RecordingState::Failed);
                        self.snapshot(snapshot);
                        let refreshed = self.pending == Pending::Refresh;
                        if refreshed {
                            self.pending = Pending::None;
                        }
                        if failed || refreshed {
                            self.restore();
                        }
                    }
                    Err(error) => {
                        // No session to ask again: keep recovery and offer
                        // Retry and Delete.
                        self.pending = Pending::None;
                        if let Some(snapshot) = self.snapshot.as_mut() {
                            snapshot.state = RecordingState::Failed;
                        }
                        self.errors
                            .apply(hud::ErrorEvent::ActionFailed { message: error });
                        self.restore();
                    }
                }
            }
            Event::Finished { result, .. } => match result {
                Ok(recording) => {
                    crate::history::with_history(|h| h.reload());
                    let preview = self
                        .root
                        .join(&recording.entry.id)
                        .join(captures_history::HISTORY_PREVIEW_FILE);
                    crate::previews::show(&self.options, &recording.entry, &preview, false);
                    if self.settings.recording.open_editor_after_recording
                        && let Err(error) =
                            crate::recording::open(&self.options, &recording.entry.id)
                    {
                        eprintln!("couldn't open the recording editor: {error}");
                    }
                    if let Some(warning) = recording.warning {
                        self.notices.push(warning);
                    }
                    return self.end();
                }
                Err(error) => return self.terminal_error(error),
            },
            Event::Discarded { result, .. } => match result {
                Ok(_) => return self.end(),
                Err(error) => return self.terminal_error(error),
            },
            Event::SelectRegion { reply, .. } => {
                let _ = reply.send(None);
            }
            Event::Microphones { .. } | Event::Retired => {}
        }
        false
    }

    fn terminal_error(&mut self, error: String) -> bool {
        if self.flow.is_current() {
            self.failed(error);
            false
        } else {
            self.notices.push(format!(
                "{error}\nThe recording recovery files have been retained."
            ));
            self.end()
        }
    }

    fn failed(&mut self, error: String) {
        // A failed resume or microphone reopen can leave a valid paused take:
        // ask the session for its state before enabling any action.
        self.pending = Pending::Refresh;
        self.poll_pending = true;
        self.worker.send(Command::Snapshot {
            generation: self.generation(),
        });
        self.errors
            .apply(hud::ErrorEvent::ActionFailed { message: error });
        // Escape must not throw away the recovery bundle while the error is read.
        let _ = self.flow.disarm_escape();
        self.restore();
    }

    /// Wait out the recording delay behind the countdown scrim, then start.
    fn countdown(&mut self, restart: bool) {
        let seconds = self.settings.recording.countdown_seconds;
        let result = if restart {
            self.flow.restart_countdown(seconds)
        } else {
            self.flow.start_countdown(seconds)
        };
        if let Err(error) = result {
            if self.flow.is_current() {
                self.failed(error);
            } else {
                self.stop_cancelled();
            }
            return;
        }
        self.pending = Pending::Countdown;
        self.hide();
        if seconds == 0 {
            unsafe { SetTimer(Some(self.hwnd), START_TIMER, SETTLE_MS, None) };
            return;
        }
        let opened = unsafe { countdown_window(&self.display) }.and_then(|hwnd| {
            Countdown::open(
                hwnd,
                &self.display,
                self.g.clone(),
                &self.tokens,
                seconds,
                true,
            )
        });
        match opened {
            Ok(countdown) => self.countdown = Some(countdown),
            Err(error) => self.failed(error.message().to_string()),
        }
    }

    fn tick(&mut self) {
        let current = self.flow.is_current();
        let Some(countdown) = self.countdown.as_mut() else {
            return;
        };
        match countdown.tick(&self.tokens, current) {
            Tick::Running => {}
            Tick::Elapsed => {
                // Dropping the scrim closes it before the first frame.
                self.countdown = None;
                unsafe { SetTimer(Some(self.hwnd), START_TIMER, SETTLE_MS, None) };
            }
            Tick::Closed => {
                self.countdown = None;
                self.stop_cancelled();
            }
        }
    }

    fn begin(&mut self) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), START_TIMER);
        }
        if self.pending != Pending::Countdown {
            return;
        }
        if self.flow.is_current() {
            self.pending = Pending::Start;
            self.worker.send(Command::Start {
                generation: self.generation(),
                exclude_captures_app: self.excluded(),
            });
        } else {
            self.stop_cancelled();
        }
    }

    fn action(&mut self, control: Control) {
        let input = self.input();
        let view = self.hud.view();
        if !view.control(control).enabled {
            return;
        }
        self.errors.apply(hud::ErrorEvent::ActionStarted);
        let generation = self.generation();
        let excluded = self.excluded();
        match control {
            Control::Stop => self.finish(),
            Control::PauseResume => {
                self.pending = Pending::Change;
                self.worker.send(if input.state == RecordingState::Paused {
                    Command::Resume {
                        generation,
                        exclude_captures_app: excluded,
                    }
                } else {
                    Command::Pause { generation }
                });
            }
            Control::Microphone => {
                self.pending = Pending::Change;
                self.worker.send(Command::SetMicrophoneMuted {
                    generation,
                    muted: !input.microphone_muted,
                    exclude_captures_app: excluded,
                });
            }
            Control::Restart if !view.restart_confirms => {
                self.pending = Pending::Restart;
                self.worker.send(Command::Restart { generation });
            }
            Control::Restart | Control::Delete => {
                self.pending = Pending::Confirm;
                let restart = usize::from(control == Control::Restart);
                unsafe {
                    let _ =
                        PostMessageW(Some(self.hwnd), WM_APP_CONFIRM, WPARAM(restart), LPARAM(0));
                }
            }
            // Shipping `start_capture({ mode: "region" })`: the capture bar
            // opens on Region with Record locked while this take runs.
            Control::Screenshot => unsafe {
                let _ = PostMessageW(Some(self.hwnd), WM_APP_SCREENSHOT, WPARAM(0), LPARAM(0));
            },
            Control::Hide => self.hide(),
        }
        self.refresh();
    }

    fn confirmed(&mut self, restart: bool, accepted: bool) {
        if self.pending != Pending::Confirm {
            return;
        }
        let generation = self.generation();
        if !accepted {
            self.pending = Pending::None;
        } else if restart {
            self.pending = Pending::Restart;
            self.worker.send(Command::Restart { generation });
        } else {
            self.pending = Pending::Discard;
            self.worker.send(Command::Discard { generation });
        }
        self.refresh();
    }

    fn end(&mut self) -> bool {
        self.flow.cancel();
        self.worker.send(Command::Retire);
        true
    }

    fn point(&self, lp: LPARAM) -> (f32, f32) {
        (
            (lp.0 & 0xffff) as i16 as f32 / self.scale,
            ((lp.0 >> 16) & 0xffff) as i16 as f32 / self.scale,
        )
    }

    fn cursor(&self) -> (f32, f32) {
        let mut p = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut p);
            let _ = ScreenToClient(self.hwnd, &mut p);
        }
        (p.x as f32 / self.scale, p.y as f32 / self.scale)
    }

    fn handle(&mut self, hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
        if let Some(countdown) = &self.countdown
            && countdown.hwnd == hwnd
        {
            match msg {
                WM_TIMER => self.tick(),
                WM_PAINT => unsafe {
                    let _ = ValidateRect(Some(hwnd), None);
                },
                WM_ERASEBKGND => return Some(LRESULT(1)),
                WM_KEYDOWN if VIRTUAL_KEY(wp.0 as u16) == VK_ESCAPE => {
                    self.flow.cancel();
                    self.tick();
                }
                _ => return None,
            }
            return Some(LRESULT(0));
        }
        if hwnd != self.hwnd {
            return None;
        }
        match msg {
            WM_TIMER if wp.0 == START_TIMER => self.begin(),
            WM_TIMER => {
                if self.shown {
                    self.refresh();
                }
            }
            WM_PAINT => unsafe {
                let _ = ValidateRect(Some(hwnd), None);
            },
            WM_ERASEBKGND => return Some(LRESULT(1)),
            WM_MOUSEACTIVATE => return Some(LRESULT(MA_NOACTIVATE as isize)),
            WM_NCHITTEST => {
                // Buttons are client area; the rest of the card drags the
                // window (shipping `cursor: grab`).
                let (x, y) = self.cursor();
                let caption = self.hud.hit(x, y).is_none() && Hud::over_card(x, y);
                return Some(LRESULT(if caption { HTCAPTION } else { HTCLIENT } as isize));
            }
            WM_NCLBUTTONDBLCLK => {}
            WM_MOUSEMOVE => {
                if !self.tracking {
                    let mut track = TRACKMOUSEEVENT {
                        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    self.tracking = unsafe { TrackMouseEvent(&mut track) }.is_ok();
                }
                let (x, y) = self.point(lp);
                let hover = self.hud.hit(x, y);
                if std::mem::replace(&mut self.hud.hover, hover) != hover {
                    let _ = self.render();
                }
            }
            WM_MOUSELEAVE => {
                self.tracking = false;
                if self.hud.hover.take().is_some() {
                    let _ = self.render();
                }
            }
            WM_LBUTTONDOWN => {
                let (x, y) = self.point(lp);
                self.hud.pressed = self.hud.hit(x, y);
                unsafe { SetCapture(hwnd) };
            }
            WM_LBUTTONUP => {
                let (x, y) = self.point(lp);
                let _ = unsafe { ReleaseCapture() };
                let pressed = self.hud.pressed.take();
                let hit = self.hud.hit(x, y);
                if let Some(control) = pressed.filter(|_| pressed == hit) {
                    self.action(control);
                } else {
                    let _ = self.render();
                }
            }
            WM_SETCURSOR => {
                let (x, y) = self.cursor();
                let cursor = if self.hud.hit(x, y).is_some() {
                    IDC_HAND
                } else {
                    IDC_ARROW
                };
                unsafe { SetCursor(LoadCursorW(None, cursor).ok()) };
                return Some(LRESULT(1));
            }
            _ => return None,
        }
        Some(LRESULT(0))
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_APP_SCREENSHOT {
        crate::live::recording_screenshot();
        return LRESULT(0);
    }
    if msg == WM_APP_CONFIRM {
        // The message box runs its own loop: ask without borrowing.
        let restart = wp.0 == 1;
        let (text, title) = if restart {
            (
                "The current recording will be deleted and a new countdown will begin.",
                "Restart recording?",
            )
        } else {
            (
                "This recording will be deleted permanently.",
                "Delete recording?",
            )
        };
        let answer = unsafe {
            MessageBoxW(
                Some(hwnd),
                &HSTRING::from(text),
                &HSTRING::from(title),
                MB_OKCANCEL | MB_ICONQUESTION | MB_DEFBUTTON2 | MB_TOPMOST,
            )
        };
        RECORDER.with(|r| {
            if let Ok(mut r) = r.try_borrow_mut()
                && let Some(r) = r.as_mut()
            {
                r.confirmed(restart, answer == IDOK);
            }
        });
        return LRESULT(0);
    }
    let handled = RECORDER.with(|r| {
        let mut r = r.try_borrow_mut().ok()?;
        r.as_mut()?.handle(hwnd, msg, wp, lp)
    });
    handled.unwrap_or_else(|| unsafe { DefWindowProcW(hwnd, msg, wp, lp) })
}

unsafe fn register() -> Result<HINSTANCE> {
    unsafe {
        let instance: HINSTANCE = GetModuleHandleW(None)?.into();
        if !REGISTERED.get() {
            for name in [
                w!("CapturesRecordingControls"),
                w!("CapturesRecordingCountdown"),
            ] {
                RegisterClassW(&WNDCLASSW {
                    lpfnWndProc: Some(wndproc),
                    hInstance: instance,
                    lpszClassName: name,
                    ..Default::default()
                });
            }
            REGISTERED.set(true);
        }
        Ok(instance)
    }
}

/// The hidden controls window, centered above the bottom of the display's
/// work area: its handle, scale and pixel size.
unsafe fn controls_window(display: &DisplayDescriptor) -> Result<(HWND, f32, (u32, u32))> {
    unsafe {
        let instance = register()?;
        let center = POINT {
            x: display.x + display.width as i32 / 2,
            y: display.y + display.height as i32 / 2,
        };
        let monitor = MonitorFromPoint(center, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let _ = GetMonitorInfoW(monitor, &mut info);
        let (mut dpi, mut unused) = (96, 96);
        let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi, &mut unused);
        let scale = dpi.max(96) as f32 / 96.;
        let px = (
            (controls::SIZE.0 * scale).round() as u32,
            (controls::SIZE.1 * scale).round() as u32,
        );
        let work = info.rcWork;
        let x = work.left + (work.right - work.left - px.0 as i32) / 2;
        let y = work.bottom - px.1 as i32 - (BOTTOM * scale).round() as i32;
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_NOREDIRECTIONBITMAP,
            w!("CapturesRecordingControls"),
            w!("Captures Recording Controls"),
            WS_POPUP,
            x,
            y,
            px.0 as i32,
            px.1 as i32,
            None,
            None,
            Some(instance),
            None,
        )?;
        Ok((hwnd, scale, px))
    }
}

/// A topmost popup covering `display` for the countdown scrim.
unsafe fn countdown_window(display: &DisplayDescriptor) -> Result<HWND> {
    unsafe {
        let instance = register()?;
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOREDIRECTIONBITMAP,
            w!("CapturesRecordingCountdown"),
            w!("Recording Countdown"),
            WS_POPUP,
            display.x,
            display.y,
            display.width as i32,
            display.height as i32,
            None,
            None,
            Some(instance),
            None,
        )
    }
}
