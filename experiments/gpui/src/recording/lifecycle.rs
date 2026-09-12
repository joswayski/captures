use anyhow::{Context as _, Result, anyhow};
use captures_capture::DisplayDescriptor;
use captures_media::{CancelToken, ExportFormat, RecordingAudioLayout, RecordingSegmentInput};
use captures_recording::{
    DraftStore, RecordingDraftManifest, RecordingKind, RecordingOptions, RecordingSegmentInfo,
    RecordingSegmentManifest, RecordingState, RecordingTarget,
};
use gpui::{
    Animation, AnimationExt, AnyWindowHandle, App, AppContext, Bounds, Context, Global,
    IntoElement, PromptLevel, Render, Timer, Window, WindowBackgroundAppearance, WindowBounds,
    WindowDecorations, WindowKind, WindowOptions, bounds, div, point, prelude::*, px, rgb, rgba,
    size,
};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use super::{
    RecordingSegment, editor,
    model::{HudPhase, PendingAction, format_time},
};
use crate::ui::{icon, metric, theme};

static SESSION_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AfterStop {
    StayPaused,
    Screenshot(bool),
    Resume,
    Restart,
    Finalize,
    Discard,
}

fn after_stop(action: PendingAction, session_available: bool) -> AfterStop {
    match action {
        PendingAction::Screenshot { resume_after } => AfterStop::Screenshot(resume_after),
        PendingAction::MuteRestart | PendingAction::Resume if session_available => {
            AfterStop::Resume
        }
        PendingAction::Restart => AfterStop::Restart,
        PendingAction::Stop => AfterStop::Finalize,
        PendingAction::Discard => AfterStop::Discard,
        PendingAction::LockPause
        | PendingAction::Pause
        | PendingAction::MuteRestart
        | PendingAction::Resume
        | PendingAction::None => AfterStop::StayPaused,
    }
}

fn screenshot_may_resume(
    expected_generation: u64,
    current_generation: u64,
    resume_after: bool,
    phase: HudPhase,
    session_available: bool,
) -> bool {
    expected_generation == current_generation
        && resume_after
        && matches!(phase, HudPhase::Paused)
        && session_available
}

fn may_begin_segment(phase: HudPhase, active: bool) -> bool {
    !active
        && !matches!(
            phase,
            HudPhase::Pausing | HudPhase::Finalizing | HudPhase::Discarding
        )
}

pub(super) struct ActiveRecording {
    hud: gpui::Entity<RecordingHud>,
}

impl Global for ActiveRecording {}

pub(super) struct RecordingHud {
    options: RecordingOptions,
    display: DisplayDescriptor,
    output_directory: PathBuf,
    work_directory: PathBuf,
    store: DraftStore,
    manifest: RecordingDraftManifest,
    segments: Vec<RecordingSegmentInfo>,
    active: Option<RecordingSegment>,
    phase: HudPhase,
    pending: PendingAction,
    generation: u64,
    started: Option<Instant>,
    elapsed_ms: u64,
    hidden: bool,
    error: Option<String>,
    warning: Option<String>,
    hovered_action: Option<&'static str>,
    window: Option<AnyWindowHandle>,
    guide_windows: Vec<AnyWindowHandle>,
    quit_after_terminal: bool,
}

impl RecordingHud {
    fn new(
        options: RecordingOptions,
        display: DisplayDescriptor,
        output_directory: PathBuf,
        store: DraftStore,
        manifest: RecordingDraftManifest,
        work_directory: PathBuf,
        guide_windows: Vec<AnyWindowHandle>,
    ) -> Self {
        let countdown = options.countdown_seconds;
        Self {
            options,
            display,
            output_directory,
            work_directory,
            store,
            manifest,
            segments: Vec::new(),
            active: None,
            phase: if countdown == 0 {
                HudPhase::Starting
            } else {
                HudPhase::Countdown(countdown)
            },
            pending: PendingAction::None,
            generation: 1,
            started: None,
            elapsed_ms: 0,
            hidden: false,
            error: None,
            warning: None,
            hovered_action: None,
            window: None,
            guide_windows,
            quit_after_terminal: false,
        }
    }

    fn total_elapsed_ms(&self) -> u64 {
        self.elapsed_ms
            .saturating_add(self.started.map_or(0, |at| at.elapsed().as_millis() as u64))
    }

    fn schedule_countdown(&mut self, cx: &mut Context<Self>) {
        let generation = self.generation;
        let seconds = match self.phase {
            HudPhase::Countdown(seconds) => seconds,
            HudPhase::Starting => 0,
            _ => return,
        };
        cx.spawn(async move |this, cx| {
            for remaining in (1..=seconds).rev() {
                let current = this.update(cx, |this, cx| {
                    if this.generation != generation
                        || !matches!(this.phase, HudPhase::Countdown(_))
                    {
                        return false;
                    }
                    this.phase = HudPhase::Countdown(remaining);
                    cx.notify();
                    true
                })?;
                if !current {
                    return Ok(());
                }
                Timer::after(Duration::from_secs(1)).await;
            }
            this.update(cx, |this, cx| {
                if this.generation == generation
                    && matches!(this.phase, HudPhase::Countdown(_) | HudPhase::Starting)
                {
                    this.begin_segment(cx);
                }
            })
        })
        .detach();
    }

    fn begin_segment(&mut self, cx: &mut Context<Self>) {
        if !may_begin_segment(self.phase, self.active.is_some()) {
            return;
        }
        if !captures_session::capture_session_available() {
            self.phase = HudPhase::Failed;
            self.error = Some(
                "Screen capture is unavailable while the desktop session is locked or inactive."
                    .into(),
            );
            cx.notify();
            return;
        }
        if !cx
            .global::<crate::settings::Settings>()
            .include_mini_previews_in_captures
        {
            crate::preview::hide(cx);
        }
        self.phase = HudPhase::Starting;
        self.error = None;
        let generation = self.generation;
        let options = self.options.clone();
        let display = self.display.clone();
        let settings = cx.global::<crate::settings::Settings>();
        let exclude_app = !settings.include_mini_previews_in_captures
            && !settings.include_recording_controls_in_captures;
        let path = self.work_directory.join(format!(
            "gpui-segment-{:03}.mp4",
            self.manifest.segments.len()
        ));
        let task = cx.background_spawn(async move {
            if !captures_session::capture_session_available() {
                return Err("Screen capture is unavailable while the desktop session is locked or inactive.".to_owned());
            }
            super::start_segment(&options, &path, &display, exclude_app)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| this.segment_started(generation, result, cx))
        })
        .detach();
        cx.notify();
    }

    fn segment_started(
        &mut self,
        generation: u64,
        result: Result<RecordingSegment, String>,
        cx: &mut Context<Self>,
    ) {
        if generation != self.generation
            || matches!(self.phase, HudPhase::Discarding | HudPhase::Finalizing)
        {
            if let Ok(segment) = result {
                std::thread::spawn(move || {
                    let _ = segment.discard();
                });
            }
            return;
        }
        match result {
            Ok(segment) => {
                self.active = Some(segment);
                self.started = Some(Instant::now());
                if let Err(error) = self.register_active_segment() {
                    if let Some(segment) = self.active.take() {
                        std::thread::spawn(move || {
                            let _ = segment.discard();
                        });
                    }
                    self.fail(error);
                } else {
                    self.phase = HudPhase::Recording;
                    self.start_watchers(cx);
                }
            }
            Err(error) => self.fail(error),
        }
        cx.notify();
    }

    fn start_watchers(&self, cx: &mut Context<Self>) {
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            loop {
                Timer::after(Duration::from_millis(250)).await;
                let keep_going = this.update(cx, |this, cx| {
                    if this.generation != generation
                        || !matches!(this.phase, HudPhase::Recording | HudPhase::Paused)
                    {
                        return false;
                    }
                    if matches!(this.phase, HudPhase::Recording) {
                        if let Some(warning) =
                            this.active.as_ref().and_then(RecordingSegment::warning)
                        {
                            this.warning = Some(warning);
                        }
                        if !captures_session::capture_session_available() {
                            this.pause(PendingAction::LockPause, cx);
                        }
                    }
                    cx.notify();
                    true
                })?;
                if !keep_going {
                    break;
                }
            }
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    fn pause(&mut self, action: PendingAction, cx: &mut Context<Self>) {
        if !matches!(self.phase, HudPhase::Recording) {
            return;
        }
        let Some(segment) = self.active.take() else {
            return;
        };
        self.elapsed_ms = self.total_elapsed_ms();
        self.started = None;
        self.phase = HudPhase::Pausing;
        self.pending = action;
        let generation = self.generation;
        let task =
            cx.background_spawn(async move { segment.stop().map_err(|error| error.to_string()) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| this.segment_stopped(generation, result, cx))
        })
        .detach();
        cx.notify();
    }

    fn segment_stopped(
        &mut self,
        generation: u64,
        result: Result<RecordingSegmentInfo, String>,
        cx: &mut Context<Self>,
    ) {
        if generation != self.generation {
            return;
        }
        match result {
            Ok(info) => {
                if let Err(error) = self.append_completed_segment(info) {
                    self.fail(error);
                    cx.notify();
                    return;
                }
                let action = std::mem::replace(&mut self.pending, PendingAction::None);
                self.phase = HudPhase::Paused;
                match after_stop(action, captures_session::capture_session_available()) {
                    AfterStop::Screenshot(resume_after) => self.invoke_screenshot(resume_after, cx),
                    AfterStop::Resume => self.begin_segment(cx),
                    AfterStop::Restart => self.restart_after_pause(cx),
                    AfterStop::Finalize => self.finalize(cx),
                    AfterStop::Discard => self.discard_after_pause(cx),
                    AfterStop::StayPaused if action == PendingAction::LockPause => {
                        self.warning = Some("The desktop session became locked or inactive. Recording was paused for safety.".into());
                    }
                    AfterStop::StayPaused => {}
                }
            }
            Err(error) => self.fail_retain(error),
        }
        cx.notify();
    }

    fn invoke_screenshot(&mut self, resume_after: bool, cx: &mut Context<Self>) {
        let generation = self.generation;
        let hud = cx.entity().downgrade();
        crate::app::capture_with_completion(
            Box::new(move |cx| {
                let Some(hud) = hud.upgrade() else {
                    return;
                };
                hud.update(cx, |this, cx| {
                    if screenshot_may_resume(
                        generation,
                        this.generation,
                        resume_after,
                        this.phase,
                        captures_session::capture_session_available(),
                    ) {
                        this.begin_segment(cx);
                    }
                });
            }),
            cx,
        );
    }

    fn toggle_microphone(&mut self, cx: &mut Context<Self>) {
        if self.options.audio.microphone_device_id.is_none()
            || !matches!(self.phase, HudPhase::Recording | HudPhase::Paused)
        {
            return;
        }
        self.options.audio.microphone_muted = !self.options.audio.microphone_muted;
        self.manifest.options.audio.microphone_muted = self.options.audio.microphone_muted;
        self.manifest.updated_at_ms = now_ms();
        if let Err(error) = self.store.save(&self.manifest) {
            self.error = Some(error.to_string());
            cx.notify();
            return;
        }
        if matches!(self.phase, HudPhase::Recording) {
            self.pause(PendingAction::MuteRestart, cx);
        } else if captures_session::capture_session_available() {
            self.begin_segment(cx);
        }
    }

    fn request_restart(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.phase.controllable() {
            return;
        }
        if matches!(self.phase, HudPhase::Failed) {
            self.restart_after_pause(cx);
            return;
        }
        let answer = window.prompt(
            PromptLevel::Warning,
            "Restart recording?",
            Some("The current recording will be deleted and a new countdown will begin."),
            &["Restart", "Cancel"],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if answer.await.unwrap_or(1) == 0 {
                this.update(cx, |this, cx| {
                    if matches!(this.phase, HudPhase::Recording) {
                        this.pause(PendingAction::Restart, cx);
                    } else if matches!(this.phase, HudPhase::Paused | HudPhase::Failed) {
                        this.restart_after_pause(cx);
                    }
                })?;
            }
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    fn restart_after_pause(&mut self, cx: &mut Context<Self>) {
        if matches!(
            self.phase,
            HudPhase::Starting | HudPhase::Pausing | HudPhase::Finalizing | HudPhase::Discarding
        ) {
            return;
        }
        self.generation = self.generation.wrapping_add(1);
        self.phase = HudPhase::Starting;
        let old_store = self.store.clone();
        let old_id = self.manifest.session_id.clone();
        let output = self.output_directory.clone();
        let options = self.options.clone();
        let generation = self.generation;
        let task = cx.background_spawn(async move {
            let created = create_draft(&output, &options)?;
            if let Err(error) = old_store.remove(&old_id) {
                let _ = created.0.remove(&created.1.session_id);
                return Err(anyhow::Error::new(error));
            }
            Ok(created)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                match result {
                    Ok((store, manifest, directory)) => {
                        this.store = store;
                        this.manifest = manifest;
                        this.work_directory = directory;
                        this.segments.clear();
                        this.elapsed_ms = 0;
                        this.error = None;
                        this.warning = None;
                        this.phase = if this.options.countdown_seconds == 0 {
                            HudPhase::Starting
                        } else {
                            HudPhase::Countdown(this.options.countdown_seconds)
                        };
                        this.schedule_countdown(cx);
                    }
                    Err(error) => this.fail(error.to_string()),
                }
                cx.notify();
            })
        })
        .detach();
    }

    fn request_stop(&mut self, cx: &mut Context<Self>) {
        match self.phase {
            HudPhase::Recording => self.pause(PendingAction::Stop, cx),
            HudPhase::Paused => self.finalize(cx),
            _ => {}
        }
    }

    fn request_quit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.phase, HudPhase::Finalizing | HudPhase::Discarding) {
            self.quit_after_terminal = true;
            self.warning =
                Some("Captures will quit when the current operation finishes safely.".into());
            cx.notify();
            return;
        }
        let can_save = self.active.is_some() || !self.segments.is_empty();
        let buttons: &[&str] = if can_save {
            &["Stop, save, and quit", "Discard and quit", "Cancel"]
        } else {
            &["Discard and quit", "Cancel"]
        };
        let answer = window.prompt(
            PromptLevel::Warning,
            "Quit during recording?",
            Some(if can_save {
                "Stop safely and save the recording, or explicitly discard it before quitting."
            } else {
                "Recording has not produced recoverable media yet."
            }),
            buttons,
            cx,
        );
        cx.spawn(async move |this, cx| {
            let answer = answer.await.unwrap_or(buttons.len() - 1);
            this.update(cx, |this, cx| {
                if answer == 0 {
                    this.quit_after_terminal = true;
                    if can_save {
                        match this.phase {
                            HudPhase::Recording => this.pause(PendingAction::Stop, cx),
                            HudPhase::Paused | HudPhase::Failed => this.finalize(cx),
                            _ => this.discard_after_pause(cx),
                        }
                    } else {
                        this.discard_after_pause(cx);
                    }
                } else if can_save && answer == 1 {
                    this.quit_after_terminal = true;
                    if matches!(this.phase, HudPhase::Recording) {
                        this.pause(PendingAction::Discard, cx);
                    } else {
                        this.discard_after_pause(cx);
                    }
                }
            })?;
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    fn finalize(&mut self, cx: &mut Context<Self>) {
        if self.segments.is_empty() {
            self.fail("The recording contains no completed media.".into());
            cx.notify();
            return;
        }
        self.phase = HudPhase::Finalizing;
        self.manifest.state = RecordingState::Finalizing;
        self.manifest.updated_at_ms = now_ms();
        if let Err(error) = self.store.save(&self.manifest) {
            self.fail(error.to_string());
            cx.notify();
            return;
        }
        let options = self.options.clone();
        let segments = self.segments.clone();
        let output = self.output_directory.clone();
        let work = self.work_directory.clone();
        let store = self.store.clone();
        let id = self.manifest.session_id.clone();
        let generation = self.generation;
        let task = cx.background_spawn(async move {
            finalize_segments(&options, &segments, &output, &work)?.and_then(|path| {
                store.remove(&id).map_err(|error| error.to_string())?;
                Ok(path)
            })
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                match result {
                    Ok(path) => {
                        crate::app::saved(path.clone(), cx);
                        if !this.quit_after_terminal {
                            let _ = editor::open(path, cx);
                        }
                        this.close_window(cx);
                        if this.quit_after_terminal {
                            cx.quit();
                        }
                    }
                    Err(error) => this.fail_retain(error),
                }
                cx.notify();
            })
        })
        .detach();
    }

    fn request_discard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.phase, HudPhase::Finalizing | HudPhase::Discarding) {
            return;
        }
        let answer = window.prompt(
            PromptLevel::Warning,
            "Delete recording?",
            Some("This recording and its recovery draft will be deleted permanently."),
            &["Delete", "Cancel"],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if answer.await.unwrap_or(1) == 0 {
                this.update(cx, |this, cx| {
                    if matches!(this.phase, HudPhase::Recording) {
                        this.pause(PendingAction::Discard, cx);
                    } else {
                        this.discard_after_pause(cx);
                    }
                })?;
            }
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    fn discard_after_pause(&mut self, cx: &mut Context<Self>) {
        if matches!(self.phase, HudPhase::Finalizing | HudPhase::Discarding) {
            return;
        }
        self.generation = self.generation.wrapping_add(1);
        self.phase = HudPhase::Discarding;
        let store = self.store.clone();
        let id = self.manifest.session_id.clone();
        let task = cx
            .background_spawn(async move { store.remove(&id).map_err(|error| error.to_string()) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        this.close_window(cx);
                        if this.quit_after_terminal {
                            cx.quit();
                        }
                    }
                    Err(error) => this.fail(error),
                }
                cx.notify();
            })
        })
        .detach();
    }

    fn register_active_segment(&mut self) -> Result<(), String> {
        let segment = self.active.as_ref().ok_or("recording segment is missing")?;
        let index = self.manifest.segments.len() as u32;
        let video = self
            .work_directory
            .join(format!("gpui-segment-{index:03}.mp4"));
        let (width, height) = segment.dimensions();
        let (system_audio_relative_path, system_audio_offset_ms) =
            optional_draft_path(&self.work_directory, segment.system_audio_draft_info())?;
        let (microphone_relative_path, microphone_offset_ms) =
            optional_draft_path(&self.work_directory, segment.microphone_draft_info())?;
        self.manifest.segments.push(RecordingSegmentManifest {
            index,
            relative_path: relative_path(&self.work_directory, &video)?,
            system_audio_relative_path,
            system_audio_offset_ms,
            system_audio_warning: None,
            microphone_relative_path,
            microphone_offset_ms,
            microphone_warning: None,
            started_at_ms: now_ms(),
            duration_ms: 0,
            width,
            height,
            size_bytes: 0,
            dropped_frames: 0,
            complete: false,
        });
        self.manifest.state = RecordingState::Recording;
        self.manifest.updated_at_ms = now_ms();
        self.store
            .save(&self.manifest)
            .map_err(|error| error.to_string())
    }

    fn append_completed_segment(&mut self, info: RecordingSegmentInfo) -> Result<(), String> {
        let relative = relative_path(&self.work_directory, &info.path)?;
        let pending = self
            .manifest
            .segments
            .iter_mut()
            .rev()
            .find(|segment| !segment.complete && segment.relative_path == relative)
            .ok_or("recording draft did not contain the active segment")?;
        pending.system_audio_relative_path = info
            .system_audio_path
            .as_ref()
            .map(|p| relative_path(&self.work_directory, p))
            .transpose()?;
        pending.system_audio_offset_ms = info.system_audio_offset_ms;
        pending.system_audio_warning = info.system_audio_warning.clone();
        pending.microphone_relative_path = info
            .microphone_path
            .as_ref()
            .map(|p| relative_path(&self.work_directory, p))
            .transpose()?;
        pending.microphone_offset_ms = info.microphone_offset_ms;
        pending.microphone_warning = info.microphone_warning.clone();
        pending.duration_ms = info.duration_ms;
        pending.width = info.width;
        pending.height = info.height;
        pending.size_bytes = info.size_bytes;
        pending.dropped_frames = info.dropped_frames;
        pending.complete = true;
        self.manifest.state = RecordingState::Paused;
        self.manifest.updated_at_ms = now_ms();
        self.store
            .save(&self.manifest)
            .map_err(|error| error.to_string())?;
        self.segments.push(info);
        Ok(())
    }

    fn fail(&mut self, error: String) {
        self.phase = HudPhase::Failed;
        self.error = Some(error.clone());
        self.manifest.state = RecordingState::Failed;
        self.manifest.last_error = Some(error);
        self.manifest.updated_at_ms = now_ms();
        let _ = self.store.save(&self.manifest);
    }

    fn fail_retain(&mut self, error: String) {
        self.fail(format!(
            "{error} The recoverable draft was retained at {}.",
            self.work_directory.display()
        ));
    }

    fn close_window(&mut self, cx: &mut Context<Self>) {
        for guide in self.guide_windows.drain(..) {
            let _ = cx.update_window(guide, |_, window, _| window.remove_window());
        }
        if let Some(window) = self.window {
            let _ = cx.update_window(window, |_, window, _| window.remove_window());
        }
        let current = cx.entity();
        if cx
            .try_global::<ActiveRecording>()
            .is_some_and(|active| active.hud == current)
        {
            cx.remove_global::<ActiveRecording>();
        }
    }

    fn action_button(
        &self,
        id: &'static str,
        icon_name: &'static str,
        label: &'static str,
        enabled: bool,
        cx: &mut Context<Self>,
        action: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> gpui::Stateful<gpui::Div> {
        let colors = theme(cx);
        div()
            .id(id)
            .relative()
            .size(px(36.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(10.))
            .text_size(px(16.))
            .cursor_pointer()
            .text_color(colors.glass_text())
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                this.hovered_action = hovered.then_some(id);
                cx.notify();
            }))
            .when(enabled, |button| {
                button
                    .hover(|style| style.bg(rgba(0xffffff1f)))
                    .on_click(cx.listener(move |this, _, window, cx| action(this, window, cx)))
            })
            .when(!enabled, |button| button.opacity(0.35))
            .child(
                icon(icon_name)
                    .size(px(18.))
                    .text_color(colors.glass_text()),
            )
            .when(self.hovered_action == Some(id), |button| {
                button.child(
                    div()
                        .absolute()
                        .bottom(px(42.))
                        .px(metric("--s-3"))
                        .py(metric("--s-2"))
                        .rounded(metric("--r-sm"))
                        .bg(colors.glass())
                        .text_size(metric("--text-sm"))
                        .whitespace_nowrap()
                        .child(label),
                )
            })
    }
}

impl Render for RecordingHud {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = theme(cx);
        if self.hidden {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(colors.glass())
                .text_color(colors.glass_text())
                .child(
                    div()
                        .id("restore-recording-controls")
                        .cursor_pointer()
                        .px(metric("--s-6"))
                        .py(metric("--s-4"))
                        .rounded(px(18.))
                        .bg(rgba(0xffffff14))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.hidden = false;
                            window.resize(size(px(620.), px(104.)));
                            cx.notify();
                        }))
                        .child("●  Recording · Show controls"),
                );
        }
        let elapsed = self.total_elapsed_ms();
        let controls = self.phase.controllable() && !matches!(self.phase, HudPhase::Failed);
        let status = match self.phase {
            HudPhase::Countdown(n) => format!("Starting in {n}"),
            _ => self.phase.status().to_owned(),
        };
        let dot = div()
            .size(px(9.))
            .rounded_full()
            .bg(if matches!(self.phase, HudPhase::Paused) {
                rgb(0xf5ad42)
            } else {
                rgb(0xff4f5f)
            });
        let dot = if matches!(self.phase, HudPhase::Recording) {
            dot.with_animation(
                "recording-dot-pulse",
                Animation::new(Duration::from_millis(1_600)).repeat(),
                |dot, delta| dot.opacity(0.8 + 0.2 * (delta * std::f32::consts::TAU).cos()),
            )
            .into_any_element()
        } else {
            dot.into_any_element()
        };
        let actions = div()
            .flex()
            .items_center()
            .gap(px(10.))
            .child(
                div()
                    .w(px(108.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(dot)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(format_time(elapsed, false))
                            .child(div().text_size(px(9.)).opacity(0.62).child(status)),
                    ),
            )
            .child(self.action_button(
                "recording-stop",
                "stop",
                "Stop and save",
                controls,
                cx,
                |this, _, cx| this.request_stop(cx),
            ))
            .child(self.action_button(
                "recording-pause",
                if matches!(self.phase, HudPhase::Paused) {
                    "play"
                } else {
                    "pause"
                },
                if matches!(self.phase, HudPhase::Paused) {
                    "Resume recording"
                } else {
                    "Pause recording"
                },
                controls,
                cx,
                |this, _, cx| {
                    if matches!(this.phase, HudPhase::Paused) {
                        this.pending = PendingAction::Resume;
                        this.begin_segment(cx);
                    } else {
                        this.pause(PendingAction::Pause, cx);
                    }
                },
            ))
            .child(self.action_button(
                "recording-restart",
                "restart",
                "Restart recording",
                self.phase.controllable(),
                cx,
                |this, window, cx| this.request_restart(window, cx),
            ))
            .child(self.action_button(
                "recording-screenshot",
                "screenshot",
                "Take a region screenshot",
                controls,
                cx,
                |this, _, cx| this.pause(PendingAction::Screenshot { resume_after: true }, cx),
            ))
            .child(self.action_button(
                "recording-microphone",
                if self.options.audio.microphone_muted {
                    "microphone-muted"
                } else {
                    "microphone"
                },
                "Mute or unmute microphone",
                controls && self.options.audio.microphone_device_id.is_some(),
                cx,
                |this, _, cx| this.toggle_microphone(cx),
            ))
            .child(self.action_button(
                "recording-discard",
                "trash",
                "Delete recording",
                !matches!(self.phase, HudPhase::Finalizing | HudPhase::Discarding),
                cx,
                |this, window, cx| this.request_discard(window, cx),
            ))
            .child(self.action_button(
                "recording-hide",
                "hide",
                "Hide controls",
                !matches!(self.phase, HudPhase::Discarding),
                cx,
                |this, window, cx| {
                    this.hidden = true;
                    window.resize(size(px(250.), px(52.)));
                    cx.notify();
                },
            ));
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(5.))
            .bg(colors.glass())
            .text_color(colors.glass_text())
            .rounded(px(20.))
            .border_1()
            .border_color(rgba(0xffffff29))
            .on_mouse_down(gpui::MouseButton::Left, |_, window, _| {
                window.start_window_move()
            })
            .child(
                div()
                    .text_size(px(10.))
                    .opacity(0.68)
                    .child("Controls appear in Linux recordings · Hide them to keep them out"),
            )
            .child(actions)
            .when_some(
                self.error.clone().or(self.warning.clone()),
                |root, message| {
                    root.child(
                        div()
                            .max_w(px(580.))
                            .text_size(px(11.))
                            .text_color(rgb(0xffb4b9))
                            .child(message),
                    )
                },
            )
    }
}

struct RegionGuide;

impl Render for RegionGuide {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().bg(theme(cx).color("--theme-signal"))
    }
}

pub(super) fn start(
    options: RecordingOptions,
    display: DisplayDescriptor,
    output_directory: PathBuf,
    cx: &mut App,
) -> Result<()> {
    options.validate().map_err(anyhow::Error::msg)?;
    if cx.try_global::<ActiveRecording>().is_some() {
        return Err(anyhow!("a recording is already active"));
    }
    if !captures_session::capture_session_available() {
        return Err(anyhow!(
            "screen capture is unavailable while the desktop session is locked or inactive"
        ));
    }
    fs::create_dir_all(&output_directory)
        .with_context(|| format!("create {}", output_directory.display()))?;
    let (store, manifest, work_directory) = create_draft(&output_directory, &options)?;
    let guide_options = options.clone();
    let guide_display = display.clone();
    let bounds = Bounds::centered(None, size(px(620.), px(104.)), cx);
    let handle = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: None,
            kind: WindowKind::PopUp,
            is_resizable: false,
            is_minimizable: false,
            window_background: WindowBackgroundAppearance::Transparent,
            window_decorations: Some(WindowDecorations::Client),
            app_id: Some("captures-gpui-recording".into()),
            ..Default::default()
        },
        move |window, cx| {
            window.set_window_title("Captures GPUI Recording controls");
            cx.new(|_| {
                RecordingHud::new(
                    options,
                    display,
                    output_directory,
                    store,
                    manifest,
                    work_directory,
                    Vec::new(),
                )
            })
        },
    )?;
    let guide_windows = match open_region_guides(&guide_options, &guide_display, cx) {
        Ok(windows) => windows,
        Err(error) => {
            let _ = handle.update(cx, |_, window, _| window.remove_window());
            return Err(error);
        }
    };
    handle.update(cx, |hud, window, cx| {
        hud.window = Some(window.window_handle());
        hud.guide_windows = guide_windows;
        hud.schedule_countdown(cx);
    })?;
    let hud = cx.read_window(&handle, |hud, _| hud)?;
    cx.set_global(ActiveRecording { hud });
    cx.defer(|cx| {
        let excluded = !cx
            .global::<crate::settings::Settings>()
            .include_recording_controls_in_captures;
        if let Err(error) =
            crate::desktop::exclude_from_capture("Captures GPUI Recording controls", excluded)
        {
            crate::ui::error(error, cx);
        }
    });
    Ok(())
}

fn open_region_guides(
    options: &RecordingOptions,
    display: &DisplayDescriptor,
    cx: &mut App,
) -> Result<Vec<AnyWindowHandle>> {
    let RecordingTarget::Region { rect, .. } = &options.target else {
        return Ok(Vec::new());
    };
    let mut windows = Vec::with_capacity(4);
    let (origin_x, origin_y) = display.overlay_position();
    for (index, (x, y, width, height)) in
        region_border_geometries(rect, (origin_x.round() as i32, origin_y.round() as i32))
            .into_iter()
            .enumerate()
    {
        let title = format!("Captures GPUI Recording region {index} ({x},{y})");
        let window_title = title.clone();
        let handle = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds(
                    point(px(x as f32), px(y as f32)),
                    size(px(width as f32), px(height as f32)),
                ))),
                titlebar: None,
                focus: false,
                kind: WindowKind::PopUp,
                is_movable: false,
                is_resizable: false,
                is_minimizable: false,
                window_background: WindowBackgroundAppearance::Transparent,
                window_decorations: Some(WindowDecorations::Client),
                app_id: Some("captures-gpui-recording-region-guide".into()),
                ..Default::default()
            },
            move |window, cx| {
                window.set_window_title(&window_title);
                cx.new(|_| RegionGuide)
            },
        );
        match handle {
            Ok(handle) => windows.push(handle.into()),
            Err(error) => {
                for window in windows.drain(..) {
                    let _ = cx.update_window(window, |_, window, _| window.remove_window());
                }
                return Err(error);
            }
        }
        crate::ui::job(
            cx,
            move || {
                std::thread::sleep(Duration::from_millis(100));
                crate::desktop::position_guide(&title, x, y).map_err(|error| error.to_string())
            },
            |result, cx| {
                if let Err(error) = result {
                    crate::ui::error(format!("Cannot position recording guide: {error}"), cx);
                }
            },
        );
    }
    Ok(windows)
}

pub(super) fn restore_controls(cx: &mut App) {
    let Some(active) = cx
        .try_global::<ActiveRecording>()
        .map(|active| active.hud.clone())
    else {
        return;
    };
    active.update(cx, |hud, cx| {
        hud.hidden = false;
        cx.notify();
    });
    for window in cx.windows() {
        let _ = window.update(cx, |_, window, _| window.activate_window());
    }
}

pub(super) fn request_quit(cx: &mut App) {
    let Some(active) = cx
        .try_global::<ActiveRecording>()
        .map(|active| active.hud.clone())
    else {
        cx.quit();
        return;
    };
    let window = active.read(cx).window;
    let Some(window_handle) = window else {
        return;
    };
    let _ = cx.update_window(window_handle, |_, window, cx| {
        active.update(cx, |hud, cx| hud.request_quit(window, cx));
    });
}

fn create_draft(
    output: &Path,
    options: &RecordingOptions,
) -> Result<(DraftStore, RecordingDraftManifest, PathBuf)> {
    let root = crate::settings::data_dir().join("recording-drafts");
    crate::desktop::private_directory(&root)?;
    let store = DraftStore::new(root);
    let manifest = RecordingDraftManifest::new(session_id(), options.clone(), now_ms());
    let directory = store.create(&manifest)?;
    crate::desktop::private_directory(&directory)?;
    fs::write(
        directory.join("destination.txt"),
        output.as_os_str().as_encoded_bytes(),
    )?;
    crate::desktop::private_file(&directory.join("destination.txt"))?;
    Ok((store, manifest, directory))
}

fn session_id() -> String {
    let stamp = now_ms();
    let counter = SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!(
        "{:08x}-{:04x}-4{:03x}-8{:03x}-{:012x}",
        stamp as u32,
        (stamp >> 32) as u16,
        counter & 0xfff,
        std::process::id() & 0xfff,
        stamp ^ counter
    )
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn relative_path(root: &Path, path: &Path) -> Result<String, String> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| "recording draft media escaped its private directory".to_owned())?;
    if relative
        .components()
        .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err("recording draft media path is invalid".into());
    }
    Ok(relative.to_string_lossy().into_owned())
}

fn optional_draft_path(
    root: &Path,
    value: Option<(PathBuf, i64)>,
) -> Result<(Option<String>, i64), String> {
    value.map_or(Ok((None, 0)), |(path, offset)| {
        Ok((Some(relative_path(root, &path)?), offset))
    })
}

fn segment_inputs(segments: &[RecordingSegmentInfo]) -> Vec<RecordingSegmentInput> {
    segments
        .iter()
        .map(|segment| RecordingSegmentInput {
            video_path: segment.path.clone(),
            system_audio_path: segment.system_audio_path.clone(),
            system_audio_offset_ms: segment.system_audio_offset_ms,
            microphone_path: segment.microphone_path.clone(),
            microphone_offset_ms: segment.microphone_offset_ms,
            duration_ms: segment.duration_ms,
        })
        .collect()
}

fn finalize_segments(
    options: &RecordingOptions,
    segments: &[RecordingSegmentInfo],
    output: &Path,
    work: &Path,
) -> Result<Result<PathBuf, String>, String> {
    let format = if options.kind == RecordingKind::Gif {
        ExportFormat::Gif
    } else {
        ExportFormat::Mp4
    };
    let extension = if format == ExportFormat::Gif {
        "gif"
    } else {
        "mp4"
    };
    let staging = work.join(format!("finished.{extension}"));
    let media = crate::media::toolchain();
    media.verify().map_err(|error| error.to_string())?;
    let cancel = CancelToken::default();
    let inputs = segment_inputs(segments);
    if format == ExportFormat::Gif {
        let master = work.join("gif-master.mp4");
        media
            .concatenate_segments(
                &inputs
                    .iter()
                    .map(|input| input.video_path.clone())
                    .collect::<Vec<_>>(),
                &master,
                &cancel,
            )
            .map_err(|error| error.to_string())?;
        media
            .create_gif(
                &master,
                &staging,
                options.frames_per_second,
                options.gif.max_width,
                options.gif.max_colors,
                &cancel,
            )
            .map_err(|error| error.to_string())?;
    } else {
        media
            .assemble_recording_segments(
                &inputs,
                &staging,
                RecordingAudioLayout {
                    system_audio: options.audio.capture_system_audio,
                    microphone_audio: segments
                        .iter()
                        .any(|segment| segment.microphone_path.is_some()),
                },
                &cancel,
            )
            .map_err(|error| error.to_string())?;
    }
    Ok(publish_unique(&staging, output, extension))
}

pub(super) fn publish_unique(
    staging: &Path,
    directory: &Path,
    extension: &str,
) -> Result<PathBuf, String> {
    fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let stamp = now_ms();
    for index in 0.. {
        let path = directory.join(format!(
            "Capture-{stamp}{}.{}",
            if index == 0 {
                String::new()
            } else {
                format!("-{index}")
            },
            extension
        ));
        match fs::hard_link(staging, &path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.to_string()),
        }
    }
    unreachable!()
}

fn region_border_geometries(
    rect: &captures_recording::CaptureRect,
    origin: (i32, i32),
) -> [(i32, i32, u32, u32); 4] {
    let x = origin.0.saturating_add(rect.x);
    let y = origin.1.saturating_add(rect.y);
    let width = rect.width.max(3);
    let height = rect.height.max(3);
    [
        (x, y, width, 3),
        (x, y.saturating_add(height as i32 - 3), width, 3),
        (x, y, 3, height),
        (x.saturating_add(width as i32 - 3), y, 3, height),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use captures_recording::CaptureRect;

    #[test]
    fn region_guide_geometry_adds_nonzero_display_origin() {
        let rect = CaptureRect {
            x: 17,
            y: 23,
            width: 401,
            height: 199,
        };
        let lines = region_border_geometries(&rect, (-1920, 75));
        assert_eq!(lines[0], (-1903, 98, 401, 3));
        assert_eq!(lines[1], (-1903, 294, 401, 3));
        assert_eq!(lines[3], (-1505, 98, 3, 199));
    }

    #[test]
    fn generated_session_ids_are_valid_for_draft_store() {
        let first = session_id();
        let second = session_id();
        assert_ne!(first, second);
        assert_eq!(first.len(), 36);
        assert_eq!(&first[14..15], "4");
    }

    #[test]
    fn stop_actions_resolve_lock_restart_and_cancel_interleavings() {
        assert_eq!(
            after_stop(PendingAction::LockPause, false),
            AfterStop::StayPaused
        );
        assert_eq!(
            after_stop(PendingAction::Resume, false),
            AfterStop::StayPaused
        );
        assert_eq!(
            after_stop(PendingAction::Restart, false),
            AfterStop::Restart
        );
        assert_eq!(after_stop(PendingAction::Discard, true), AfterStop::Discard);
        assert_eq!(after_stop(PendingAction::Stop, true), AfterStop::Finalize);
    }

    #[test]
    fn screenshot_completion_only_resumes_current_unlocked_session() {
        assert!(screenshot_may_resume(7, 7, true, HudPhase::Paused, true));
        assert!(!screenshot_may_resume(7, 8, true, HudPhase::Paused, true));
        assert!(!screenshot_may_resume(7, 7, false, HudPhase::Paused, true));
        assert!(!screenshot_may_resume(7, 7, true, HudPhase::Paused, false));
        assert!(!screenshot_may_resume(
            7,
            7,
            true,
            HudPhase::Discarding,
            true
        ));
    }

    #[test]
    fn zero_countdown_starting_phase_can_begin_exactly_once() {
        assert!(may_begin_segment(HudPhase::Starting, false));
        assert!(may_begin_segment(HudPhase::Countdown(1), false));
        assert!(may_begin_segment(HudPhase::Paused, false));
        assert!(!may_begin_segment(HudPhase::Starting, true));
        assert!(!may_begin_segment(HudPhase::Pausing, false));
        assert!(!may_begin_segment(HudPhase::Finalizing, false));
        assert!(!may_begin_segment(HudPhase::Discarding, false));
    }
}
