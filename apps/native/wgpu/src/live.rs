use std::{
    borrow::Cow,
    collections::HashMap,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::{Duration, Instant},
};

use captures_app::{
    Artifact, Request, Response,
    capture_flow::CaptureFlow,
    region::RegionSession,
    selection::Rect as SelectionRect,
    shortcuts::CaptureShortcut,
    window::{Target as WindowCaptureTarget, WindowSession},
};
use captures_capture::{DisplayDescriptor, LogicalRect};
use captures_recording::{
    AudioOptions, CaptureRect, GifOptions, RecordingKind, RecordingOptions,
    RecordingSessionSnapshot, RecordingState, RecordingTarget,
};
use captures_recording_platform::{RecordingCapabilities, recording_controls_are_excluded};
use captures_settings::AppSettings;
use eframe::egui::{self, RichText};

use crate::{
    capture_controls::{self, CaptureControls},
    selector,
    selector::Selector,
    window_selector::{self, SelectionTarget, WindowSelector},
};
use crate::{recording, recording_hud, tokens::Tokens};

enum Job {
    LoadHistory {
        root: PathBuf,
    },
    Execute {
        request: Request,
        preview: Option<PreviewGuard>,
    },
    PrepareRegion {
        display_id: String,
        generation: u64,
        freeze: bool,
        include_cursor: bool,
    },
    CaptureRegion {
        root: PathBuf,
        generation: u64,
        session: Box<RegionSession>,
        rect: LogicalRect,
        after_countdown: bool,
    },
    PrepareWindow {
        display_id: String,
        generation: u64,
        freeze: bool,
        include_cursor: bool,
    },
    CaptureWindow {
        root: PathBuf,
        generation: u64,
        session: Arc<WindowSession>,
        target: WindowCaptureTarget,
        after_countdown: bool,
    },
    DecodeHistory {
        generation: u64,
        path: PathBuf,
    },
    DecodePreview {
        generation: u64,
        artifact_id: String,
        path: PathBuf,
    },
    Copy {
        path: PathBuf,
        preview: Option<PreviewGuard>,
    },
    Shutdown,
}

enum Reply {
    HistoryLoaded(Result<Vec<Artifact>, String>),
    Executed {
        preview: Option<PreviewGuard>,
        result: Result<Box<Response>, String>,
    },
    HistoryCleared(Result<Box<Response>, String>),
    RegionPrepared {
        generation: u64,
        result: Result<Box<RegionSession>, String>,
    },
    RegionCaptured {
        generation: u64,
        result: Result<Box<Artifact>, String>,
    },
    WindowPrepared {
        generation: u64,
        result: Result<Arc<WindowSession>, String>,
    },
    WindowCaptured {
        generation: u64,
        result: Result<Box<Artifact>, String>,
    },
    Copied {
        preview: Option<PreviewGuard>,
        result: Result<(), String>,
    },
    HistoryDecoded {
        generation: u64,
        path: PathBuf,
        result: Result<Decoded, String>,
    },
    PreviewDecoded {
        generation: u64,
        artifact_id: String,
        result: Result<Decoded, String>,
    },
}

struct Decoded {
    image: egui::ColorImage,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PreviewGuard {
    artifact_id: String,
    generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum CapturePhase {
    DisplayCountdown,
    DisplayCapturing,
    RegionPreparing,
    RegionSelecting,
    RegionCountdown {
        rect: SelectionRect,
        after_countdown: bool,
    },
    RegionCapturing,
    WindowPreparing,
    WindowSelecting,
    WindowCountdown {
        target: SelectionTarget,
        after_countdown: bool,
    },
    WindowCapturing,
    ControlsPreparing,
    ControlsSelecting,
    ControlsCountdown {
        target: capture_controls::Target,
        after_countdown: bool,
    },
    ControlsCapturing,
    RecordingPreparing {
        target: capture_controls::Target,
    },
    RecordingCountdown,
    RecordingStarting,
    Recording,
    RecordingPausing,
    RecordingPaused,
    RecordingMuting {
        paused: bool,
    },
    RecordingRestarting,
    RecordingFinalizing,
    RecordingDiscarding,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum RecordingScreenshotPhase {
    WaitingForHud,
    Preparing,
    Selecting,
    Countdown {
        rect: SelectionRect,
        after_countdown: bool,
    },
    Capturing,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaptureRequest {
    NewCapture,
    Recording(capture_controls::TargetMode),
    Display,
    Region,
    Window,
}

enum SelectorMessage {
    ConfirmRegion {
        generation: u64,
        rect: SelectionRect,
    },
    ConfirmWindow {
        generation: u64,
        target: SelectionTarget,
    },
    ConfirmControls {
        generation: u64,
        target: capture_controls::Target,
    },
    StartRecording {
        generation: u64,
        target: capture_controls::Target,
    },
    PauseRecording {
        generation: u64,
    },
    ResumeRecording {
        generation: u64,
    },
    SetMicrophoneMuted {
        generation: u64,
        muted: bool,
    },
    RestartRecording {
        generation: u64,
    },
    StartRecordingScreenshot {
        generation: u64,
    },
    ConfirmRestartRecording {
        generation: u64,
    },
    CancelRestartRecording {
        generation: u64,
    },
    StopRecording {
        generation: u64,
    },
    DiscardRecording {
        generation: u64,
    },
    HideRecordingControls {
        generation: u64,
    },
    SwitchControlsDisplay {
        generation: u64,
        display_id: String,
    },
    Cancel {
        generation: u64,
        kind: SelectorKind,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum PreviewMessage {
    Copy {
        artifact_id: String,
        generation: u64,
    },
    Save {
        artifact_id: String,
        generation: u64,
        directory: PathBuf,
        format: captures_settings::ScreenshotFormat,
    },
    OpenHistory {
        artifact_id: String,
        generation: u64,
    },
    Dismiss {
        artifact_id: String,
        generation: u64,
    },
    ToggleCollapsed,
    ClearAll {
        artifact_ids: Vec<String>,
    },
}

#[derive(Clone, Copy)]
struct CaptureTarget {
    monitor: usize,
    position: egui::Pos2,
    size: egui::Vec2,
    preview_bounds: Option<captures_app::preview::ThumbnailMonitorBounds>,
}

struct PreviewCard {
    generation: u64,
    artifact_id: String,
    image_path: PathBuf,
    width: u32,
    height: u32,
    texture: Option<egui::TextureHandle>,
    busy: Option<crate::mini_preview::Busy>,
    message: Option<String>,
}

#[derive(Clone)]
struct PreviewRenderCard {
    artifact_id: String,
    generation: u64,
    width: u32,
    height: u32,
    texture: egui::TextureHandle,
    busy: Option<crate::mini_preview::Busy>,
    message: Option<String>,
    layout: captures_app::preview::PreviewCardLayout,
}

struct MiniPreviews {
    visibility: captures_app::preview::ThumbnailVisibility,
    stack: captures_app::preview::PreviewStack,
    next_generation: u64,
    cards: HashMap<String, PreviewCard>,
    stack_target: Option<CaptureTarget>,
    waiting_artifact: Option<String>,
    capture_generation: Option<u64>,
    capture_target: Option<CaptureTarget>,
    show: bool,
    include_in_captures: bool,
    placement: captures_settings::MiniPreviewPlacement,
    omission_frame: Option<u64>,
}

impl Default for MiniPreviews {
    fn default() -> Self {
        Self {
            visibility: captures_app::preview::ThumbnailVisibility::default(),
            stack: captures_app::preview::PreviewStack::default(),
            next_generation: 0,
            cards: HashMap::new(),
            stack_target: None,
            waiting_artifact: None,
            capture_generation: None,
            capture_target: None,
            show: true,
            include_in_captures: false,
            placement: captures_settings::MiniPreviewPlacement::default(),
            omission_frame: None,
        }
    }
}

impl MiniPreviews {
    fn begin_capture(
        &mut self,
        settings: &AppSettings,
        target: Option<CaptureTarget>,
        frame: u64,
    ) -> Result<(), String> {
        let was_visible = self.is_visible();
        let generation = self
            .visibility
            .begin_capture()
            .ok_or("A previous mini-preview capture is still preparing.")?;
        self.capture_generation = Some(generation);
        self.capture_target = target;
        self.show = settings.show_mini_previews;
        self.include_in_captures = settings.include_mini_previews_in_captures;
        self.placement = settings.mini_preview_placement;
        self.omission_frame =
            (was_visible && self.show && !self.include_in_captures).then_some(frame);
        Ok(())
    }

    fn restore_capture(&mut self) {
        if let Some(generation) = self.capture_generation.take() {
            self.visibility.restore_capture(generation);
        }
        self.capture_target = None;
        self.omission_frame = None;
    }

    fn start_artifact(
        &mut self,
        artifact: &Artifact,
    ) -> Result<Option<(PreviewGuard, PathBuf)>, String> {
        let Some(capture_generation) = self.capture_generation.take() else {
            return Err("Mini-preview capture state was lost before persistence.".into());
        };
        self.omission_frame = None;
        let target = self.capture_target.take();
        if self.show && target.is_none() {
            self.visibility.restore_capture(capture_generation);
            return Err("Mini-preview monitor state was lost before persistence.".into());
        }
        if self.show && target.is_some_and(|target| target.preview_bounds.is_none()) {
            self.visibility.restore_capture(capture_generation);
            return Err("Mini-preview positioning is unavailable for this display.".into());
        }
        let artifact_id = artifact.entry.id.clone();
        if !self
            .visibility
            .wait_for_artifact(capture_generation, artifact_id.clone())
        {
            return Err("Mini-preview capture generation changed before persistence.".into());
        }
        if !self.stack.insert(artifact_id.clone()) {
            self.visibility.restore_capture(capture_generation);
            return Err("Mini-preview artifact was already present in the stack.".into());
        }
        self.next_generation = self.next_generation.wrapping_add(1);
        let generation = self.next_generation;
        self.cards.insert(
            artifact_id.clone(),
            PreviewCard {
                generation,
                artifact_id: artifact_id.clone(),
                image_path: artifact.image_path.clone(),
                width: artifact.entry.width,
                height: artifact.entry.height,
                texture: None,
                busy: None,
                message: None,
            },
        );
        if target.is_some() {
            self.stack_target = target;
        }
        self.waiting_artifact = Some(artifact_id.clone());
        Ok(Some((
            PreviewGuard {
                artifact_id,
                generation,
            },
            artifact.preview_path.clone(),
        )))
    }

    fn accepts(&self, artifact_id: &str, generation: u64) -> bool {
        self.cards
            .get(artifact_id)
            .is_some_and(|card| card.generation == generation)
    }

    fn dismiss(&mut self, artifact_id: &str, generation: u64) -> bool {
        if !self.accepts(artifact_id, generation) {
            return false;
        }
        self.remove(artifact_id)
    }

    fn remove(&mut self, artifact_id: &str) -> bool {
        if !self.stack.remove(artifact_id) {
            return false;
        }
        self.cards.remove(artifact_id);
        if self.waiting_artifact.as_deref() == Some(artifact_id) {
            self.waiting_artifact = None;
            self.visibility.stop_waiting_for_artifact();
        }
        if self.stack.ids().is_empty() {
            self.stack_target = None;
        }
        true
    }

    fn clear(&mut self, artifact_ids: &[String]) -> usize {
        let removed = self.stack.remove_all(artifact_ids);
        for artifact_id in artifact_ids {
            self.cards.remove(artifact_id);
        }
        if self
            .waiting_artifact
            .as_ref()
            .is_some_and(|waiting| artifact_ids.contains(waiting))
        {
            self.waiting_artifact = None;
            self.visibility.stop_waiting_for_artifact();
        }
        if self.stack.ids().is_empty() {
            self.stack_target = None;
        }
        removed
    }

    fn mark_ready(&mut self, artifact_id: &str) {
        if self.waiting_artifact.as_deref() == Some(artifact_id) {
            self.visibility.mark_artifact_ready(artifact_id);
            self.waiting_artifact = None;
        }
    }

    fn is_visible(&self) -> bool {
        captures_app::preview::stack_should_be_visible(
            self.cards
                .values()
                .filter(|card| card.texture.is_some())
                .count(),
            self.visibility.is_suppressed(),
            self.show,
            self.include_in_captures,
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SelectorKind {
    Region,
    Window,
    Controls,
}

fn request_hidden_root_paint(ctx: &egui::Context) {
    ctx.send_viewport_cmd_to(
        egui::ViewportId::ROOT,
        egui::ViewportCommand::RequestPaintWhileHidden,
    );
}

/// The nonvisual state machine is intentionally independent of egui so stale
/// worker replies can be tested without constructing a renderer.
#[derive(Default)]
struct Selection {
    generation: u64,
    id: Option<String>,
}

impl Selection {
    fn clear(&mut self) {
        self.generation += 1;
        self.id = None;
    }

    fn begin(&mut self, id: String) -> u64 {
        self.generation += 1;
        self.id = Some(id);
        self.generation
    }

    fn accepts(&self, generation: u64) -> bool {
        generation == self.generation
    }
}

pub struct Live {
    root: PathBuf,
    tx: Sender<Job>,
    rx: Receiver<Reply>,
    worker: Option<thread::JoinHandle<()>>,
    displays: Vec<DisplayDescriptor>,
    display_id: Option<String>,
    artifacts: Vec<Artifact>,
    selection: Selection,
    texture: Option<egui::TextureHandle>,
    preview_loading: bool,
    decoded_path: Option<PathBuf>,
    status: String,
    error: Option<String>,
    pending: usize,
    capture_waiting_for_hide: bool,
    hide_started: Option<Instant>,
    hidden_since: Option<Instant>,
    capture_in_flight: bool,
    auto_copy_on_capture: bool,
    include_cursor: bool,
    flow: Option<CaptureFlow>,
    capture_phase: Option<CapturePhase>,
    countdown_target: Option<CaptureTarget>,
    region_session: Option<Box<RegionSession>>,
    region_texture: Option<egui::TextureHandle>,
    region_selector: Arc<Mutex<Selector>>,
    selector_tx: Sender<SelectorMessage>,
    selector_rx: Receiver<SelectorMessage>,
    preview_tx: Sender<PreviewMessage>,
    preview_rx: Receiver<PreviewMessage>,
    previews: MiniPreviews,
    root_hide_deferred: bool,
    open_history_requested: bool,
    region_freeze: bool,
    region_auto_start: bool,
    region_countdown_seconds: u8,
    window_session: Option<Arc<WindowSession>>,
    window_texture: Option<egui::TextureHandle>,
    window_selector: Arc<Mutex<WindowSelector>>,
    window_freeze: bool,
    window_auto_start: bool,
    window_countdown_seconds: u8,
    controls: Arc<Mutex<CaptureControls>>,
    selector_scope_generation: Arc<AtomicU64>,
    controls_freeze: bool,
    controls_auto_start: bool,
    controls_countdown_seconds: u8,
    recording_worker: recording::Worker,
    recording_toolchain_ready: bool,
    recording_toolchain_error: Option<String>,
    include_recording_controls: bool,
    recording_snapshot: Option<RecordingSessionSnapshot>,
    recording_segment_started: Option<Instant>,
    recording_snapshot_poll_pending: bool,
    recording_last_snapshot_poll: Instant,
    recording_has_started: bool,
    recording_restart_confirmation: bool,
    recording_screenshot_flow: Option<CaptureFlow>,
    recording_screenshot_phase: Option<RecordingScreenshotPhase>,
    recording_screenshot_hide_started: Option<Instant>,
    recording_screenshot_session: Option<Box<RegionSession>>,
    recording_screenshot_texture: Option<egui::TextureHandle>,
    recording_screenshot_settings: Option<AppSettings>,
    recording_controls_hidden: Option<u64>,
    recording_hidden_notice_until: Option<Instant>,
    recording_restore_available: bool,
    history_refresh_status: Option<String>,
    can_hide: Option<bool>,
    confirm_delete: Option<String>,
    confirm_clear_history: bool,
    requested_capture: Option<CaptureRequest>,
    restore_root_visible: bool,
}

impl Live {
    pub fn new(ctx: egui::Context, root: Option<PathBuf>) -> Self {
        let root = root.unwrap_or_else(captures_app::default_history_root);
        let (tx, jobs) = mpsc::channel();
        let (out, rx) = mpsc::channel();
        let (selector_tx, selector_rx) = mpsc::channel();
        let (preview_tx, preview_rx) = mpsc::channel();
        let capture_ctx = ctx.clone();
        let worker = thread::spawn(move || {
            // Retain ownership on X11. Disk decode and clipboard encoding never
            // block the UI, and the full uncompressed image is not retained by it.
            let mut clipboard = None;
            while let Ok(job) = jobs.recv() {
                let reply = match job {
                    Job::Shutdown => break,
                    Job::LoadHistory { root } => Reply::HistoryLoaded(load_history(&root)),
                    Job::Execute { request, preview } => {
                        let clearing = matches!(request, Request::ClearHistory { .. });
                        let result = captures_app::execute(request)
                            .map(Box::new)
                            .map_err(|error| error.to_string());
                        if clearing {
                            Reply::HistoryCleared(result)
                        } else {
                            Reply::Executed { preview, result }
                        }
                    }
                    Job::PrepareRegion {
                        display_id,
                        generation,
                        freeze,
                        include_cursor,
                    } => Reply::RegionPrepared {
                        generation,
                        result: RegionSession::prepare(
                            &display_id,
                            generation,
                            freeze,
                            include_cursor,
                        )
                        .map(Box::new)
                        .map_err(|error| error.to_string()),
                    },
                    Job::CaptureRegion {
                        root,
                        generation,
                        session,
                        rect,
                        after_countdown,
                    } => Reply::RegionCaptured {
                        generation,
                        result: session
                            .capture(&root, rect, after_countdown)
                            .map(Box::new)
                            .map_err(|error| error.to_string()),
                    },
                    Job::PrepareWindow {
                        display_id,
                        generation,
                        freeze,
                        include_cursor,
                    } => Reply::WindowPrepared {
                        generation,
                        result: WindowSession::prepare(
                            &display_id,
                            generation,
                            freeze,
                            include_cursor,
                            0.,
                        )
                        .map(Arc::new)
                        .map_err(|error| error.to_string()),
                    },
                    Job::CaptureWindow {
                        root,
                        generation,
                        session,
                        target,
                        after_countdown,
                    } => Reply::WindowCaptured {
                        generation,
                        result: session
                            .capture(&root, &target, after_countdown)
                            .map(Box::new)
                            .map_err(|error| error.to_string()),
                    },
                    Job::DecodeHistory { generation, path } => Reply::HistoryDecoded {
                        generation,
                        result: decode(&path),
                        path,
                    },
                    Job::DecodePreview {
                        generation,
                        artifact_id,
                        path,
                    } => Reply::PreviewDecoded {
                        generation,
                        artifact_id,
                        result: decode(&path),
                    },
                    Job::Copy { path, preview } => Reply::Copied {
                        preview,
                        result: copy_image(&path, &mut clipboard),
                    },
                };
                if out.send(reply).is_err() {
                    break;
                }
                capture_ctx.request_repaint();
            }
        });
        let mut live = Self {
            root,
            tx,
            rx,
            worker: Some(worker),
            displays: vec![],
            display_id: None,
            artifacts: vec![],
            selection: Selection::default(),
            texture: None,
            preview_loading: false,
            decoded_path: None,
            status: "Loading capture workspace…".into(),
            error: None,
            pending: 0,
            capture_waiting_for_hide: false,
            hide_started: None,
            hidden_since: None,
            capture_in_flight: false,
            auto_copy_on_capture: false,
            include_cursor: false,
            flow: None,
            capture_phase: None,
            countdown_target: None,
            region_session: None,
            region_texture: None,
            region_selector: Arc::new(Mutex::new(Selector::default())),
            selector_tx,
            selector_rx,
            preview_tx,
            preview_rx,
            previews: MiniPreviews::default(),
            root_hide_deferred: false,
            open_history_requested: false,
            region_freeze: false,
            region_auto_start: false,
            region_countdown_seconds: 0,
            window_session: None,
            window_texture: None,
            window_selector: Arc::new(Mutex::new(WindowSelector::default())),
            window_freeze: false,
            window_auto_start: false,
            window_countdown_seconds: 0,
            controls: Arc::new(Mutex::new(CaptureControls::default())),
            selector_scope_generation: Arc::new(AtomicU64::new(0)),
            controls_freeze: false,
            controls_auto_start: false,
            controls_countdown_seconds: 0,
            recording_worker: recording::Worker::new(ctx.clone()),
            recording_toolchain_ready: false,
            recording_toolchain_error: None,
            include_recording_controls: false,
            recording_snapshot: None,
            recording_segment_started: None,
            recording_snapshot_poll_pending: false,
            recording_last_snapshot_poll: Instant::now(),
            recording_has_started: false,
            recording_restart_confirmation: false,
            recording_screenshot_flow: None,
            recording_screenshot_phase: None,
            recording_screenshot_hide_started: None,
            recording_screenshot_session: None,
            recording_screenshot_texture: None,
            recording_screenshot_settings: None,
            recording_controls_hidden: None,
            recording_hidden_notice_until: None,
            recording_restore_available: false,
            history_refresh_status: None,
            can_hide: None,
            confirm_delete: None,
            confirm_clear_history: false,
            requested_capture: None,
            restore_root_visible: true,
        };
        live.load_history();
        live.send(Request::Displays);
        live
    }

    pub fn is_capturing(&self) -> bool {
        self.flow.is_some() || self.capture_in_flight
    }

    pub fn can_launch_capture(&self) -> bool {
        self.pending == 0 && !self.is_capturing() && self.requested_capture.is_none()
    }

    pub fn recording_controls_hidden(&self) -> bool {
        recording_controls_hidden(
            self.recording_controls_hidden,
            self.flow.as_ref().map(CaptureFlow::generation),
        )
    }

    pub fn set_recording_restore_available(&mut self, available: bool, ctx: &egui::Context) {
        self.recording_restore_available = available;
        if !available {
            self.show_recording_controls(ctx);
        }
    }

    pub fn show_recording_controls(&mut self, ctx: &egui::Context) -> bool {
        if !self.recording_controls_hidden() {
            self.recording_controls_hidden = None;
            self.recording_hidden_notice_until = None;
            return false;
        }
        self.recording_controls_hidden = None;
        self.recording_hidden_notice_until = None;
        ctx.request_repaint_of(egui::ViewportId::from_hash_of("recording-controls"));
        request_hidden_root_paint(ctx);
        true
    }

    pub fn selector_generation(&self) -> Option<u64> {
        let generation = self.selector_scope_generation.load(Ordering::Acquire);
        active_selector_generation(
            generation,
            self.capture_phase,
            self.flow.as_ref().map(CaptureFlow::generation),
            self.flow.as_ref().is_some_and(CaptureFlow::is_current),
        )
    }

    pub fn apply_selector_shortcut(
        &mut self,
        shortcut: CaptureShortcut,
        ctx: &egui::Context,
    ) -> bool {
        if self.selector_generation().is_none() {
            return false;
        }
        if shortcut != CaptureShortcut::NewCapture {
            self.controls
                .lock()
                .unwrap()
                .apply_target_shortcut(shortcut);
            ctx.request_repaint_of(egui::ViewportId::from_hash_of("capture-controls"));
        }
        true
    }

    fn can_start_capture(&self) -> bool {
        self.can_launch_capture() && self.display_id.is_some() && self.can_hide == Some(true)
    }

    pub fn take_open_history_requested(&mut self) -> bool {
        std::mem::take(&mut self.open_history_requested)
    }

    pub fn request_capture(&mut self, request: CaptureRequest) {
        if request == CaptureRequest::NewCapture && self.recording_controls_hidden() {
            // The shipping New Capture action restores a hidden active HUD; it
            // never starts a second capture or replaces the accepted take.
            self.requested_capture = None;
            return;
        }
        if self.pending > 0 || self.is_capturing() || self.requested_capture.is_some() {
            self.error = Some("Another capture or history action is still in progress.".into());
        } else {
            self.requested_capture = Some(request);
        }
    }

    pub fn launch_requested_capture(
        &mut self,
        ctx: &egui::Context,
        frame: &eframe::Frame,
        settings: Result<AppSettings, String>,
    ) {
        let Some(request) = self.requested_capture.take() else {
            return;
        };
        if !self.can_start_capture() {
            self.error = Some(
                "Capture is unavailable until the current action finishes and a display is ready."
                    .into(),
            );
            return;
        }
        let settings = match settings {
            Ok(settings) => settings,
            Err(error) => {
                self.error = Some(error);
                return;
            }
        };
        let target = capture_target(frame, &self.displays, self.display_id.as_deref());
        self.countdown_target = target;
        let countdown = matches!(request, CaptureRequest::Display)
            .then_some(settings.screenshot_countdown_seconds)
            .unwrap_or(0);
        if target.is_none()
            && (!matches!(request, CaptureRequest::Display)
                || settings.screenshot_countdown_seconds > 0)
        {
            self.error = Some(match request {
                CaptureRequest::Display => {
                    "The selected display is no longer available for countdown.".into()
                }
                CaptureRequest::NewCapture | CaptureRequest::Recording(_) => {
                    "The selected display is no longer available for capture controls.".into()
                }
                CaptureRequest::Region => {
                    "The selected display is no longer available for region selection.".into()
                }
                CaptureRequest::Window => {
                    "The selected display is no longer available for window selection.".into()
                }
            });
            return;
        }
        let flow = match CaptureFlow::begin(countdown) {
            Ok(flow) => flow,
            Err(error) => {
                self.error = Some(format!("Could not arm capture Escape: {error}"));
                return;
            }
        };
        if let Err(error) =
            self.previews
                .begin_capture(&settings, target, ctx.cumulative_frame_nr())
        {
            flow.cancel();
            self.error = Some(error);
            return;
        }
        self.restore_root_visible = frame
            .winit_window()
            .and_then(|window| window.is_visible())
            .unwrap_or(true);
        self.flow = Some(flow);
        self.auto_copy_on_capture = settings.auto_copy_to_clipboard;
        self.include_cursor = settings.show_cursor_in_screenshots;
        self.recording_screenshot_settings = matches!(
            request,
            CaptureRequest::NewCapture | CaptureRequest::Recording(_)
        )
        .then(|| settings.clone());
        match request {
            CaptureRequest::NewCapture | CaptureRequest::Recording(_) => {
                self.selector_scope_generation.store(0, Ordering::Release);
                self.capture_phase = Some(CapturePhase::ControlsPreparing);
                self.controls_freeze = settings.freeze_screen;
                self.controls_auto_start = settings.auto_start_on_selection;
                self.controls_countdown_seconds = settings.screenshot_countdown_seconds;
                self.include_recording_controls = settings.include_recording_controls_in_captures;
                let mut controls = self.controls.lock().unwrap();
                controls.reset();
                controls.configure_recording(
                    &settings.recording,
                    RecordingCapabilities::current(settings.include_recording_controls_in_captures),
                );
                if let CaptureRequest::Recording(target) = request {
                    controls.select_recording_target(target);
                }
                drop(controls);
                self.recording_toolchain_ready = false;
                self.recording_toolchain_error = None;
                self.recording_has_started = false;
                self.recording_worker
                    .send(recording::Command::VerifyToolchain {
                        generation: self
                            .flow
                            .as_ref()
                            .expect("new capture owns flow")
                            .generation(),
                    });
                self.recording_worker
                    .send(recording::Command::ListMicrophones {
                        generation: self
                            .flow
                            .as_ref()
                            .expect("new capture owns flow")
                            .generation(),
                    });
                self.status = "Preparing capture controls… Press Escape to cancel.".into();
                self.hide_for_capture(ctx);
            }
            CaptureRequest::Display => {
                self.capture_phase = Some(CapturePhase::DisplayCountdown);
                self.status = "Preparing screenshot… Press Escape to cancel.".into();
                request_hidden_root_paint(ctx);
                ctx.request_repaint();
            }
            CaptureRequest::Region => {
                self.capture_phase = Some(CapturePhase::RegionPreparing);
                self.region_freeze = settings.freeze_screen;
                self.region_auto_start = settings.auto_start_on_selection;
                self.region_countdown_seconds = settings.screenshot_countdown_seconds;
                self.status = "Preparing region selector… Press Escape to cancel.".into();
                self.hide_for_capture(ctx);
            }
            CaptureRequest::Window => {
                self.capture_phase = Some(CapturePhase::WindowPreparing);
                self.window_freeze = settings.freeze_screen;
                self.window_auto_start = settings.auto_start_on_selection;
                self.window_countdown_seconds = settings.screenshot_countdown_seconds;
                self.status = "Preparing window selector… Press Escape to cancel.".into();
                self.hide_for_capture(ctx);
            }
        }
    }

    pub fn flush(&mut self) {
        // Cancel preparation/countdown before draining work. CaptureFlow::cancel
        // leaves a capture that already crossed its persistence commit point alone.
        self.selector_scope_generation.store(0, Ordering::Release);
        if let Some(flow) = &self.recording_screenshot_flow {
            flow.cancel();
        }
        let drain_recording_worker = is_recording_phase(self.capture_phase);
        if let Some(flow) = &self.flow {
            let generation = flow.generation();
            if self.recording_has_started
                && matches!(
                    self.capture_phase,
                    Some(
                        CapturePhase::RecordingStarting
                            | CapturePhase::Recording
                            | CapturePhase::RecordingPausing
                            | CapturePhase::RecordingPaused
                            | CapturePhase::RecordingMuting { .. }
                    )
                )
            {
                // A take that crossed the Escape handoff is user media. Finish
                // it before process teardown instead of deleting accepted segments.
                self.recording_worker.send(recording::Command::Finish {
                    generation,
                    history_root: self.root.clone(),
                });
            } else if is_recording_phase(self.capture_phase)
                && !matches!(
                    self.capture_phase,
                    Some(CapturePhase::RecordingFinalizing | CapturePhase::RecordingDiscarding)
                )
            {
                flow.cancel();
                self.recording_worker
                    .send(recording::Command::Discard { generation });
            } else {
                flow.cancel();
            }
        }
        // Finish accepted capture/export/delete operations before process teardown.
        let _ = self.tx.send(Job::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        if drain_recording_worker {
            // Recording phases own draft media or an accepted take, so process
            // the queued Finish/Discard before allowing process teardown.
            self.recording_worker.shutdown();
        } else {
            // Selector startup may still be inside blocking ALSA device discovery.
            // It owns no media, so do not make tray Quit wait for that unrelated call.
            self.recording_worker.shutdown_detached();
        }
        self.flow = None;
        self.capture_phase = None;
        self.region_session = None;
        self.recording_screenshot_flow = None;
        self.recording_screenshot_session = None;
        self.window_session = None;
    }

    fn send(&mut self, request: Request) {
        self.pending += 1;
        self.error = None;
        let _ = self.tx.send(Job::Execute {
            request,
            preview: None,
        });
    }

    fn load_history(&mut self) {
        self.pending += 1;
        let _ = self.tx.send(Job::LoadHistory {
            root: self.root.clone(),
        });
    }

    fn send_preview(&mut self, request: Request, preview: PreviewGuard) {
        self.pending += 1;
        let _ = self.tx.send(Job::Execute {
            request,
            preview: Some(preview),
        });
    }

    fn begin_root_hide(&mut self, ctx: &egui::Context) {
        self.capture_waiting_for_hide = true;
        self.hide_started = Some(Instant::now());
        self.hidden_since = None;
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        ctx.request_repaint();
    }

    fn hide_for_capture(&mut self, ctx: &egui::Context) {
        if self
            .previews
            .omission_frame
            .is_some_and(|frame| frame >= ctx.cumulative_frame_nr())
        {
            // A deferred viewport remains alive for the frame in which it was
            // last declared. Omit it for one completed pass before hiding and
            // settling the root, rather than trusting asynchronous Visible(false).
            self.root_hide_deferred = true;
            ctx.request_repaint();
        } else {
            self.begin_root_hide(ctx);
        }
    }

    fn load_selected(&mut self) {
        let Some(item) = self
            .artifacts
            .iter()
            .find(|item| self.selection.id.as_deref() == Some(item.entry.id.as_str()))
        else {
            self.texture = None;
            self.decoded_path = None;
            self.preview_loading = false;
            return;
        };
        let generation = self.selection.generation;
        let _ = self.tx.send(Job::DecodeHistory {
            generation,
            path: item.image_path.clone(),
        });
    }

    fn select(&mut self, id: String) {
        self.selection.begin(id);
        self.texture = None;
        self.decoded_path = None;
        self.preview_loading = true;
        self.load_selected();
    }

    pub fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.can_hide = frame
            .winit_window()
            .map(|window| window.is_visible().is_some());
        while let Ok(message) = self.selector_rx.try_recv() {
            match message {
                SelectorMessage::Cancel {
                    generation,
                    kind: SelectorKind::Region,
                } if self
                    .recording_screenshot_flow
                    .as_ref()
                    .is_some_and(|flow| flow.generation() == generation)
                    && self.recording_screenshot_phase
                        == Some(RecordingScreenshotPhase::Selecting) =>
                {
                    if let Some(flow) = &self.recording_screenshot_flow {
                        flow.cancel();
                    }
                }
                SelectorMessage::ConfirmRegion { generation, rect }
                    if self.recording_screenshot_flow.as_ref().is_some_and(|flow| {
                        flow.generation() == generation && flow.is_current()
                    }) && self.recording_screenshot_phase
                        == Some(RecordingScreenshotPhase::Selecting) =>
                {
                    let seconds = self
                        .recording_screenshot_settings
                        .as_ref()
                        .map(|settings| settings.screenshot_countdown_seconds)
                        .unwrap_or(0);
                    let Some(flow) = &mut self.recording_screenshot_flow else {
                        continue;
                    };
                    match flow.start_countdown(seconds) {
                        Ok(()) => {
                            self.recording_screenshot_phase =
                                Some(RecordingScreenshotPhase::Countdown {
                                    rect,
                                    after_countdown: seconds > 0,
                                });
                            self.status =
                                "Screenshot region confirmed. Press Escape to cancel.".into();
                            request_hidden_root_paint(ctx);
                        }
                        Err(error) => {
                            self.error = Some(error);
                            self.finish_recording_screenshot(ctx, false);
                        }
                    }
                }
                SelectorMessage::Cancel { generation, kind }
                    if accepts_selector_action(
                        self.flow.as_ref().map(CaptureFlow::generation),
                        generation,
                        self.flow.as_ref().is_some_and(CaptureFlow::is_current),
                        self.capture_phase,
                        kind,
                    ) =>
                {
                    let Some(flow) = &self.flow else { continue };
                    flow.cancel();
                }
                SelectorMessage::ConfirmRegion { generation, rect }
                    if accepts_selector_action(
                        self.flow.as_ref().map(CaptureFlow::generation),
                        generation,
                        self.flow.as_ref().is_some_and(CaptureFlow::is_current),
                        self.capture_phase,
                        SelectorKind::Region,
                    ) =>
                {
                    let Some(flow) = &mut self.flow else {
                        continue;
                    };
                    match flow.start_countdown(self.region_countdown_seconds) {
                        Ok(()) => {
                            self.capture_phase = Some(CapturePhase::RegionCountdown {
                                rect,
                                after_countdown: self.region_countdown_seconds > 0,
                            });
                            self.status = "Region confirmed. Press Escape to cancel.".into();
                            request_hidden_root_paint(ctx);
                            ctx.request_repaint();
                        }
                        Err(error) => {
                            self.error = Some(error);
                            self.finish_capture(ctx, false);
                        }
                    }
                }
                SelectorMessage::ConfirmWindow { generation, target }
                    if accepts_selector_action(
                        self.flow.as_ref().map(CaptureFlow::generation),
                        generation,
                        self.flow.as_ref().is_some_and(CaptureFlow::is_current),
                        self.capture_phase,
                        SelectorKind::Window,
                    ) =>
                {
                    let Some(flow) = &mut self.flow else {
                        continue;
                    };
                    match flow.start_countdown(self.window_countdown_seconds) {
                        Ok(()) => {
                            self.capture_phase = Some(CapturePhase::WindowCountdown {
                                target,
                                after_countdown: self.window_countdown_seconds > 0,
                            });
                            self.status = "Window confirmed. Press Escape to cancel.".into();
                            request_hidden_root_paint(ctx);
                            ctx.request_repaint();
                        }
                        Err(error) => {
                            self.error = Some(error);
                            self.finish_capture(ctx, false);
                        }
                    }
                }
                SelectorMessage::ConfirmControls { generation, target }
                    if accepts_selector_action(
                        self.flow.as_ref().map(CaptureFlow::generation),
                        generation,
                        self.flow.as_ref().is_some_and(CaptureFlow::is_current),
                        self.capture_phase,
                        SelectorKind::Controls,
                    ) =>
                {
                    let Some(flow) = &mut self.flow else {
                        continue;
                    };
                    match flow.start_countdown(self.controls_countdown_seconds) {
                        Ok(()) => {
                            self.capture_phase = Some(CapturePhase::ControlsCountdown {
                                target,
                                after_countdown: self.controls_countdown_seconds > 0,
                            });
                            self.status = "Target confirmed. Press Escape to cancel.".into();
                            request_hidden_root_paint(ctx);
                            ctx.request_repaint();
                        }
                        Err(error) => {
                            self.error = Some(error);
                            self.finish_capture(ctx, false);
                        }
                    }
                }
                SelectorMessage::StartRecording { generation, target }
                    if accepts_selector_action(
                        self.flow.as_ref().map(CaptureFlow::generation),
                        generation,
                        self.flow.as_ref().is_some_and(CaptureFlow::is_current),
                        self.capture_phase,
                        SelectorKind::Controls,
                    ) =>
                {
                    if !self.recording_toolchain_ready {
                        self.error =
                            Some(self.recording_toolchain_error.clone().unwrap_or_else(|| {
                                "FFmpeg and ffprobe verification is still in progress.".into()
                            }));
                        continue;
                    }
                    let Some(display) = self
                        .displays
                        .iter()
                        .find(|display| Some(&display.id) == self.display_id.as_ref())
                        .cloned()
                    else {
                        self.error = Some("The recording display is no longer available.".into());
                        self.finish_capture(ctx, false);
                        continue;
                    };
                    let selection = self.controls.lock().unwrap().recording_selection();
                    let options = match recording_options(
                        &selection,
                        target,
                        &display,
                        self.window_session.as_deref(),
                    ) {
                        Ok(options) => options,
                        Err(error) => {
                            self.error = Some(error);
                            self.finish_capture(ctx, false);
                            continue;
                        }
                    };
                    self.selector_scope_generation.store(0, Ordering::Release);
                    self.capture_phase = Some(CapturePhase::RecordingPreparing { target });
                    self.recording_worker.send(recording::Command::Prepare {
                        generation,
                        recovery_root: recording_recovery_root(&self.root),
                        options,
                        display,
                    });
                    self.status = "Preparing recording… Press Escape to cancel.".into();
                    request_hidden_root_paint(ctx);
                }
                SelectorMessage::PauseRecording { generation }
                    if self.flow.as_ref().map(CaptureFlow::generation) == Some(generation)
                        && self.capture_phase == Some(CapturePhase::Recording) =>
                {
                    self.capture_phase = Some(CapturePhase::RecordingPausing);
                    self.status = "Pausing recording…".into();
                    self.recording_worker
                        .send(recording::Command::Pause { generation });
                }
                SelectorMessage::ResumeRecording { generation }
                    if self.flow.as_ref().map(CaptureFlow::generation) == Some(generation)
                        && self.capture_phase == Some(CapturePhase::RecordingPaused) =>
                {
                    self.capture_phase = Some(CapturePhase::RecordingStarting);
                    self.recording_worker.send(recording::Command::Resume {
                        generation,
                        exclude_captures_app: recording_controls_are_excluded(
                            self.include_recording_controls,
                        ),
                    });
                }
                SelectorMessage::SetMicrophoneMuted { generation, muted }
                    if self.flow.as_ref().map(CaptureFlow::generation) == Some(generation)
                        && matches!(
                            self.capture_phase,
                            Some(CapturePhase::Recording | CapturePhase::RecordingPaused)
                        ) =>
                {
                    let paused = self.capture_phase == Some(CapturePhase::RecordingPaused);
                    self.capture_phase = Some(CapturePhase::RecordingMuting { paused });
                    self.status = if muted {
                        "Muting microphone…".into()
                    } else {
                        "Unmuting microphone…".into()
                    };
                    self.recording_worker
                        .send(recording::Command::SetMicrophoneMuted {
                            generation,
                            muted,
                            exclude_captures_app: recording_controls_are_excluded(
                                self.include_recording_controls,
                            ),
                        });
                }
                SelectorMessage::RestartRecording { generation }
                    if self.flow.as_ref().map(CaptureFlow::generation) == Some(generation)
                        && matches!(
                            self.capture_phase,
                            Some(CapturePhase::Recording | CapturePhase::RecordingPaused)
                        ) =>
                {
                    self.recording_restart_confirmation = true;
                    request_hidden_root_paint(ctx);
                }
                SelectorMessage::StartRecordingScreenshot { generation }
                    if self.flow.as_ref().map(CaptureFlow::generation) == Some(generation)
                        && matches!(
                            self.capture_phase,
                            Some(CapturePhase::Recording | CapturePhase::RecordingPaused)
                        ) =>
                {
                    self.start_recording_screenshot(ctx);
                }
                SelectorMessage::CancelRestartRecording { generation }
                    if self.flow.as_ref().map(CaptureFlow::generation) == Some(generation) =>
                {
                    self.recording_restart_confirmation = false;
                    request_hidden_root_paint(ctx);
                }
                SelectorMessage::ConfirmRestartRecording { generation }
                    if self.flow.as_ref().map(CaptureFlow::generation) == Some(generation)
                        && self.recording_restart_confirmation
                        && matches!(
                            self.capture_phase,
                            Some(CapturePhase::Recording | CapturePhase::RecordingPaused)
                        ) =>
                {
                    self.recording_restart_confirmation = false;
                    let seconds = self
                        .recording_snapshot
                        .as_ref()
                        .map(|snapshot| snapshot.options.countdown_seconds)
                        .unwrap_or(0);
                    let Some(flow) = &mut self.flow else {
                        continue;
                    };
                    match flow.restart_countdown(seconds) {
                        Ok(()) => {
                            self.recording_controls_hidden = None;
                            self.recording_hidden_notice_until = None;
                            self.capture_phase = Some(CapturePhase::RecordingRestarting);
                            self.status = "Restarting recording…".into();
                            self.recording_worker
                                .send(recording::Command::Restart { generation });
                            request_hidden_root_paint(ctx);
                        }
                        Err(error) => {
                            self.error =
                                Some(format!("Could not arm restarted recording Escape: {error}"));
                        }
                    }
                }
                SelectorMessage::StopRecording { generation }
                    if self.flow.as_ref().map(CaptureFlow::generation) == Some(generation)
                        && matches!(
                            self.capture_phase,
                            Some(CapturePhase::Recording | CapturePhase::RecordingPaused)
                        ) =>
                {
                    self.capture_phase = Some(CapturePhase::RecordingFinalizing);
                    self.status = "Finalizing recording…".into();
                    self.recording_worker.send(recording::Command::Finish {
                        generation,
                        history_root: self.root.clone(),
                    });
                }
                SelectorMessage::DiscardRecording { generation }
                    if self.flow.as_ref().map(CaptureFlow::generation) == Some(generation)
                        && matches!(
                            self.capture_phase,
                            Some(CapturePhase::Recording | CapturePhase::RecordingPaused)
                        ) =>
                {
                    self.capture_phase = Some(CapturePhase::RecordingDiscarding);
                    self.status = "Discarding recording…".into();
                    self.recording_worker
                        .send(recording::Command::Discard { generation });
                }
                SelectorMessage::HideRecordingControls { generation }
                    if self.flow.as_ref().map(CaptureFlow::generation) == Some(generation)
                        && self.recording_restore_available
                        && matches!(
                            self.capture_phase,
                            Some(CapturePhase::Recording | CapturePhase::RecordingPaused)
                        ) =>
                {
                    self.recording_controls_hidden = Some(generation);
                    self.recording_hidden_notice_until =
                        Some(Instant::now() + Duration::from_millis(6_200));
                    self.recording_restart_confirmation = false;
                    request_hidden_root_paint(ctx);
                    ctx.request_repaint_after(Duration::from_millis(6_200));
                }
                SelectorMessage::SwitchControlsDisplay {
                    generation,
                    display_id,
                } if accepts_selector_action(
                    self.flow.as_ref().map(CaptureFlow::generation),
                    generation,
                    self.flow.as_ref().is_some_and(CaptureFlow::is_current),
                    self.capture_phase,
                    SelectorKind::Controls,
                ) =>
                {
                    if !self.displays.iter().any(|display| display.id == display_id) {
                        self.selector_scope_generation
                            .store(generation, Ordering::Release);
                        self.error = Some("The selected display is no longer available.".into());
                        continue;
                    }
                    self.display_id = Some(display_id);
                    let Some(target) =
                        capture_target(frame, &self.displays, self.display_id.as_deref())
                    else {
                        self.error = Some(
                            "The selected display is unavailable for capture controls.".into(),
                        );
                        self.finish_capture(ctx, false);
                        continue;
                    };
                    self.countdown_target = Some(target);
                    self.previews.capture_target = Some(target);
                    self.selector_scope_generation.store(0, Ordering::Release);
                    self.controls.lock().unwrap().reset_for_display_change();
                    self.window_session = None;
                    self.window_texture = None;
                    self.capture_phase = Some(CapturePhase::ControlsPreparing);
                    self.status = "Switching capture controls to the selected display…".into();
                    request_hidden_root_paint(ctx);
                    self.begin_root_hide(ctx);
                }
                SelectorMessage::ConfirmRegion { .. }
                | SelectorMessage::ConfirmWindow { .. }
                | SelectorMessage::ConfirmControls { .. }
                | SelectorMessage::StartRecording { .. }
                | SelectorMessage::PauseRecording { .. }
                | SelectorMessage::ResumeRecording { .. }
                | SelectorMessage::SetMicrophoneMuted { .. }
                | SelectorMessage::RestartRecording { .. }
                | SelectorMessage::StartRecordingScreenshot { .. }
                | SelectorMessage::ConfirmRestartRecording { .. }
                | SelectorMessage::CancelRestartRecording { .. }
                | SelectorMessage::StopRecording { .. }
                | SelectorMessage::DiscardRecording { .. }
                | SelectorMessage::HideRecordingControls { .. }
                | SelectorMessage::SwitchControlsDisplay { .. }
                | SelectorMessage::Cancel { .. } => {}
            }
        }
        while let Ok(message) = self.preview_rx.try_recv() {
            match message {
                PreviewMessage::Copy {
                    artifact_id,
                    generation,
                } if self.previews.accepts(&artifact_id, generation) => {
                    let path = {
                        let card = self
                            .previews
                            .cards
                            .get_mut(&artifact_id)
                            .expect("accepted preview exists");
                        card.busy = Some(crate::mini_preview::Busy::Copy);
                        card.message = Some("Copying full-resolution pixels…".into());
                        card.image_path.clone()
                    };
                    self.pending += 1;
                    let _ = self.tx.send(Job::Copy {
                        path,
                        preview: Some(PreviewGuard {
                            artifact_id,
                            generation,
                        }),
                    });
                }
                PreviewMessage::Save {
                    artifact_id,
                    generation,
                    directory,
                    format,
                } if self.previews.accepts(&artifact_id, generation) => {
                    let id = {
                        let card = self
                            .previews
                            .cards
                            .get_mut(&artifact_id)
                            .expect("accepted preview exists");
                        card.busy = Some(crate::mini_preview::Busy::Save);
                        card.message = Some("Saving with current preferences…".into());
                        card.artifact_id.clone()
                    };
                    self.send_preview(
                        Request::SaveScreenshot {
                            root: self.root.clone(),
                            id,
                            directory,
                            format,
                        },
                        PreviewGuard {
                            artifact_id,
                            generation,
                        },
                    );
                }
                PreviewMessage::OpenHistory {
                    artifact_id,
                    generation,
                } if self.previews.accepts(&artifact_id, generation) => {
                    self.select(artifact_id);
                    self.open_history_requested = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.request_repaint();
                }
                PreviewMessage::Dismiss {
                    artifact_id,
                    generation,
                } => {
                    if self.previews.dismiss(&artifact_id, generation) {
                        request_hidden_root_paint(ctx);
                        ctx.request_repaint();
                    }
                }
                PreviewMessage::ToggleCollapsed => {
                    self.previews
                        .stack
                        .set_collapsed(!self.previews.stack.is_collapsed());
                    ctx.request_repaint();
                }
                PreviewMessage::ClearAll { artifact_ids } => {
                    if self.previews.clear(&artifact_ids) > 0 {
                        request_hidden_root_paint(ctx);
                        ctx.request_repaint();
                    }
                }
                PreviewMessage::Copy { .. }
                | PreviewMessage::Save { .. }
                | PreviewMessage::OpenHistory { .. } => {}
            }
        }
        while let Some(event) = self.recording_worker.try_recv() {
            match event {
                recording::Event::ToolchainVerified { generation, result }
                    if self.flow.as_ref().map(CaptureFlow::generation) == Some(generation) =>
                {
                    self.recording_toolchain_ready = result.is_ok();
                    self.recording_toolchain_error = result.err();
                }
                recording::Event::Microphones {
                    generation,
                    devices,
                } if self.flow.as_ref().map(CaptureFlow::generation) == Some(generation) => {
                    self.controls.lock().unwrap().set_microphones(devices);
                }
                recording::Event::Snapshot { generation, result }
                    if self.flow.as_ref().map(CaptureFlow::generation) == Some(generation) =>
                {
                    self.recording_snapshot_poll_pending = false;
                    match result {
                        Ok(snapshot) => {
                            self.recording_segment_started =
                                snapshot_interpolation_origin(snapshot.state, Instant::now());
                            self.recording_snapshot = Some(snapshot);
                        }
                        Err(error) => self.error = Some(error),
                    }
                }
                recording::Event::Prepared { generation, result }
                    if accepts_recording_event(
                        self.flow.as_ref().map(CaptureFlow::generation),
                        generation,
                        self.flow.as_ref().is_some_and(CaptureFlow::is_current),
                        self.capture_phase,
                        |phase| matches!(phase, CapturePhase::RecordingPreparing { .. }),
                    ) =>
                {
                    match result {
                        Ok(snapshot) => {
                            let seconds = snapshot.options.countdown_seconds;
                            self.recording_snapshot = Some(snapshot);
                            let Some(flow) = &mut self.flow else { continue };
                            match flow.start_countdown(seconds) {
                                Ok(()) => {
                                    self.capture_phase = Some(CapturePhase::RecordingCountdown);
                                    self.status = "Recording ready. Press Escape to cancel.".into();
                                    request_hidden_root_paint(ctx);
                                }
                                Err(error) => {
                                    self.error = Some(error);
                                    self.recording_worker
                                        .send(recording::Command::Discard { generation });
                                    self.capture_phase = Some(CapturePhase::RecordingDiscarding);
                                }
                            }
                        }
                        Err(error) => {
                            self.error = Some(error);
                            self.finish_capture(ctx, false);
                        }
                    }
                }
                recording::Event::Started { generation, result }
                    if accepts_recording_event(
                        self.flow.as_ref().map(CaptureFlow::generation),
                        generation,
                        self.flow.as_ref().is_some_and(CaptureFlow::is_current),
                        self.capture_phase,
                        |phase| phase == CapturePhase::RecordingStarting,
                    ) =>
                {
                    match result {
                        Ok(snapshot) => {
                            let disarmed = self
                                .flow
                                .as_mut()
                                .expect("accepted recording start owns flow")
                                .disarm_escape();
                            if let Err(error) = disarmed {
                                self.error =
                                    Some(format!("Recording Escape handoff failed: {error}"));
                                if self.recording_has_started {
                                    self.capture_phase = Some(CapturePhase::RecordingFinalizing);
                                    self.recording_worker.send(recording::Command::Finish {
                                        generation,
                                        history_root: self.root.clone(),
                                    });
                                } else {
                                    self.capture_phase = Some(CapturePhase::RecordingDiscarding);
                                    self.recording_worker
                                        .send(recording::Command::Discard { generation });
                                }
                                continue;
                            }
                            self.recording_has_started = true;
                            self.recording_snapshot = Some(snapshot);
                            self.recording_segment_started = Some(Instant::now());
                            self.recording_snapshot_poll_pending = false;
                            self.recording_last_snapshot_poll = Instant::now();
                            self.previews.restore_capture();
                            self.capture_phase = Some(CapturePhase::Recording);
                            self.status = "Recording in progress".into();
                            request_hidden_root_paint(ctx);
                        }
                        Err(error) => {
                            self.error = Some(error);
                            if self.recording_has_started {
                                self.capture_phase = Some(CapturePhase::RecordingFinalizing);
                                self.status =
                                    "Resume failed; preserving the accepted recording…".into();
                                self.recording_worker.send(recording::Command::Finish {
                                    generation,
                                    history_root: self.root.clone(),
                                });
                            } else {
                                self.finish_capture(ctx, false);
                            }
                        }
                    }
                }
                recording::Event::Paused { generation, result }
                    if accepts_recording_event(
                        self.flow.as_ref().map(CaptureFlow::generation),
                        generation,
                        self.flow.as_ref().is_some_and(CaptureFlow::is_current),
                        self.capture_phase,
                        |phase| phase == CapturePhase::RecordingPausing,
                    ) =>
                {
                    match result {
                        Ok(snapshot) => {
                            self.recording_snapshot = Some(snapshot);
                            self.recording_segment_started = None;
                            self.capture_phase = Some(CapturePhase::RecordingPaused);
                            self.status = "Recording paused".into();
                        }
                        Err(error) => {
                            self.error = Some(error);
                            self.finish_capture(ctx, false);
                        }
                    }
                }
                recording::Event::MicrophoneMuted { generation, result }
                    if accepts_recording_event(
                        self.flow.as_ref().map(CaptureFlow::generation),
                        generation,
                        self.flow.as_ref().is_some_and(CaptureFlow::is_current),
                        self.capture_phase,
                        |phase| matches!(phase, CapturePhase::RecordingMuting { .. }),
                    ) =>
                {
                    match result {
                        Ok(snapshot) => {
                            self.recording_segment_started =
                                snapshot_interpolation_origin(snapshot.state, Instant::now());
                            self.capture_phase =
                                Some(if snapshot.state == RecordingState::Paused {
                                    CapturePhase::RecordingPaused
                                } else {
                                    CapturePhase::Recording
                                });
                            self.status = if snapshot.state == RecordingState::Paused {
                                "Recording paused".into()
                            } else {
                                "Recording in progress".into()
                            };
                            self.recording_snapshot = Some(snapshot);
                        }
                        Err(failure) => {
                            self.error = Some(format!(
                                "Could not change microphone; accepted media was preserved: {}",
                                failure.error
                            ));
                            if failure
                                .snapshot
                                .is_some_and(|snapshot| snapshot.state == RecordingState::Failed)
                            {
                                self.finish_capture(ctx, false);
                            } else if self.recording_has_started {
                                self.capture_phase = Some(CapturePhase::RecordingFinalizing);
                                self.status =
                                    "Microphone change failed; preserving recording…".into();
                                self.recording_worker.send(recording::Command::Finish {
                                    generation,
                                    history_root: self.root.clone(),
                                });
                            } else {
                                self.finish_capture(ctx, false);
                            }
                        }
                    }
                }
                recording::Event::Restarted { generation, result }
                    if accepts_recording_event(
                        self.flow.as_ref().map(CaptureFlow::generation),
                        generation,
                        self.flow.as_ref().is_some_and(CaptureFlow::is_current),
                        self.capture_phase,
                        |phase| phase == CapturePhase::RecordingRestarting,
                    ) =>
                {
                    match result {
                        Ok(snapshot) => {
                            self.recording_has_started = false;
                            self.recording_snapshot = Some(snapshot);
                            self.recording_segment_started = None;
                            self.recording_snapshot_poll_pending = false;
                            self.capture_phase = Some(CapturePhase::RecordingCountdown);
                            self.status = "Recording ready. Press Escape to cancel.".into();
                            request_hidden_root_paint(ctx);
                        }
                        Err(error) => {
                            self.error = Some(format!(
                                "Could not restart recording; recovery files were preserved: {error}"
                            ));
                            self.finish_capture(ctx, false);
                        }
                    }
                }
                recording::Event::Finished { generation, result }
                    if accepts_recording_event(
                        self.flow.as_ref().map(CaptureFlow::generation),
                        generation,
                        true,
                        self.capture_phase,
                        |phase| phase == CapturePhase::RecordingFinalizing,
                    ) =>
                {
                    match result {
                        Ok(finalized) => {
                            self.status = finalized.warning.map_or_else(
                                || format!("Recording saved to {}", finalized.path.display()),
                                |warning| {
                                    format!(
                                        "Recording saved to {} — {warning}",
                                        finalized.path.display()
                                    )
                                },
                            );
                            self.history_refresh_status = Some(self.status.clone());
                            self.finish_capture(ctx, false);
                            self.load_history();
                        }
                        Err(error) => {
                            self.error = Some(error);
                            self.finish_capture(ctx, false);
                        }
                    }
                }
                recording::Event::Discarded { generation, result }
                    if accepts_recording_event(
                        self.flow.as_ref().map(CaptureFlow::generation),
                        generation,
                        true,
                        self.capture_phase,
                        |phase| phase == CapturePhase::RecordingDiscarding,
                    ) =>
                {
                    if let Err(error) = result {
                        self.error = Some(error);
                    }
                    self.finish_capture(ctx, false);
                }
                recording::Event::ToolchainVerified { .. }
                | recording::Event::Microphones { .. }
                | recording::Event::Snapshot { .. }
                | recording::Event::Prepared { .. }
                | recording::Event::Started { .. }
                | recording::Event::Paused { .. }
                | recording::Event::MicrophoneMuted { .. }
                | recording::Event::Restarted { .. }
                | recording::Event::Finished { .. }
                | recording::Event::Discarded { .. } => {}
            }
        }
        if self.root_hide_deferred
            && self.flow.as_ref().is_some_and(CaptureFlow::is_current)
            && self
                .previews
                .omission_frame
                .is_none_or(|frame| ctx.cumulative_frame_nr() > frame)
        {
            self.root_hide_deferred = false;
            self.begin_root_hide(ctx);
        }
        if let Some(flow) = &self.flow {
            if !flow.is_current() {
                match self.capture_phase {
                    Some(CapturePhase::RecordingRestarting) => {
                        let generation = flow.generation();
                        self.recording_worker
                            .send(recording::Command::Discard { generation });
                        self.capture_phase = Some(CapturePhase::RecordingDiscarding);
                        self.status = "Discarding cancelled recording restart…".into();
                    }
                    Some(
                        CapturePhase::Recording
                        | CapturePhase::RecordingPausing
                        | CapturePhase::RecordingPaused
                        | CapturePhase::RecordingMuting { .. }
                        | CapturePhase::RecordingStarting,
                    ) if self.recording_has_started => {
                        let generation = flow.generation();
                        self.recording_worker.send(recording::Command::Finish {
                            generation,
                            history_root: self.root.clone(),
                        });
                        self.capture_phase = Some(CapturePhase::RecordingFinalizing);
                        self.status = "Desktop session changed; preserving recording…".into();
                    }
                    Some(CapturePhase::RecordingFinalizing | CapturePhase::RecordingDiscarding) => {
                    }
                    Some(phase) if is_recording_phase(Some(phase)) => {
                        let generation = flow.generation();
                        self.recording_worker
                            .send(recording::Command::Discard { generation });
                        self.capture_phase = Some(CapturePhase::RecordingDiscarding);
                        self.status = "Discarding cancelled recording…".into();
                    }
                    _ => {
                        self.status =
                            "Capture cancelled (Escape or desktop session unavailable).".into();
                        self.finish_capture(ctx, false);
                    }
                }
            } else {
                if matches!(
                    self.capture_phase,
                    Some(CapturePhase::Recording | CapturePhase::RecordingPaused)
                ) && !self.recording_snapshot_poll_pending
                    && self.recording_last_snapshot_poll.elapsed() >= Duration::from_millis(250)
                {
                    self.recording_snapshot_poll_pending = true;
                    self.recording_last_snapshot_poll = Instant::now();
                    self.recording_worker.send(recording::Command::Snapshot {
                        generation: flow.generation(),
                    });
                }
                // Only active captures poll; settled history/preferences stay event-driven.
                // A running timer remains smooth, while paused recordings wake only for the
                // next bounded warning snapshot or a worker response.
                match self.capture_phase {
                    Some(CapturePhase::Recording) => {
                        ctx.request_repaint_after(Duration::from_millis(100));
                    }
                    Some(CapturePhase::RecordingPaused)
                        if !self.recording_snapshot_poll_pending =>
                    {
                        ctx.request_repaint_after(
                            Duration::from_millis(250)
                                .saturating_sub(self.recording_last_snapshot_poll.elapsed()),
                        );
                    }
                    Some(CapturePhase::RecordingPaused) => {}
                    _ => ctx.request_repaint_after(Duration::from_millis(100)),
                }
            }
        }
        if let Some(flow) = &self.recording_screenshot_flow {
            if !flow.is_current() {
                self.finish_recording_screenshot(ctx, false);
            } else {
                let generation = flow.generation();
                match self.recording_screenshot_phase {
                    Some(RecordingScreenshotPhase::WaitingForHud)
                        if self
                            .recording_screenshot_hide_started
                            .is_some_and(|started| {
                                started.elapsed() >= Duration::from_millis(300)
                            }) =>
                    {
                        let Some(display_id) = self.display_id.clone() else {
                            self.error =
                                Some("The recording display is no longer available.".into());
                            self.finish_recording_screenshot(ctx, false);
                            return;
                        };
                        let settings = self
                            .recording_screenshot_settings
                            .as_ref()
                            .expect("recording screenshot retains settings");
                        self.recording_screenshot_phase = Some(RecordingScreenshotPhase::Preparing);
                        self.pending += 1;
                        let _ = self.tx.send(Job::PrepareRegion {
                            display_id,
                            generation,
                            freeze: settings.freeze_screen,
                            include_cursor: settings.show_cursor_in_screenshots,
                        });
                    }
                    Some(RecordingScreenshotPhase::WaitingForHud) => {
                        ctx.request_repaint_after(Duration::from_millis(16));
                    }
                    Some(RecordingScreenshotPhase::Countdown {
                        rect,
                        after_countdown,
                    }) if flow.countdown().remaining(Instant::now()) == 0 => {
                        let Some(session) = self.recording_screenshot_session.take() else {
                            self.error = Some("Region preparation was lost before capture.".into());
                            self.finish_recording_screenshot(ctx, false);
                            return;
                        };
                        self.recording_screenshot_texture = None;
                        self.recording_screenshot_phase = Some(RecordingScreenshotPhase::Capturing);
                        self.pending += 1;
                        let _ = self.tx.send(Job::CaptureRegion {
                            root: self.root.clone(),
                            generation,
                            session,
                            rect,
                            after_countdown,
                        });
                    }
                    Some(RecordingScreenshotPhase::Countdown { .. }) => {
                        ctx.request_repaint_after(Duration::from_millis(100));
                    }
                    _ => {}
                }
            }
        }
        if self.capture_waiting_for_hide {
            let visible = frame.winit_window().and_then(|window| window.is_visible());
            if visible == Some(false) {
                self.hidden_since.get_or_insert_with(Instant::now);
            } else {
                self.hidden_since = None;
            }
            if self
                .hidden_since
                .is_some_and(|since| since.elapsed() >= Duration::from_millis(150))
            {
                self.capture_waiting_for_hide = false;
                let Some(display_id) = self.display_id.clone() else {
                    self.flow = None;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    return;
                };
                let Some(flow) = &self.flow else { return };
                let generation = flow.generation();
                match self.capture_phase {
                    Some(CapturePhase::DisplayCountdown) => {
                        self.capture_in_flight = true;
                        self.capture_phase = Some(CapturePhase::DisplayCapturing);
                        self.send(Request::CaptureDisplay {
                            root: self.root.clone(),
                            display_id,
                            generation,
                            include_cursor: self.include_cursor,
                        });
                    }
                    Some(CapturePhase::RegionPreparing) => {
                        self.pending += 1;
                        let _ = self.tx.send(Job::PrepareRegion {
                            display_id,
                            generation,
                            freeze: self.region_freeze,
                            include_cursor: self.include_cursor,
                        });
                    }
                    Some(CapturePhase::WindowPreparing) => {
                        self.pending += 1;
                        let _ = self.tx.send(Job::PrepareWindow {
                            display_id,
                            generation,
                            freeze: self.window_freeze,
                            include_cursor: self.include_cursor,
                        });
                    }
                    Some(CapturePhase::ControlsPreparing) => {
                        self.pending += 1;
                        let _ = self.tx.send(Job::PrepareWindow {
                            display_id,
                            generation,
                            freeze: self.controls_freeze,
                            include_cursor: self.include_cursor,
                        });
                    }
                    Some(CapturePhase::RecordingCountdown) => {
                        self.capture_phase = Some(CapturePhase::RecordingStarting);
                        self.recording_worker.send(recording::Command::Start {
                            generation,
                            exclude_captures_app: recording_controls_are_excluded(
                                self.include_recording_controls,
                            ),
                        });
                        self.status = "Starting recording…".into();
                    }
                    Some(CapturePhase::RegionCountdown {
                        rect,
                        after_countdown,
                    }) => {
                        let Some(session) = self.region_session.take() else {
                            self.error = Some("Region preparation was lost before capture.".into());
                            self.finish_capture(ctx, false);
                            return;
                        };
                        self.region_texture = None;
                        self.capture_in_flight = true;
                        self.capture_phase = Some(CapturePhase::RegionCapturing);
                        self.pending += 1;
                        let _ = self.tx.send(Job::CaptureRegion {
                            root: self.root.clone(),
                            generation,
                            session,
                            rect,
                            after_countdown,
                        });
                    }
                    Some(CapturePhase::WindowCountdown {
                        target,
                        after_countdown,
                    }) => {
                        let Some(session) = self.window_session.take() else {
                            self.error = Some("Window preparation was lost before capture.".into());
                            self.finish_capture(ctx, false);
                            return;
                        };
                        let target = match target {
                            SelectionTarget::Display => WindowCaptureTarget::Display,
                            SelectionTarget::Window(index) => {
                                let Some(window) = session.windows().get(index) else {
                                    self.error =
                                        Some("The selected window changed before capture.".into());
                                    self.finish_capture(ctx, false);
                                    return;
                                };
                                WindowCaptureTarget::Window {
                                    id: window.id.clone(),
                                }
                            }
                        };
                        self.window_texture = None;
                        self.capture_in_flight = true;
                        self.capture_phase = Some(CapturePhase::WindowCapturing);
                        self.pending += 1;
                        let _ = self.tx.send(Job::CaptureWindow {
                            root: self.root.clone(),
                            generation,
                            session,
                            target,
                            after_countdown,
                        });
                    }
                    Some(CapturePhase::ControlsCountdown {
                        target,
                        after_countdown,
                    }) => {
                        let Some(session) = self.window_session.take() else {
                            self.error =
                                Some("Capture-control preparation was lost before capture.".into());
                            self.finish_capture(ctx, false);
                            return;
                        };
                        let target = match target {
                            capture_controls::Target::Region(rect) => {
                                WindowCaptureTarget::Region { rect }
                            }
                            capture_controls::Target::Display => WindowCaptureTarget::Display,
                            capture_controls::Target::Window(index) => {
                                let Some(window) = session.windows().get(index) else {
                                    self.error =
                                        Some("The selected window changed before capture.".into());
                                    self.finish_capture(ctx, false);
                                    return;
                                };
                                WindowCaptureTarget::Window {
                                    id: window.id.clone(),
                                }
                            }
                        };
                        self.window_texture = None;
                        self.capture_in_flight = true;
                        self.capture_phase = Some(CapturePhase::ControlsCapturing);
                        self.pending += 1;
                        let _ = self.tx.send(Job::CaptureWindow {
                            root: self.root.clone(),
                            generation,
                            session,
                            target,
                            after_countdown,
                        });
                    }
                    _ => {}
                }
            } else if self
                .hide_started
                .is_some_and(|since| since.elapsed() > Duration::from_secs(2))
            {
                self.capture_waiting_for_hide = false;
                self.error =
                    Some("Could not hide the capture window. No screenshot was taken.".into());
                self.finish_capture(ctx, false);
            } else {
                ctx.request_repaint_after(Duration::from_millis(16));
            }
        }
        while let Ok(reply) = self.rx.try_recv() {
            match reply {
                Reply::HistoryLoaded(result) => {
                    self.pending = self.pending.saturating_sub(1);
                    match result {
                        Ok(artifacts) => self.apply(Response::History { artifacts }, false),
                        Err(error) => self.error = Some(error),
                    }
                }
                Reply::HistoryCleared(result) => {
                    self.pending = self.pending.saturating_sub(1);
                    match result {
                        Ok(response) => self.apply(*response, true),
                        Err(error) => {
                            // Some files may already have been deleted. Refresh the
                            // remaining history without concealing the operation error.
                            self.load_history();
                            self.error = Some(format!("Could not clear history: {error}"));
                        }
                    }
                }
                Reply::Copied { preview, result } => {
                    self.pending = self.pending.saturating_sub(1);
                    if let Some(preview) = preview {
                        if let Some(card) = self
                            .previews
                            .cards
                            .get_mut(&preview.artifact_id)
                            .filter(|card| card.generation == preview.generation)
                        {
                            card.busy = None;
                            card.message = Some(match result {
                                Ok(()) => "Copied full-resolution pixels".into(),
                                Err(error) => format!("Copy failed: {error}"),
                            });
                        }
                    } else {
                        match result {
                            Ok(()) => self.status = "Copied actual capture pixels".into(),
                            Err(error) => self.error = Some(error),
                        }
                    }
                }
                Reply::Executed { preview, result } => {
                    self.pending = self.pending.saturating_sub(1);
                    if self.capture_phase == Some(CapturePhase::DisplayCapturing) {
                        self.capture_in_flight = false;
                        let captured = matches!(result.as_deref(), Ok(Response::Captured { .. }));
                        self.finish_capture(ctx, captured);
                    }
                    match result {
                        Err(error) => {
                            if let Some(preview) = preview {
                                if let Some(card) = self
                                    .previews
                                    .cards
                                    .get_mut(&preview.artifact_id)
                                    .filter(|card| card.generation == preview.generation)
                                {
                                    card.busy = None;
                                    card.message = Some(format!("Save failed: {error}"));
                                }
                            } else {
                                self.error = Some(error);
                            }
                        }
                        Ok(response) => {
                            let announce = preview.as_ref().is_none_or(|preview| {
                                self.previews
                                    .accepts(&preview.artifact_id, preview.generation)
                            });
                            self.apply(*response, announce);
                            if let Some(preview) = preview
                                && let Some(card) = self
                                    .previews
                                    .cards
                                    .get_mut(&preview.artifact_id)
                                    .filter(|card| card.generation == preview.generation)
                            {
                                card.busy = None;
                                card.message = Some("Saved with current preferences".into());
                            }
                        }
                    }
                }
                Reply::RegionPrepared { generation, result } => {
                    self.pending = self.pending.saturating_sub(1);
                    let recording_screenshot =
                        self.recording_screenshot_flow.as_ref().is_some_and(|flow| {
                            flow.generation() == generation && flow.is_current()
                        }) && self.recording_screenshot_phase
                            == Some(RecordingScreenshotPhase::Preparing);
                    if recording_screenshot {
                        match result {
                            Ok(session) => {
                                let expected = self
                                    .displays
                                    .iter()
                                    .find(|display| Some(&display.id) == self.display_id.as_ref());
                                if !expected.is_some_and(|display| {
                                    same_display_geometry(display, session.display())
                                }) {
                                    self.error = Some(
                                        "The recording display changed while preparing the screenshot."
                                            .into(),
                                    );
                                    self.finish_recording_screenshot(ctx, false);
                                    continue;
                                }
                                self.recording_screenshot_texture =
                                    session.frozen_image().map(|image| {
                                        ctx.load_texture(
                                            format!("recording-screenshot-frozen-{generation}"),
                                            egui::ColorImage::from_rgba_unmultiplied(
                                                [image.width() as usize, image.height() as usize],
                                                image.as_raw(),
                                            ),
                                            egui::TextureOptions::LINEAR,
                                        )
                                    });
                                self.recording_screenshot_session = Some(session);
                                self.region_selector.lock().unwrap().reset();
                                self.recording_screenshot_phase =
                                    Some(RecordingScreenshotPhase::Selecting);
                                self.status =
                                    "Select a screenshot region. Press Escape to cancel.".into();
                                request_hidden_root_paint(ctx);
                            }
                            Err(error) => {
                                self.error = Some(error);
                                self.finish_recording_screenshot(ctx, false);
                            }
                        }
                        continue;
                    }
                    let accepted = accepts_prepare_reply(
                        self.flow.as_ref().map(CaptureFlow::generation),
                        generation,
                        self.flow.as_ref().is_some_and(CaptureFlow::is_current),
                        self.capture_phase,
                        CapturePhase::RegionPreparing,
                    );
                    if !accepted {
                        continue;
                    }
                    match result {
                        Ok(session) => {
                            let expected = self
                                .displays
                                .iter()
                                .find(|display| Some(&display.id) == self.display_id.as_ref());
                            if !expected.is_some_and(|display| {
                                same_display_geometry(display, session.display())
                            }) {
                                self.error = Some(
                                    "The selected display changed while preparing the region."
                                        .into(),
                                );
                                self.finish_capture(ctx, false);
                                continue;
                            }
                            self.region_texture = session.frozen_image().map(|image| {
                                ctx.load_texture(
                                    format!("region-frozen-{generation}"),
                                    egui::ColorImage::from_rgba_unmultiplied(
                                        [image.width() as usize, image.height() as usize],
                                        image.as_raw(),
                                    ),
                                    egui::TextureOptions::LINEAR,
                                )
                            });
                            self.region_session = Some(session);
                            self.region_selector.lock().unwrap().reset();
                            self.capture_phase = Some(CapturePhase::RegionSelecting);
                            self.status = "Select a region. Press Escape to cancel.".into();
                            request_hidden_root_paint(ctx);
                            ctx.request_repaint();
                        }
                        Err(error) => {
                            self.error = Some(error);
                            self.finish_capture(ctx, false);
                        }
                    }
                }
                Reply::RegionCaptured { generation, result } => {
                    self.pending = self.pending.saturating_sub(1);
                    let recording_screenshot = self
                        .recording_screenshot_flow
                        .as_ref()
                        .is_some_and(|flow| flow.generation() == generation)
                        && self.recording_screenshot_phase
                            == Some(RecordingScreenshotPhase::Capturing);
                    if recording_screenshot {
                        let captured = result.is_ok();
                        self.finish_recording_screenshot(ctx, captured);
                        match result {
                            Ok(artifact) => self
                                .accept_artifact(*artifact, "Screenshot captured while recording"),
                            Err(error) => self.error = Some(error),
                        }
                        continue;
                    }
                    let accepted = self
                        .flow
                        .as_ref()
                        .is_some_and(|flow| flow.generation() == generation)
                        && self.capture_phase == Some(CapturePhase::RegionCapturing);
                    if !accepted {
                        continue;
                    }
                    self.capture_in_flight = false;
                    let captured = result.is_ok();
                    self.finish_capture(ctx, captured);
                    match result {
                        Ok(artifact) => self.accept_artifact(*artifact, "Region captured as PNG"),
                        Err(error) => self.error = Some(error),
                    }
                }
                Reply::WindowPrepared { generation, result } => {
                    self.pending = self.pending.saturating_sub(1);
                    let direct = accepts_prepare_reply(
                        self.flow.as_ref().map(CaptureFlow::generation),
                        generation,
                        self.flow.as_ref().is_some_and(CaptureFlow::is_current),
                        self.capture_phase,
                        CapturePhase::WindowPreparing,
                    );
                    let controls = accepts_prepare_reply(
                        self.flow.as_ref().map(CaptureFlow::generation),
                        generation,
                        self.flow.as_ref().is_some_and(CaptureFlow::is_current),
                        self.capture_phase,
                        CapturePhase::ControlsPreparing,
                    );
                    if !direct && !controls {
                        continue;
                    }
                    match result {
                        Ok(session) => {
                            let expected = self
                                .displays
                                .iter()
                                .find(|display| Some(&display.id) == self.display_id.as_ref());
                            if !expected.is_some_and(|display| {
                                same_display_geometry(display, session.display())
                            }) {
                                self.error = Some(
                                    "The selected display changed while preparing windows.".into(),
                                );
                                self.finish_capture(ctx, false);
                                continue;
                            }
                            self.window_texture = session.frozen_image().map(|image| {
                                ctx.load_texture(
                                    format!("window-frozen-{generation}"),
                                    egui::ColorImage::from_rgba_unmultiplied(
                                        [image.width() as usize, image.height() as usize],
                                        image.as_raw(),
                                    ),
                                    egui::TextureOptions::LINEAR,
                                )
                            });
                            self.window_session = Some(session);
                            if controls {
                                let auto_capture_display = self.controls_auto_start
                                    && self.controls.lock().unwrap().mode()
                                        == capture_controls::TargetMode::Display;
                                if auto_capture_display {
                                    let Some(flow) = &mut self.flow else {
                                        continue;
                                    };
                                    if let Err(error) =
                                        flow.start_countdown(self.controls_countdown_seconds)
                                    {
                                        self.error = Some(error);
                                        self.finish_capture(ctx, false);
                                        continue;
                                    }
                                    self.capture_phase = Some(CapturePhase::ControlsCountdown {
                                        target: capture_controls::Target::Display,
                                        after_countdown: self.controls_countdown_seconds > 0,
                                    });
                                    self.status = "Display changed. Press Escape to cancel.".into();
                                } else {
                                    self.capture_phase = Some(CapturePhase::ControlsSelecting);
                                    self.status =
                                        "Choose a screenshot target. Press Escape to cancel."
                                            .into();
                                }
                            } else {
                                self.window_selector.lock().unwrap().reset();
                                self.capture_phase = Some(CapturePhase::WindowSelecting);
                                self.status =
                                    "Choose a window or the display. Press Escape to cancel."
                                        .into();
                            }
                            request_hidden_root_paint(ctx);
                            ctx.request_repaint();
                        }
                        Err(error) => {
                            self.error = Some(error);
                            self.finish_capture(ctx, false);
                        }
                    }
                }
                Reply::WindowCaptured { generation, result } => {
                    self.pending = self.pending.saturating_sub(1);
                    let controls = self.capture_phase == Some(CapturePhase::ControlsCapturing);
                    let accepted = self
                        .flow
                        .as_ref()
                        .is_some_and(|flow| flow.generation() == generation)
                        && matches!(
                            self.capture_phase,
                            Some(CapturePhase::WindowCapturing | CapturePhase::ControlsCapturing)
                        );
                    if !accepted {
                        continue;
                    }
                    self.capture_in_flight = false;
                    let captured = result.is_ok();
                    self.finish_capture(ctx, captured);
                    match result {
                        Ok(artifact) => self.accept_artifact(
                            *artifact,
                            if controls {
                                "Screenshot captured as PNG"
                            } else {
                                "Window selection captured as PNG"
                            },
                        ),
                        Err(error) => self.error = Some(error),
                    }
                }
                Reply::HistoryDecoded {
                    generation,
                    path,
                    result,
                } if self.selection.accepts(generation) => match result {
                    Ok(decoded) => {
                        self.preview_loading = false;
                        self.texture = Some(ctx.load_texture(
                            format!("capture:{}", path.display()),
                            decoded.image,
                            egui::TextureOptions::LINEAR,
                        ));
                        self.decoded_path = Some(path);
                    }
                    Err(error) => {
                        self.preview_loading = false;
                        self.error = Some(error);
                    }
                },
                Reply::HistoryDecoded { .. } => {}
                Reply::PreviewDecoded {
                    generation,
                    artifact_id,
                    result,
                } if self.previews.accepts(&artifact_id, generation) => match result {
                    Ok(decoded) => {
                        self.previews.mark_ready(&artifact_id);
                        let card = self
                            .previews
                            .cards
                            .get_mut(&artifact_id)
                            .expect("accepted preview exists");
                        card.texture = Some(ctx.load_texture(
                            format!("mini-preview:{generation}:{artifact_id}"),
                            decoded.image,
                            egui::TextureOptions::LINEAR,
                        ));
                        request_hidden_root_paint(ctx);
                        ctx.request_repaint();
                    }
                    Err(error) => {
                        self.previews.dismiss(&artifact_id, generation);
                        self.error = Some(format!("Could not load mini preview: {error}"));
                    }
                },
                Reply::PreviewDecoded { .. } => {}
            }
        }
    }

    fn start_recording_screenshot(&mut self, ctx: &egui::Context) {
        let Some(parent) = self.flow.as_ref() else {
            return;
        };
        let Some(settings) = self.recording_screenshot_settings.as_ref().cloned() else {
            self.error = Some("Screenshot preferences are unavailable for this recording.".into());
            return;
        };
        let Some(target) = self.countdown_target else {
            self.error = Some("The recording display is no longer available.".into());
            return;
        };
        let flow = match parent.begin_recording_screenshot(0) {
            Ok(flow) => flow,
            Err(error) => {
                self.error = Some(format!("Could not start recording screenshot: {error}"));
                return;
            }
        };
        if let Err(error) =
            self.previews
                .begin_capture(&settings, Some(target), ctx.cumulative_frame_nr())
        {
            flow.cancel();
            self.error = Some(error);
            return;
        }
        self.auto_copy_on_capture = settings.auto_copy_to_clipboard;
        self.include_cursor = settings.show_cursor_in_screenshots;
        self.recording_screenshot_flow = Some(flow);
        self.recording_screenshot_phase = Some(RecordingScreenshotPhase::WaitingForHud);
        self.recording_screenshot_hide_started = Some(Instant::now());
        self.region_selector.lock().unwrap().reset();
        self.status = "Preparing region screenshot… Press Escape to cancel.".into();
        request_hidden_root_paint(ctx);
        ctx.request_repaint_after(Duration::from_millis(300));
    }

    fn finish_recording_screenshot(&mut self, ctx: &egui::Context, preserve_auto_copy: bool) {
        self.recording_screenshot_flow = None;
        self.recording_screenshot_phase = None;
        self.recording_screenshot_hide_started = None;
        self.recording_screenshot_session = None;
        self.recording_screenshot_texture = None;
        self.region_selector.lock().unwrap().reset();
        if !preserve_auto_copy {
            self.previews.restore_capture();
            self.auto_copy_on_capture = false;
        }
        self.status = if self.capture_phase == Some(CapturePhase::RecordingPaused) {
            "Recording paused".into()
        } else {
            "Recording in progress".into()
        };
        request_hidden_root_paint(ctx);
        ctx.request_repaint();
    }

    fn finish_capture(&mut self, ctx: &egui::Context, preserve_auto_copy: bool) {
        self.selector_scope_generation.store(0, Ordering::Release);
        self.recording_screenshot_flow = None;
        self.recording_screenshot_phase = None;
        self.recording_screenshot_hide_started = None;
        self.recording_screenshot_session = None;
        self.recording_screenshot_texture = None;
        self.recording_screenshot_settings = None;
        self.flow = None;
        self.capture_phase = None;
        self.capture_waiting_for_hide = false;
        self.hide_started = None;
        self.hidden_since = None;
        self.capture_in_flight = false;
        self.recording_snapshot = None;
        self.recording_segment_started = None;
        self.recording_snapshot_poll_pending = false;
        self.recording_has_started = false;
        self.recording_restart_confirmation = false;
        self.recording_controls_hidden = None;
        self.recording_hidden_notice_until = None;
        self.root_hide_deferred = false;
        if !preserve_auto_copy {
            self.previews.restore_capture();
            self.auto_copy_on_capture = false;
        }
        self.region_session = None;
        self.region_texture = None;
        self.region_selector.lock().unwrap().reset();
        self.window_session = None;
        self.window_texture = None;
        self.window_selector.lock().unwrap().reset();
        self.controls.lock().unwrap().reset();
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(self.restore_root_visible));
        request_hidden_root_paint(ctx);
        ctx.request_repaint();
    }

    fn accept_artifact(&mut self, artifact: Artifact, status: &str) {
        let id = artifact.entry.id.clone();
        let path = artifact.image_path.clone();
        let preview = self.previews.start_artifact(&artifact);
        self.artifacts.insert(0, artifact);
        self.select(id);
        self.status = status.into();
        match preview {
            Ok(Some((preview, path))) => {
                let _ = self.tx.send(Job::DecodePreview {
                    generation: preview.generation,
                    artifact_id: preview.artifact_id,
                    path,
                });
            }
            Ok(None) => {}
            Err(error) => self.error = Some(error),
        }
        if std::mem::take(&mut self.auto_copy_on_capture) {
            self.pending += 1;
            let _ = self.tx.send(Job::Copy {
                path,
                preview: None,
            });
        }
    }

    fn apply(&mut self, response: Response, announce: bool) {
        match response {
            Response::Displays { displays } => {
                self.display_id = self
                    .display_id
                    .take()
                    .filter(|id| displays.iter().any(|display| &display.id == id))
                    .or_else(|| {
                        displays
                            .iter()
                            .find(|display| display.is_primary)
                            .or(displays.first())
                            .map(|d| d.id.clone())
                    });
                self.displays = displays;
                self.status = "Displays refreshed".into();
            }
            Response::History { artifacts } => {
                let removed = self
                    .previews
                    .stack
                    .ids()
                    .iter()
                    .filter(|id| {
                        !artifacts
                            .iter()
                            .any(|artifact| artifact.entry.id == id.as_str())
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                self.previews.clear(&removed);
                self.confirm_clear_history = false;
                self.confirm_delete = None;
                self.selection.clear();
                self.texture = None;
                self.decoded_path = None;
                self.preview_loading = false;
                self.artifacts = artifacts;
                if let Some(id) = self.artifacts.first().map(|item| item.entry.id.clone()) {
                    self.select(id);
                }
                self.status = self
                    .history_refresh_status
                    .take()
                    .unwrap_or_else(|| "History loaded".into());
            }
            Response::Captured { artifact } => {
                self.accept_artifact(artifact, "Full display captured as PNG");
            }
            Response::Saved { artifact, path } => {
                if let Some(item) = self
                    .artifacts
                    .iter_mut()
                    .find(|item| item.entry.id == artifact.entry.id)
                {
                    *item = artifact;
                }
                if announce {
                    self.status = format!("Saved {}", path.display());
                }
            }
            Response::Deleted { id } => {
                self.previews.remove(&id);
                self.artifacts.retain(|item| item.entry.id != id);
                self.confirm_delete = None;
                self.selection.clear();
                self.texture = None;
                self.decoded_path = None;
                self.preview_loading = false;
                if let Some(id) = self.artifacts.first().map(|item| item.entry.id.clone()) {
                    self.select(id);
                }
                self.status = "Removed from capture history; exported files were kept".into();
            }
            Response::PermissionGranted => {
                self.status = "Capture permission granted".into();
                self.send(Request::Displays);
            }
            Response::HistoryRoot { .. } => {}
        }
    }

    pub fn viewports(
        &mut self,
        ctx: &egui::Context,
        tokens: &Tokens,
        settings: Result<AppSettings, String>,
    ) {
        self.capture_viewports(ctx, tokens);
        if self.flow.is_none()
            && let Ok(settings) = &settings
        {
            if self.previews.show && !settings.show_mini_previews {
                self.previews.stack.set_collapsed(false);
                self.previews.visibility.reset_session_placement();
            }
            self.previews.show = settings.show_mini_previews;
            self.previews.include_in_captures = settings.include_mini_previews_in_captures;
            self.previews.placement = settings.mini_preview_placement;
        }
        if !self.previews.is_visible() {
            return;
        }
        let Some(target) = self.previews.stack_target else {
            return;
        };
        let Some(preview_bounds) = target.preview_bounds else {
            return;
        };
        let count = self.previews.stack.ids().len();
        let collapsed = self.previews.stack.is_collapsed();
        let placement = self.previews.placement;
        let top_anchor = placement.is_top();
        let cards = self
            .previews
            .stack
            .ids()
            .iter()
            .enumerate()
            .filter_map(|(index, artifact_id)| {
                let card = self.previews.cards.get(artifact_id)?;
                Some(PreviewRenderCard {
                    artifact_id: artifact_id.clone(),
                    generation: card.generation,
                    width: card.width,
                    height: card.height,
                    texture: card.texture.clone()?,
                    busy: card.busy,
                    message: card.message.clone(),
                    layout: self.previews.stack.card_layout(index, top_anchor)?,
                })
            })
            .collect::<Vec<_>>();
        if cards.is_empty() {
            return;
        }
        let tokens = tokens.clone();
        let geometry = captures_app::preview::thumbnail_geometry(
            preview_bounds,
            count,
            collapsed,
            None,
            placement,
        );
        let sender = self.preview_tx.clone();
        let clear_ids = self.previews.stack.ids().to_vec();
        let scroll_content_height = (self.previews.stack.content_height()
            - captures_app::preview::THUMBNAIL_CONTROL_GUTTER)
            .max(0.) as f32;
        let save = settings.ok().map(|settings| {
            (
                PathBuf::from(settings.output_directory),
                settings.screenshot_format,
            )
        });
        let builder = egui::ViewportBuilder::default()
            .with_title("Captures Mini Preview")
            .with_visible(true)
            .with_position(egui::pos2(geometry.x as f32, geometry.y as f32))
            .with_inner_size(egui::vec2(
                captures_app::preview::THUMBNAIL_WIDTH as f32,
                geometry.height as f32,
            ))
            .with_min_inner_size(egui::vec2(
                captures_app::preview::THUMBNAIL_WIDTH as f32,
                geometry.height as f32,
            ))
            .with_max_inner_size(egui::vec2(
                captures_app::preview::THUMBNAIL_WIDTH as f32,
                geometry.height as f32,
            ))
            .with_decorations(false)
            .with_resizable(false)
            .with_transparent(true)
            .with_has_shadow(false)
            .with_always_on_top()
            .with_taskbar(false);
        // winit's active hint is unsupported by X11. Use it where the backend
        // implements it, and verify X11 focus behavior in the real host smoke.
        #[cfg(target_os = "windows")]
        let builder = builder.with_active(false);
        #[cfg(target_os = "linux")]
        let builder = builder
            .with_window_type(egui::X11WindowType::Notification)
            // winit does not implement its active hint on X11. Bypass WM
            // activation while retaining direct pointer input for the card.
            .with_override_redirect(true);
        ctx.show_viewport_deferred(
            egui::ViewportId::from_hash_of("live-mini-preview"),
            builder,
            move |ui, _| {
                // egui's deferred child may be mapped by the WM before its
                // builder position is applied. Reassert the absolute outer
                // position after mapping; unlike `with_monitor`, this does not
                // create a borderless-fullscreen viewport at the monitor origin.
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(
                        geometry.x as f32,
                        geometry.y as f32,
                    )));
                if ui.input(|input| input.viewport().close_requested()) {
                    let _ = sender.send(PreviewMessage::ClearAll {
                        artifact_ids: clear_ids.clone(),
                    });
                    ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
                    return;
                }
                let mut message = None;
                let mut show_card = |ui: &mut egui::Ui, card: &PreviewRenderCard| {
                    let action = crate::mini_preview::show(
                        ui,
                        &tokens,
                        crate::mini_preview::View {
                            artifact_id: &card.artifact_id,
                            texture: &card.texture,
                            width: card.width,
                            height: card.height,
                            busy: card.busy,
                            message: card.message.as_deref(),
                            can_save: save.is_some(),
                            interactive: card.layout.interactive,
                            collapsed,
                            stack_count: count,
                        },
                    );
                    let next_message = match action {
                        Some(crate::mini_preview::Action::ExpandStack) => {
                            Some(PreviewMessage::ToggleCollapsed)
                        }
                        Some(crate::mini_preview::Action::Copy) => Some(PreviewMessage::Copy {
                            artifact_id: card.artifact_id.clone(),
                            generation: card.generation,
                        }),
                        Some(crate::mini_preview::Action::Save) => {
                            save.as_ref()
                                .map(|(directory, format)| PreviewMessage::Save {
                                    artifact_id: card.artifact_id.clone(),
                                    generation: card.generation,
                                    directory: directory.clone(),
                                    format: *format,
                                })
                        }
                        Some(crate::mini_preview::Action::OpenHistory) => {
                            restore_root_for_history(ui.ctx());
                            Some(PreviewMessage::OpenHistory {
                                artifact_id: card.artifact_id.clone(),
                                generation: card.generation,
                            })
                        }
                        Some(crate::mini_preview::Action::Dismiss) => {
                            Some(PreviewMessage::Dismiss {
                                artifact_id: card.artifact_id.clone(),
                                generation: card.generation,
                            })
                        }
                        None => None,
                    };
                    if next_message.is_some() {
                        message = next_message;
                    }
                };
                if collapsed {
                    for card in &cards {
                        let rect = egui::Rect::from_min_size(
                            egui::pos2(
                                captures_app::preview::THUMBNAIL_PADDING as f32,
                                card.layout.y as f32,
                            ),
                            egui::vec2(
                                (captures_app::preview::THUMBNAIL_WIDTH
                                    - captures_app::preview::THUMBNAIL_PADDING * 2.)
                                    as f32,
                                captures_app::preview::THUMBNAIL_CARD_HEIGHT as f32,
                            ),
                        );
                        ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                            show_card(ui, card);
                        });
                    }
                } else {
                    let gutter = captures_app::preview::THUMBNAIL_CONTROL_GUTTER as f32;
                    let card_area = if top_anchor {
                        egui::Rect::from_min_max(
                            egui::pos2(0., gutter),
                            egui::pos2(
                                captures_app::preview::THUMBNAIL_WIDTH as f32,
                                geometry.height as f32,
                            ),
                        )
                    } else {
                        egui::Rect::from_min_max(
                            egui::Pos2::ZERO,
                            egui::pos2(
                                captures_app::preview::THUMBNAIL_WIDTH as f32,
                                geometry.height as f32 - gutter,
                            ),
                        )
                    };
                    ui.scope_builder(egui::UiBuilder::new().max_rect(card_area), |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("preview-stack-scroll")
                            .auto_shrink([false, false])
                            .stick_to_bottom(!top_anchor)
                            .show(ui, |ui| {
                                let (content, _) = ui.allocate_exact_size(
                                    egui::vec2(ui.available_width(), scroll_content_height),
                                    egui::Sense::hover(),
                                );
                                for card in &cards {
                                    let y =
                                        card.layout.y as f32 - if top_anchor { gutter } else { 0. };
                                    let rect = egui::Rect::from_min_size(
                                        content.min
                                            + egui::vec2(
                                                captures_app::preview::THUMBNAIL_PADDING as f32,
                                                y,
                                            ),
                                        egui::vec2(
                                            (captures_app::preview::THUMBNAIL_WIDTH
                                                - captures_app::preview::THUMBNAIL_PADDING * 2.)
                                                as f32,
                                            captures_app::preview::THUMBNAIL_CARD_HEIGHT as f32,
                                        ),
                                    );
                                    ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                                        show_card(ui, card)
                                    });
                                }
                            });
                    });
                }
                if crate::mini_preview::stack_controls_visible(count, collapsed) {
                    let gutter = captures_app::preview::THUMBNAIL_CONTROL_GUTTER as f32;
                    let padding = captures_app::preview::THUMBNAIL_PADDING as f32;
                    let controls = egui::Rect::from_min_size(
                        egui::pos2(
                            padding,
                            if top_anchor {
                                0.
                            } else {
                                geometry.height as f32 - gutter
                            },
                        ),
                        egui::vec2(
                            captures_app::preview::THUMBNAIL_WIDTH as f32 - padding * 2.,
                            gutter,
                        ),
                    );
                    ui.scope_builder(egui::UiBuilder::new().max_rect(controls), |ui| {
                        match crate::mini_preview::show_stack_controls(
                            ui, &tokens, count, collapsed,
                        ) {
                            Some(crate::mini_preview::StackAction::ToggleCollapsed) => {
                                message = Some(PreviewMessage::ToggleCollapsed);
                            }
                            Some(crate::mini_preview::StackAction::ClearAll) => {
                                message = Some(PreviewMessage::ClearAll {
                                    artifact_ids: clear_ids.clone(),
                                });
                            }
                            None => {}
                        }
                    });
                }
                if let Some(message) = message {
                    let _ = sender.send(message);
                    ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
                }
            },
        );
    }

    fn capture_viewports(&mut self, ctx: &egui::Context, t: &Tokens) {
        // The guide belongs to the recording, not the HUD. Keep it during
        // countdown, pause, restart and Hide; dropping the snapshot closes it.
        if let (Some(snapshot), Some(target)) = (&self.recording_snapshot, self.countdown_target)
            && let RecordingTarget::Region { rect, .. } = snapshot.options.target
            && self.flow.as_ref().is_some_and(CaptureFlow::is_current)
        {
            let tokens = t.clone();
            let visible = self.recording_screenshot_flow.is_none();
            ctx.show_viewport_deferred(
                egui::ViewportId::from_hash_of("recording-region-indicator"),
                egui::ViewportBuilder::default()
                    .with_title("Captures Recording Region")
                    .with_position(target.position)
                    .with_inner_size(target.size)
                    .with_transparent(true)
                    .with_decorations(false)
                    .with_always_on_top()
                    .with_taskbar(false)
                    .with_active(false)
                    .with_mouse_passthrough(true),
                move |ui, _| {
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::Visible(visible));
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::ContentProtected(true));
                    crate::recording_region::show(ui, &tokens, rect);
                },
            );
        }
        if matches!(
            self.capture_phase,
            Some(
                CapturePhase::Recording
                    | CapturePhase::RecordingPaused
                    | CapturePhase::RecordingMuting { .. }
            )
        ) && let (Some(flow), Some(snapshot), Some(target)) = (
            &self.flow,
            self.recording_snapshot.clone(),
            self.countdown_target,
        ) {
            let generation = flow.generation();
            let paused = match self.capture_phase {
                Some(CapturePhase::RecordingPaused) => true,
                Some(CapturePhase::RecordingMuting { paused }) => paused,
                _ => false,
            };
            let restart_confirmation = self.recording_restart_confirmation;
            let controls_hidden = self.recording_controls_hidden == Some(generation)
                || self.recording_screenshot_flow.is_some();
            let hide_available = self.recording_restore_available;
            let busy = restart_confirmation
                || matches!(
                    self.capture_phase,
                    Some(CapturePhase::RecordingMuting { .. })
                );
            let elapsed_ms = interpolated_recording_elapsed(
                snapshot.elapsed_ms,
                self.recording_segment_started
                    .map(|started| started.elapsed()),
            );
            let sender = self.selector_tx.clone();
            let confirmation_sender = self.selector_tx.clone();
            let tokens = t.clone();
            let include_controls = self.include_recording_controls;
            let warning = snapshot.warning.clone();
            let position = target.position
                + egui::vec2(
                    (target.size.x - 430.).max(0.) / 2.,
                    (target.size.y - 102.).max(0.) - 20.,
                );
            ctx.show_viewport_deferred(
                egui::ViewportId::from_hash_of("recording-controls"),
                egui::ViewportBuilder::default()
                    .with_title("Captures Recording Controls")
                    .with_inner_size([430., 102.])
                    .with_position(position)
                    .with_transparent(true)
                    .with_decorations(false)
                    .with_always_on_top(),
                move |ui, _| {
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::Visible(!controls_hidden));
                    if !controls_hidden && ui.input(|input| input.viewport().close_requested()) {
                        let _ = sender.send(SelectorMessage::StopRecording { generation });
                        ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
                        return;
                    }
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::ContentProtected(
                            recording_controls_are_excluded(include_controls),
                        ));
                    let notice = warning.as_deref().unwrap_or({
                        if cfg!(target_os = "linux") {
                            "These controls will show in recordings on Linux"
                        } else if include_controls {
                            "These controls will show in recordings"
                        } else {
                            "These controls won’t show in recordings"
                        }
                    });
                    if let Some(action) = recording_hud::show(
                        ui,
                        &tokens,
                        recording_hud::View {
                            paused,
                            busy,
                            has_microphone: snapshot.options.audio.microphone_device_id.is_some(),
                            microphone_muted: snapshot.options.audio.microphone_muted,
                            elapsed_ms,
                            notice,
                            warning: warning.is_some(),
                            hide_available,
                        },
                    ) {
                        let message = match action {
                            recording_hud::Action::Pause => {
                                SelectorMessage::PauseRecording { generation }
                            }
                            recording_hud::Action::Resume => {
                                SelectorMessage::ResumeRecording { generation }
                            }
                            recording_hud::Action::Restart => {
                                SelectorMessage::RestartRecording { generation }
                            }
                            recording_hud::Action::Screenshot => {
                                SelectorMessage::StartRecordingScreenshot { generation }
                            }
                            recording_hud::Action::SetMicrophoneMuted(muted) => {
                                SelectorMessage::SetMicrophoneMuted { generation, muted }
                            }
                            recording_hud::Action::Stop => {
                                SelectorMessage::StopRecording { generation }
                            }
                            recording_hud::Action::Discard => {
                                SelectorMessage::DiscardRecording { generation }
                            }
                            recording_hud::Action::Hide => {
                                SelectorMessage::HideRecordingControls { generation }
                            }
                        };
                        let _ = sender.send(message);
                        ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
                    }
                    if !paused {
                        ui.ctx().request_repaint_after(Duration::from_millis(100));
                    }
                },
            );
            if controls_hidden
                && self
                    .recording_hidden_notice_until
                    .is_some_and(|deadline| deadline > Instant::now())
            {
                let notice_tokens = t.clone();
                ctx.show_viewport_deferred(
                    egui::ViewportId::from_hash_of("recording-controls-hidden"),
                    egui::ViewportBuilder::default()
                        .with_title("Recording controls hidden")
                        .with_inner_size([360., 96.])
                        .with_position(position + egui::vec2(35., 3.))
                        .with_transparent(true)
                        .with_decorations(false)
                        .with_always_on_top()
                        .with_mouse_passthrough(true),
                    move |ui, _| {
                        notice_tokens.glass_controls(ui);
                        egui::Frame::new()
                            .fill(notice_tokens.color("glass-strong"))
                            .stroke(egui::Stroke::new(
                                1.,
                                notice_tokens.color("glass-border"),
                            ))
                            .corner_radius(notice_tokens.number("r-xl") as u8)
                            .inner_margin(egui::Margin::symmetric(20, 14))
                            .show(ui, |ui| {
                                ui.set_width(320.);
                                ui.vertical_centered(|ui| {
                                    ui.strong("Recording controls hidden");
                                    ui.label(
                                        "Open Captures from the tray, reactivate the app, or press New Capture to bring them back.",
                                    );
                                });
                            });
                    },
                );
            }
            if restart_confirmation {
                let tokens = t.clone();
                ctx.show_viewport_deferred(
                    egui::ViewportId::from_hash_of("recording-restart-confirmation"),
                    egui::ViewportBuilder::default()
                        .with_title("Restart recording?")
                        .with_inner_size([360., 150.])
                        .with_position(position + egui::vec2(35., -170.))
                        .with_always_on_top()
                        .with_resizable(false),
                    move |ui, _| {
                        tokens.glass_controls(ui);
                        if ui.input(|input| input.viewport().close_requested()) {
                            let _ = confirmation_sender
                                .send(SelectorMessage::CancelRestartRecording { generation });
                            ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
                            return;
                        }
                        ui.vertical_centered(|ui| {
                            ui.add_space(14.);
                            ui.heading("Restart recording?");
                            ui.label(
                                "The current recording will be deleted and a new countdown will begin.",
                            );
                            ui.add_space(10.);
                            ui.horizontal(|ui| {
                                if ui.button("Cancel").clicked() {
                                    let _ = confirmation_sender.send(
                                        SelectorMessage::CancelRestartRecording { generation },
                                    );
                                }
                                if ui.button("Restart").clicked() {
                                    let _ = confirmation_sender.send(
                                        SelectorMessage::ConfirmRestartRecording { generation },
                                    );
                                }
                            });
                        });
                        ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
                    },
                );
            }
        }
        if self.capture_phase == Some(CapturePhase::ControlsSelecting) {
            let t = t.clone();
            let generation = self
                .flow
                .as_ref()
                .expect("capture controls own flow")
                .generation();
            // Publish selector shortcut scope only in the UI pass that declares
            // the child, never during an earlier hidden-root logic-only pass.
            self.selector_scope_generation
                .store(generation, Ordering::Release);
            let target = self
                .countdown_target
                .expect("capture-controls target validated");
            let controls = Arc::clone(&self.controls);
            let selector_scope_generation = Arc::clone(&self.selector_scope_generation);
            let sender = self.selector_tx.clone();
            let texture = self.window_texture.clone();
            let auto_start = self.controls_auto_start;
            let displays = self.displays.clone();
            let recording_unavailable_reason = (!self.recording_toolchain_ready).then(|| {
                self.recording_toolchain_error
                    .clone()
                    .unwrap_or_else(|| "Checking FFmpeg and ffprobe availability…".to_owned())
            });
            let session = Arc::clone(
                self.window_session
                    .as_ref()
                    .expect("capture controls own window session"),
            );
            ctx.show_viewport_deferred(
                egui::ViewportId::from_hash_of("capture-controls"),
                capture_viewport(
                    "Captures Capture Controls",
                    target.monitor,
                    target.position,
                    target.size,
                ),
                move |ui, _| {
                    if ui.input(|input| input.viewport().close_requested()) {
                        let _ = selector_scope_generation.compare_exchange(
                            generation,
                            0,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        );
                        captures_app::capture_flow::cancel(generation);
                        ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
                        return;
                    }
                    let action = controls.lock().unwrap().show(
                        ui,
                        &t,
                        capture_controls::View {
                            panel_id: egui::Id::unique(("capture-controls-toolbar", generation)),
                            frozen: texture.as_ref(),
                            display: session.display(),
                            displays: &displays,
                            windows: session.windows(),
                            auto_start,
                            recording_available: recording_unavailable_reason.is_none(),
                            recording_unavailable_reason: recording_unavailable_reason.as_deref(),
                        },
                        |point| session.hit_test(point),
                    );
                    if let Some(action) = action {
                        let _ = selector_scope_generation.compare_exchange(
                            generation,
                            0,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        );
                        let message = match action {
                            capture_controls::Action::Capture(target) => {
                                SelectorMessage::ConfirmControls { generation, target }
                            }
                            capture_controls::Action::StartRecording(target) => {
                                SelectorMessage::StartRecording { generation, target }
                            }
                            capture_controls::Action::SwitchDisplay(display_id) => {
                                SelectorMessage::SwitchControlsDisplay {
                                    generation,
                                    display_id,
                                }
                            }
                            capture_controls::Action::Cancel => SelectorMessage::Cancel {
                                generation,
                                kind: SelectorKind::Controls,
                            },
                        };
                        let _ = sender.send(message);
                        ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
                    }
                },
            );
        }
        if self.capture_phase == Some(CapturePhase::RegionSelecting)
            || self.recording_screenshot_phase == Some(RecordingScreenshotPhase::Selecting)
        {
            let t = t.clone();
            let recording_screenshot =
                self.recording_screenshot_phase == Some(RecordingScreenshotPhase::Selecting);
            let generation = if recording_screenshot {
                self.recording_screenshot_flow
                    .as_ref()
                    .expect("recording screenshot selection owns flow")
                    .generation()
            } else {
                self.flow
                    .as_ref()
                    .expect("selection owns flow")
                    .generation()
            };
            let target = self.countdown_target.expect("region target validated");
            let selector = Arc::clone(&self.region_selector);
            let sender = self.selector_tx.clone();
            let (texture, auto_start, display) = if recording_screenshot {
                (
                    self.recording_screenshot_texture.clone(),
                    self.recording_screenshot_settings
                        .as_ref()
                        .is_some_and(|settings| settings.auto_start_on_selection),
                    self.recording_screenshot_session
                        .as_ref()
                        .expect("recording screenshot selection owns region session")
                        .display(),
                )
            } else {
                (
                    self.region_texture.clone(),
                    self.region_auto_start,
                    self.region_session
                        .as_ref()
                        .expect("selection owns region session")
                        .display(),
                )
            };
            let (overlay_width, overlay_height) = display.overlay_size();
            let overlay_bounds = captures_app::selection::Bounds {
                width: overlay_width,
                height: overlay_height,
            };
            let viewport_id = if recording_screenshot {
                egui::ViewportId::from_hash_of(("recording-screenshot-selector", generation))
            } else {
                egui::ViewportId::from_hash_of("region-selector")
            };
            ctx.show_viewport_deferred(
                viewport_id,
                capture_viewport(
                    "Captures Region Selection",
                    target.monitor,
                    target.position,
                    target.size,
                ),
                move |ui, _| {
                    if ui.input(|input| input.viewport().close_requested()) {
                        captures_app::capture_flow::cancel(generation);
                        ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
                        return;
                    }
                    let action = selector.lock().unwrap().show(
                        ui,
                        &t,
                        texture.as_ref(),
                        auto_start,
                        Some(overlay_bounds),
                    );
                    if let Some(action) = action {
                        let message = match action {
                            selector::Action::Confirm => selector
                                .lock()
                                .unwrap()
                                .rect()
                                .map(|rect| SelectorMessage::ConfirmRegion { generation, rect }),
                            selector::Action::Cancel => Some(SelectorMessage::Cancel {
                                generation,
                                kind: SelectorKind::Region,
                            }),
                        };
                        if let Some(message) = message {
                            let _ = sender.send(message);
                            ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
                        }
                    }
                },
            );
        }
        if self.capture_phase == Some(CapturePhase::WindowSelecting) {
            let t = t.clone();
            let generation = self
                .flow
                .as_ref()
                .expect("selection owns flow")
                .generation();
            let target = self.countdown_target.expect("window target validated");
            let selector = Arc::clone(&self.window_selector);
            let sender = self.selector_tx.clone();
            let texture = self.window_texture.clone();
            let auto_start = self.window_auto_start;
            let session = Arc::clone(
                self.window_session
                    .as_ref()
                    .expect("selection owns window session"),
            );
            ctx.show_viewport_deferred(
                egui::ViewportId::from_hash_of("window-selector"),
                capture_viewport(
                    "Captures Window Selection",
                    target.monitor,
                    target.position,
                    target.size,
                ),
                move |ui, _| {
                    if ui.input(|input| input.viewport().close_requested()) {
                        captures_app::capture_flow::cancel(generation);
                        ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
                        return;
                    }
                    let action = selector.lock().unwrap().show(
                        ui,
                        &t,
                        window_selector::View {
                            frozen: texture.as_ref(),
                            display: session.display(),
                            windows: session.windows(),
                            auto_start,
                        },
                        |point| session.hit_test(point),
                    );
                    if let Some(action) = action {
                        let message = match action {
                            window_selector::Action::Confirm(target) => {
                                SelectorMessage::ConfirmWindow { generation, target }
                            }
                            window_selector::Action::Cancel => SelectorMessage::Cancel {
                                generation,
                                kind: SelectorKind::Window,
                            },
                        };
                        let _ = sender.send(message);
                        ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
                    }
                },
            );
        }
        if let Some(flow) = &self.recording_screenshot_flow
            && let Some(RecordingScreenshotPhase::Countdown { .. }) =
                self.recording_screenshot_phase
        {
            let clock = flow.countdown();
            if clock.remaining(Instant::now()) > 0 {
                let t = t.clone();
                let generation = flow.generation();
                let target = self.countdown_target.expect("countdown target validated");
                ctx.show_viewport_deferred(
                    egui::ViewportId::from_hash_of(("recording-screenshot-countdown", generation)),
                    capture_viewport(
                        "Captures Screenshot Countdown",
                        target.monitor,
                        target.position,
                        target.size,
                    ),
                    move |ui, _| {
                        if ui.input(|i| {
                            i.viewport().close_requested() || i.key_pressed(egui::Key::Escape)
                        }) {
                            captures_app::capture_flow::cancel(generation);
                            ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
                        }
                        crate::countdown::show(ui, &t, clock.remaining(Instant::now()).max(1));
                        ui.ctx().request_repaint_after(Duration::from_millis(100));
                    },
                );
            }
        }
        if let Some(flow) = &self.flow
            && !self.capture_waiting_for_hide
            && !self.capture_in_flight
            && matches!(
                self.capture_phase,
                Some(
                    CapturePhase::DisplayCountdown
                        | CapturePhase::RegionCountdown { .. }
                        | CapturePhase::WindowCountdown { .. }
                        | CapturePhase::ControlsCountdown { .. }
                        | CapturePhase::RecordingCountdown
                )
            )
        {
            let clock = flow.countdown();
            if clock.remaining(Instant::now()) > 0 {
                let t = t.clone();
                let generation = flow.generation();
                let target = self.countdown_target.expect("countdown target validated");
                ctx.show_viewport_deferred(
                    egui::ViewportId::from_hash_of("screenshot-countdown"),
                    capture_viewport(
                        if self.capture_phase == Some(CapturePhase::RecordingCountdown) {
                            "Captures Recording Countdown"
                        } else {
                            "Captures Screenshot Countdown"
                        },
                        target.monitor,
                        target.position,
                        target.size,
                    ),
                    move |ui, _| {
                        if ui.input(|i| {
                            i.viewport().close_requested() || i.key_pressed(egui::Key::Escape)
                        }) {
                            captures_app::capture_flow::cancel(generation);
                            ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
                        }
                        crate::countdown::show(ui, &t, clock.remaining(Instant::now()).max(1));
                        ui.ctx().request_repaint_after(Duration::from_millis(100));
                    },
                );
            } else {
                // Stop declaring the child before hiding the root. Hidden-root
                // logic then verifies visibility and waits for compositor settling.
                self.hide_for_capture(ctx);
            }
        }
    }

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        t: &Tokens,
        frame: &eframe::Frame,
        settings: impl Fn() -> Result<AppSettings, String>,
    ) {
        if self.flow.is_some() || self.capture_in_flight {
            ui.disable();
        }
        egui::Panel::top("live-header").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Captures");
                ui.label(
                    RichText::new("Native screenshot capture").color(t.color("text-muted")),
                );
            });
            ui.label("Display, region and window capture with countdown, cursor inclusion, automatic copy, save format/folder preferences, and one latest mini preview. Recording and editing are not connected yet.");
            ui.horizontal(|ui| {
                egui::ComboBox::from_label("Display")
                    .selected_text(self.displays.iter().find(|d| Some(&d.id) == self.display_id.as_ref()).map_or("No display", |d| d.name.as_str()))
                    .show_ui(ui, |ui| for display in &self.displays {
                        ui.selectable_value(&mut self.display_id, Some(display.id.clone()), format!("{} — {}×{}{}", display.name, display.width, display.height, if display.is_primary { " (Primary)" } else { "" }));
                    });
                if ui.button("Refresh displays").clicked() { self.send(Request::Displays); }
                let can_start_capture = self.can_start_capture();
                if ui.add_enabled(can_start_capture, egui::Button::new("New Capture")).clicked() {
                    self.request_capture(CaptureRequest::NewCapture);
                    self.launch_requested_capture(ui.ctx(), frame, settings());
                }
                if ui.add_enabled(can_start_capture, egui::Button::new("Capture display")).clicked() {
                    self.request_capture(CaptureRequest::Display);
                    self.launch_requested_capture(ui.ctx(), frame, settings());
                }
                if ui.add_enabled(can_start_capture, egui::Button::new("Capture region")).clicked() {
                    self.request_capture(CaptureRequest::Region);
                    self.launch_requested_capture(ui.ctx(), frame, settings());
                }
                if ui.add_enabled(can_start_capture, egui::Button::new("Capture window")).clicked() {
                    self.request_capture(CaptureRequest::Window);
                    self.launch_requested_capture(ui.ctx(), frame, settings());
                }
                if ui.button("Request permission").clicked() { self.send(Request::RequestPermission); }
            });
            if self.can_hide == Some(false) {
                ui.colored_label(t.color("theme-signal"), "Display, region and window capture unavailable: this Wayland backend cannot hide and verify the root window.");
            }
            if let Some(error) = &self.error { ui.colored_label(t.color("theme-signal"), error); }
            else { ui.label(RichText::new(&self.status).small().color(t.color("text-muted"))); }
        });

        egui::Panel::left("live-history")
            .resizable(true)
            .default_size(270.)
            .min_size(220.)
            .show(ui, |ui| {
                ui.heading("History");
                ui.label(
                    RichText::new(self.root.display().to_string())
                        .small()
                        .color(t.color("text-muted")),
                );
                if ui
                    .add_enabled(
                        self.pending == 0 && !self.artifacts.is_empty(),
                        egui::Button::new("Clear history…"),
                    )
                    .clicked()
                {
                    self.confirm_clear_history = true;
                    self.confirm_delete = None;
                }
                if self.confirm_clear_history {
                    ui.group(|ui| {
                        ui.label(
                            "Delete all screenshots from history? Exported files stay on disk.",
                        );
                        ui.horizontal(|ui| {
                            if ui.button("Cancel").clicked()
                                || ui.input_mut(|input| {
                                    input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)
                                })
                            {
                                self.confirm_clear_history = false;
                            }
                            if ui
                                .add_enabled(
                                    self.pending == 0,
                                    egui::Button::new(
                                        RichText::new("Delete all").color(t.color("theme-signal")),
                                    ),
                                )
                                .clicked()
                            {
                                self.confirm_clear_history = false;
                                self.send(Request::ClearHistory {
                                    root: self.root.clone(),
                                });
                            }
                        });
                    });
                }
                if self.artifacts.is_empty() {
                    ui.label("No captures yet");
                }
                egui::ScrollArea::vertical().show_rows(
                    ui,
                    40.,
                    self.artifacts.len(),
                    |ui, rows| {
                        for row in rows {
                            let item = &self.artifacts[row].entry;
                            let id = item.id.clone();
                            let date = chrono::DateTime::parse_from_rfc3339(&item.created_at)
                                .map(|date| {
                                    date.with_timezone(&chrono::Local)
                                        .format("%b %d, %H:%M")
                                        .to_string()
                                })
                                .unwrap_or_else(|_| item.created_at.clone());
                            if ui
                                .add_sized(
                                    [ui.available_width(), 40.],
                                    egui::Button::new(format!(
                                        "{} · {}×{}\n{}",
                                        match item.kind {
                                            captures_history::ArtifactKind::Screenshot =>
                                                "Screenshot",
                                            captures_history::ArtifactKind::Video => "Video",
                                            captures_history::ArtifactKind::Gif => "GIF",
                                        },
                                        item.width,
                                        item.height,
                                        date
                                    ))
                                    .wrap_mode(egui::TextWrapMode::Extend)
                                    .selected(self.selection.id.as_deref() == Some(&id)),
                                )
                                .clicked()
                            {
                                self.select(id);
                            }
                        }
                    },
                );
            });

        egui::CentralPanel::default().show(ui, |ui| {
            let selected = self.selection.id.clone();
            let selected_entry = selected.as_ref().and_then(|id| {
                self.artifacts
                    .iter()
                    .find(|artifact| &artifact.entry.id == id)
                    .map(|artifact| artifact.entry.clone())
            });
            let selected_is_screenshot = selected_entry
                .as_ref()
                .is_some_and(|entry| entry.kind == captures_history::ArtifactKind::Screenshot);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        selected_is_screenshot
                            && self.decoded_path.is_some()
                            && self.pending == 0,
                        egui::Button::new("Copy pixels"),
                    )
                    .clicked()
                {
                    self.copy();
                }
                if ui
                    .add_enabled(
                        selected_is_screenshot && self.pending == 0,
                        egui::Button::new("Save image"),
                    )
                    .clicked()
                    && let Some(id) = selected.clone()
                {
                    match settings() {
                        Ok(settings) => self.send(Request::SaveScreenshot {
                            root: self.root.clone(),
                            id,
                            directory: settings.output_directory.into(),
                            format: settings.screenshot_format,
                        }),
                        Err(error) => self.error = Some(error),
                    }
                }
                if ui
                    .add_enabled(
                        selected.is_some() && self.pending == 0,
                        egui::Button::new("Delete from history…"),
                    )
                    .clicked()
                {
                    self.confirm_clear_history = false;
                    self.confirm_delete = selected.clone();
                }
                let saved = self
                    .artifacts
                    .iter()
                    .find(|a| Some(&a.entry.id) == selected.as_ref())
                    .and_then(|a| a.entry.saved_path.as_deref());
                if ui
                    .add_enabled(saved.is_some(), egui::Button::new("Reveal export"))
                    .clicked()
                    && let Some(path) = saved
                    && let Err(error) = reveal(Path::new(path))
                {
                    self.error = Some(format!("Could not reveal export: {error}"));
                }
            });
            if let Some(entry) = selected_entry.filter(|entry| entry.kind.is_recording()) {
                ui.label(
                    RichText::new(format!(
                        "{} · {}×{} · {}",
                        if entry.kind == captures_history::ArtifactKind::Video {
                            "H.264 MP4 recording"
                        } else {
                            "GIF recording"
                        },
                        entry.width,
                        entry.height,
                        format_duration(entry.duration_ms.unwrap_or_default())
                    ))
                    .color(t.color("text-muted")),
                );
                ui.label("The native recording editor is not connected yet. History is showing the saved poster frame.");
            }
            if let Some(id) = self.confirm_delete.clone() {
                ui.group(|ui| {
                    ui.label("Delete this capture from history? Exported files are never deleted.");
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            self.confirm_delete = None;
                        }
                        if ui.button("Delete capture").clicked() {
                            self.send(Request::Delete {
                                root: self.root.clone(),
                                id,
                            });
                        }
                    });
                });
            }
            ui.add_space(t.number("s-4"));
            if let Some(texture) = &self.texture {
                ui.add(
                    egui::Image::new(texture)
                        .fit_to_exact_size(ui.available_size())
                        .maintain_aspect_ratio(true),
                );
            } else if self.preview_loading {
                ui.spinner();
            } else if selected.is_some() {
                ui.label("Preview unavailable. Select another capture or retry this one.");
            } else {
                ui.centered_and_justified(|ui| {
                    ui.label("Capture a display or select a history item to preview it.");
                });
            }
        });
    }

    fn copy(&mut self) {
        if let Some(path) = &self.decoded_path {
            self.pending += 1;
            self.error = None;
            let _ = self.tx.send(Job::Copy {
                path: path.clone(),
                preview: None,
            });
        }
    }
}

fn recording_controls_hidden(
    hidden_generation: Option<u64>,
    active_generation: Option<u64>,
) -> bool {
    hidden_generation.is_some() && hidden_generation == active_generation
}

fn capture_target(
    frame: &eframe::Frame,
    displays: &[DisplayDescriptor],
    display_id: Option<&str>,
) -> Option<CaptureTarget> {
    let display = displays
        .iter()
        .find(|display| Some(display.id.as_str()) == display_id)?;
    let window = frame.winit_window()?;
    let (index, monitor) = window
        .available_monitors()
        .enumerate()
        .find(|(_, monitor)| {
            let position = monitor.position();
            let size = monitor.size();
            monitor_matches_overlay(
                display.overlay_geometry(),
                (position.x, position.y, size.width, size.height),
                monitor.scale_factor(),
                display.scale_factor,
            )
        })?;
    let scale = monitor.scale_factor();
    let position = monitor.position().to_logical::<f32>(scale);
    let size = monitor.size().to_logical::<f32>(scale);
    let physical_position = monitor.position();
    let physical_size = monitor.size();
    Some(CaptureTarget {
        monitor: index,
        position: egui::pos2(position.x, position.y),
        size: egui::vec2(size.width, size.height),
        preview_bounds: preview_bounds(
            physical_position.x,
            physical_position.y,
            physical_size.width,
            physical_size.height,
            scale,
        ),
    })
}

fn preview_bounds(
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    scale_factor: f64,
) -> Option<captures_app::preview::ThumbnailMonitorBounds> {
    let full = crate::work_area::PhysicalRect {
        x,
        y,
        width,
        height,
    };
    let work = crate::work_area::for_monitor(full)?;
    Some(captures_app::preview::ThumbnailMonitorBounds {
        work_x: work.x,
        work_y: work.y,
        work_width: work.width,
        work_height: work.height,
        full_x: x,
        full_y: y,
        full_width: width,
        full_height: height,
        scale_factor,
    })
}

fn monitor_matches_overlay(
    overlay: (f64, f64, f64, f64),
    physical: (i32, i32, u32, u32),
    winit_scale: f64,
    capture_scale: f64,
) -> bool {
    let scale = winit_scale.max(1.);
    let actual = (
        f64::from(physical.0) / scale,
        f64::from(physical.1) / scale,
        f64::from(physical.2) / scale,
        f64::from(physical.3) / scale,
    );
    (winit_scale - capture_scale).abs() < 0.001
        && [actual.0, actual.1, actual.2, actual.3]
            .into_iter()
            .zip([overlay.0, overlay.1, overlay.2, overlay.3])
            .all(|(actual, expected)| (actual - expected).abs() <= 1.)
}

fn capture_viewport(
    title: &str,
    monitor: usize,
    position: egui::Pos2,
    size: egui::Vec2,
) -> egui::ViewportBuilder {
    egui::ViewportBuilder::default()
        .with_title(title)
        .with_visible(true)
        .with_monitor(monitor)
        .with_position(position)
        .with_inner_size(size)
        .with_fullscreen(true)
        .with_decorations(false)
        .with_resizable(false)
        .with_transparent(true)
        .with_has_shadow(false)
        .with_always_on_top()
        .with_taskbar(false)
}

fn restore_root_for_history(ctx: &egui::Context) {
    // The root may not run Live::logic while minimized. Target it directly
    // from the independently repainting preview before queueing selection.
    ctx.send_viewport_cmd_to(
        egui::ViewportId::ROOT,
        egui::ViewportCommand::Minimized(false),
    );
    ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Visible(true));
}

fn same_display_geometry(left: &DisplayDescriptor, right: &DisplayDescriptor) -> bool {
    left.id == right.id
        && left.x == right.x
        && left.y == right.y
        && left.width == right.width
        && left.height == right.height
        && left.scale_factor == right.scale_factor
}

fn active_selector_generation(
    scoped_generation: u64,
    phase: Option<CapturePhase>,
    flow_generation: Option<u64>,
    flow_is_current: bool,
) -> Option<u64> {
    (scoped_generation != 0
        && phase == Some(CapturePhase::ControlsSelecting)
        && flow_generation == Some(scoped_generation)
        && flow_is_current)
        .then_some(scoped_generation)
}

fn accepts_prepare_reply(
    active_generation: Option<u64>,
    reply_generation: u64,
    flow_is_current: bool,
    phase: Option<CapturePhase>,
    expected_phase: CapturePhase,
) -> bool {
    active_generation == Some(reply_generation) && flow_is_current && phase == Some(expected_phase)
}

fn accepts_selector_action(
    active_generation: Option<u64>,
    action_generation: u64,
    flow_is_current: bool,
    phase: Option<CapturePhase>,
    kind: SelectorKind,
) -> bool {
    active_generation == Some(action_generation)
        && flow_is_current
        && phase
            == Some(match kind {
                SelectorKind::Region => CapturePhase::RegionSelecting,
                SelectorKind::Window => CapturePhase::WindowSelecting,
                SelectorKind::Controls => CapturePhase::ControlsSelecting,
            })
}

fn accepts_recording_event(
    active_generation: Option<u64>,
    event_generation: u64,
    flow_current: bool,
    phase: Option<CapturePhase>,
    accepts_phase: impl FnOnce(CapturePhase) -> bool,
) -> bool {
    active_generation == Some(event_generation) && flow_current && phase.is_some_and(accepts_phase)
}

fn recording_options(
    selection: &capture_controls::RecordingSelection,
    target: capture_controls::Target,
    display: &DisplayDescriptor,
    session: Option<&WindowSession>,
) -> Result<RecordingOptions, String> {
    let capabilities = RecordingCapabilities::current(false);
    let target = match target {
        capture_controls::Target::Display => RecordingTarget::Display {
            display_id: display.id.clone(),
        },
        capture_controls::Target::Region(rect) => RecordingTarget::Region {
            display_id: display.id.clone(),
            rect: recording_rect(rect, display)?,
        },
        capture_controls::Target::Window(index) => {
            let window_id = session
                .and_then(|session| session.windows().get(index))
                .map(|window| window.id.clone())
                .ok_or_else(|| "The selected window changed before recording.".to_owned())?;
            RecordingTarget::Window { window_id }
        }
    };
    let options = RecordingOptions {
        kind: RecordingKind::Video,
        target,
        frames_per_second: selection.frames_per_second,
        max_resolution: selection.max_resolution,
        countdown_seconds: selection.countdown_seconds,
        show_cursor: capabilities.cursor_control && selection.show_cursor,
        highlight_clicks: capabilities.click_highlights && selection.highlight_clicks,
        // Keystroke rendering is not implemented by either native engine yet.
        show_keystrokes: false,
        audio: AudioOptions {
            capture_system_audio: capabilities.system_audio && selection.capture_system_audio,
            microphone_device_id: capabilities
                .microphone
                .then(|| selection.microphone_device_id.clone())
                .flatten(),
            mono_output: selection.mono_audio,
            ..AudioOptions::default()
        },
        gif: GifOptions::default(),
    };
    options.validate().map_err(str::to_owned)?;
    Ok(options)
}

fn recording_rect(rect: LogicalRect, display: &DisplayDescriptor) -> Result<CaptureRect, String> {
    let (overlay_width, overlay_height) = display.overlay_size();
    let x = rect.x.round().clamp(0., overlay_width) as i32;
    let y = rect.y.round().clamp(0., overlay_height) as i32;
    let width = rect
        .width
        .round()
        .clamp(0., (overlay_width - f64::from(x)).max(0.)) as u32;
    let height = rect
        .height
        .round()
        .clamp(0., (overlay_height - f64::from(y)).max(0.)) as u32;
    let rect = CaptureRect {
        x,
        y,
        width,
        height,
    };
    rect.is_valid()
        .then_some(rect)
        .ok_or_else(|| "Select a valid recording region.".to_owned())
}

fn recording_recovery_root(history_root: &Path) -> PathBuf {
    history_root.with_file_name("recording-recovery")
}

fn is_recording_phase(phase: Option<CapturePhase>) -> bool {
    matches!(
        phase,
        Some(
            CapturePhase::RecordingPreparing { .. }
                | CapturePhase::RecordingCountdown
                | CapturePhase::RecordingStarting
                | CapturePhase::Recording
                | CapturePhase::RecordingPausing
                | CapturePhase::RecordingPaused
                | CapturePhase::RecordingMuting { .. }
                | CapturePhase::RecordingRestarting
                | CapturePhase::RecordingFinalizing
                | CapturePhase::RecordingDiscarding
        )
    )
}

fn format_duration(elapsed_ms: u64) -> String {
    let elapsed_seconds = elapsed_ms / 1_000;
    format!("{}:{:02}", elapsed_seconds / 60, elapsed_seconds % 60)
}

fn snapshot_interpolation_origin(state: RecordingState, now: Instant) -> Option<Instant> {
    (state == RecordingState::Recording).then_some(now)
}

fn interpolated_recording_elapsed(
    snapshot_elapsed_ms: u64,
    since_snapshot: Option<Duration>,
) -> u64 {
    snapshot_elapsed_ms.saturating_add(since_snapshot.map_or(0, |elapsed| {
        u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
    }))
}

fn load_history(root: &Path) -> Result<Vec<Artifact>, String> {
    captures_history::load(root, chrono::Utc::now())
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|entry| {
            let directory = captures_history::entry_directory(root, &entry.id)
                .map_err(|error| error.to_string())?;
            let preview_path = directory.join(captures_history::HISTORY_PREVIEW_FILE);
            let image_path = if entry.kind == captures_history::ArtifactKind::Screenshot {
                directory.join(captures_history::HISTORY_IMAGE_FILE)
            } else {
                // Native recording editing is not connected yet. The History
                // canvas renders the persisted poster and never decodes media.mp4.
                preview_path.clone()
            };
            Ok(Artifact {
                entry,
                image_path,
                preview_path,
            })
        })
        .collect()
}

fn decode(path: &Path) -> Result<Decoded, String> {
    let rgba = image::open(path)
        .map_err(|error| format!("Could not decode {}: {error}", path.display()))?
        .into_rgba8();
    let (width, height) = rgba.dimensions();
    let bytes = rgba.into_raw();
    Ok(Decoded {
        image: egui::ColorImage::from_rgba_unmultiplied([width as usize, height as usize], &bytes),
    })
}

fn copy_image(path: &Path, clipboard: &mut Option<arboard::Clipboard>) -> Result<(), String> {
    let image = image::open(path).map_err(|e| e.to_string())?.into_rgba8();
    if clipboard.is_none() {
        *clipboard = Some(arboard::Clipboard::new().map_err(|e| e.to_string())?);
    }
    clipboard
        .as_mut()
        .expect("clipboard initialized")
        .set_image(arboard::ImageData {
            width: image.width() as usize,
            height: image.height() as usize,
            bytes: Cow::Owned(image.into_raw()),
        })
        .map_err(|e| format!("Could not copy image: {e}"))
}

fn reveal(path: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "windows")]
    let result = Command::new("explorer")
        .arg(format!("/select,{}", path.display()))
        .spawn();
    #[cfg(target_os = "linux")]
    let result = Command::new("xdg-open")
        .arg(path.parent().unwrap_or(path))
        .spawn();
    result.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preview_target() -> CaptureTarget {
        CaptureTarget {
            monitor: 0,
            position: egui::Pos2::ZERO,
            size: egui::vec2(1280., 720.),
            preview_bounds: Some(captures_app::preview::ThumbnailMonitorBounds {
                work_x: 0,
                work_y: 0,
                work_width: 1280,
                work_height: 720,
                full_x: 0,
                full_y: 0,
                full_width: 1280,
                full_height: 720,
                scale_factor: 1.,
            }),
        }
    }

    fn preview_artifact(root: &Path, color: [u8; 4]) -> Artifact {
        captures_app::persist_screenshot(
            root,
            &image::RgbaImage::from_pixel(9, 5, image::Rgba(color)),
            captures_capture::CaptureMode::Display,
        )
        .unwrap()
    }

    #[test]
    fn shutdown_drains_accepted_exports() {
        let root = tempfile::tempdir().unwrap();
        let exports = tempfile::tempdir().unwrap();
        let pixels = image::RgbaImage::from_pixel(7, 3, image::Rgba([21, 96, 177, 255]));
        let artifact = captures_app::persist_screenshot(
            root.path(),
            &pixels,
            captures_capture::CaptureMode::Display,
        )
        .unwrap();
        let mut live = Live::new(egui::Context::default(), Some(root.path().into()));
        live.send(Request::SaveScreenshot {
            root: root.path().into(),
            id: artifact.entry.id,
            directory: exports.path().into(),
            format: captures_settings::ScreenshotFormat::Png,
        });
        live.flush();
        let entries = captures_app::list(root.path()).unwrap();
        let saved = entries[0].entry.saved_path.as_ref().unwrap();
        assert_eq!(image::open(saved).unwrap().into_rgba8(), pixels);
        live.flush();
    }

    #[test]
    fn stale_decode_is_rejected_after_selection_changes() {
        let mut selection = Selection::default();
        let old = selection.begin("old".into());
        let current = selection.begin("current".into());
        assert!(!selection.accepts(old));
        assert!(selection.accepts(current));
        assert_eq!(selection.id.as_deref(), Some("current"));
        selection.clear();
        assert!(!selection.accepts(current));
        assert_eq!(selection.id, None);
    }

    #[test]
    fn history_preview_action_restores_minimized_root_without_waiting_for_live_logic() {
        let ctx = egui::Context::default();
        ctx.begin_pass(Default::default());
        restore_root_for_history(&ctx);
        let mut output = ctx.end_pass();
        let commands = &output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .expect("root viewport output")
            .commands;

        assert!(
            commands
                .iter()
                .any(|command| matches!(command, egui::ViewportCommand::Minimized(false)))
        );
        assert!(
            commands
                .iter()
                .any(|command| matches!(command, egui::ViewportCommand::Visible(true)))
        );
        output.textures_delta.clear();
    }

    #[test]
    fn hidden_root_bootstrap_requests_one_ui_pass_without_showing_root() {
        let ctx = egui::Context::default();
        let first = ctx.run_logic(&egui::RawInput::default(), request_hidden_root_paint);
        let commands = first
            .viewport_commands
            .get(&egui::ViewportId::ROOT)
            .expect("hidden-paint command targets root");
        assert_eq!(commands, &[egui::ViewportCommand::RequestPaintWhileHidden]);
        assert!(
            !commands
                .iter()
                .any(|command| matches!(command, egui::ViewportCommand::Visible(_)))
        );

        let next = ctx.run_logic(&egui::RawInput::default(), |_| {});
        assert!(next.viewport_commands.is_empty());
    }

    #[test]
    fn recording_worker_replies_require_current_generation_and_expected_phase() {
        let starting = |phase| phase == CapturePhase::RecordingStarting;
        assert!(accepts_recording_event(
            Some(42),
            42,
            true,
            Some(CapturePhase::RecordingStarting),
            starting,
        ));
        assert!(!accepts_recording_event(
            Some(43),
            42,
            true,
            Some(CapturePhase::RecordingStarting),
            starting,
        ));
        assert!(!accepts_recording_event(
            Some(42),
            42,
            false,
            Some(CapturePhase::RecordingStarting),
            starting,
        ));
        assert!(!accepts_recording_event(
            Some(42),
            42,
            true,
            Some(CapturePhase::RecordingDiscarding),
            starting,
        ));
    }

    #[test]
    fn running_snapshot_restarts_elapsed_interpolation_without_double_counting() {
        let accepted_at = Instant::now();
        assert_eq!(
            snapshot_interpolation_origin(RecordingState::Recording, accepted_at),
            Some(accepted_at)
        );
        assert_eq!(
            snapshot_interpolation_origin(RecordingState::Paused, accepted_at),
            None
        );
        assert_eq!(
            interpolated_recording_elapsed(37_000, Some(Duration::from_millis(1_234))),
            38_234
        );
        assert_eq!(interpolated_recording_elapsed(37_000, None), 37_000);
    }

    #[test]
    fn recording_options_are_video_and_preserve_the_selected_target_and_settings() {
        let display = DisplayDescriptor {
            id: "display".into(),
            name: "Fixture".into(),
            x: 0,
            y: 0,
            width: 1000,
            height: 720,
            scale_factor: 1.,
            is_primary: true,
        };
        let selection = capture_controls::RecordingSelection {
            frames_per_second: 30,
            max_resolution: captures_recording::MaxResolution::P720,
            countdown_seconds: 2,
            show_cursor: false,
            highlight_clicks: false,
            capture_system_audio: false,
            microphone_device_id: None,
            mono_audio: true,
        };
        let options = recording_options(
            &selection,
            capture_controls::Target::Region(LogicalRect {
                x: 10.4,
                y: 20.6,
                width: 300.2,
                height: 160.8,
            }),
            &display,
            None,
        )
        .unwrap();

        assert_eq!(options.kind, RecordingKind::Video);
        assert_eq!(options.frames_per_second, 30);
        assert_eq!(
            options.max_resolution,
            captures_recording::MaxResolution::P720
        );
        assert_eq!(options.countdown_seconds, 2);
        assert!(options.audio.mono_output);
        assert_eq!(
            options.target,
            RecordingTarget::Region {
                display_id: "display".into(),
                rect: CaptureRect {
                    x: 10,
                    y: 21,
                    width: 300,
                    height: 161,
                },
            }
        );
    }

    #[test]
    fn recording_region_stays_display_local_logical_on_scaled_negative_origin_display() {
        let display = DisplayDescriptor {
            id: "retina-left".into(),
            name: "Retina left".into(),
            x: -1440,
            y: 0,
            width: 2880,
            height: 1800,
            scale_factor: 2.,
            is_primary: false,
        };

        assert_eq!(
            recording_rect(
                LogicalRect {
                    x: 100.,
                    y: 50.,
                    width: 800.,
                    height: 450.,
                },
                &display,
            )
            .unwrap(),
            CaptureRect {
                x: 100,
                y: 50,
                width: 800,
                height: 450,
            }
        );
    }

    #[test]
    fn recording_history_uses_poster_without_decoding_media() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source.mp4");
        std::fs::write(&source, b"not image pixels").unwrap();
        let poster = captures_history::encode_png(&image::RgbaImage::from_pixel(
            4,
            3,
            image::Rgba([18, 92, 173, 255]),
        ))
        .unwrap();
        let entry = captures_history::HistoryEntry {
            id: "67e55044-10b1-426f-9247-bb680e5fe0c8".into(),
            kind: captures_history::ArtifactKind::Video,
            preview_url: "poster".into(),
            full_url: "media".into(),
            width: 4,
            height: 3,
            size_bytes: 16,
            created_at: chrono::Utc::now().to_rfc3339(),
            mode: None,
            saved_path: None,
            mime_type: Some("video/mp4".into()),
            duration_ms: Some(1_200),
            target: Some(RecordingTarget::Display {
                display_id: "display".into(),
            }),
            has_system_audio: false,
            has_microphone_audio: false,
            dropped_frames: 0,
        };
        captures_history::save_recording(root.path(), &entry, &poster, &source).unwrap();

        let artifacts = load_history(root.path()).unwrap();
        assert_eq!(artifacts.len(), 1);
        assert_eq!(
            artifacts[0].entry.kind,
            captures_history::ArtifactKind::Video
        );
        assert_eq!(artifacts[0].image_path, artifacts[0].preview_path);
        assert_eq!(decode(&artifacts[0].image_path).unwrap().image.size, [4, 3]);
    }

    #[test]
    fn external_capture_requests_are_single_flight() {
        let root = tempfile::tempdir().unwrap();
        let mut live = Live::new(egui::Context::default(), Some(root.path().into()));
        live.pending = 0;

        live.request_capture(CaptureRequest::Region);
        assert_eq!(live.requested_capture, Some(CaptureRequest::Region));
        live.request_capture(CaptureRequest::Window);
        assert_eq!(live.requested_capture, Some(CaptureRequest::Region));
        assert_eq!(
            live.error.as_deref(),
            Some("Another capture or history action is still in progress.")
        );
        live.flush();
    }

    #[test]
    fn capture_viewports_do_not_depend_on_workspace_or_preview_rendering() {
        let root = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        ctx.set_embed_viewports(false);
        let mut live = Live::new(ctx.clone(), Some(root.path().into()));
        live.flow = Some(CaptureFlow::begin(5).unwrap());
        live.capture_phase = Some(CapturePhase::DisplayCountdown);
        live.countdown_target = Some(CaptureTarget {
            monitor: 0,
            position: egui::pos2(0., 0.),
            size: egui::vec2(800., 600.),
            preview_bounds: None,
        });
        let tokens = crate::tokens::load()["dark-mustard"].clone();

        ctx.begin_pass(Default::default());
        live.viewports(&ctx, &tokens, Ok(AppSettings::default()));
        let mut output = ctx.end_pass();

        let declared = output
            .viewport_output
            .contains_key(&egui::ViewportId::from_hash_of("screenshot-countdown"));
        output.textures_delta.clear();
        assert!(declared);
        live.flush();
    }

    #[test]
    fn capture_completion_restores_the_root_visibility_it_started_with() {
        let root = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut live = Live::new(ctx.clone(), Some(root.path().into()));
        live.restore_root_visible = false;

        ctx.begin_pass(Default::default());
        live.finish_capture(&ctx, false);
        let mut output = ctx.end_pass();
        let commands = &output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .expect("root viewport output")
            .commands;
        assert!(commands.contains(&egui::ViewportCommand::Visible(false)));
        assert!(!commands.contains(&egui::ViewportCommand::Visible(true)));
        output.textures_delta.clear();
        live.flush();
    }

    #[test]
    fn mini_preview_cancel_restores_visibility_without_touching_history() {
        let root = tempfile::tempdir().unwrap();
        let artifact = preview_artifact(root.path(), [11, 22, 33, 255]);
        let mut previews = MiniPreviews::default();
        previews
            .begin_capture(&AppSettings::default(), Some(preview_target()), 4)
            .unwrap();
        assert!(previews.visibility.is_suppressed());

        previews.restore_capture();
        assert!(!previews.visibility.is_suppressed());
        assert!(previews.cards.is_empty());
        assert_eq!(captures_app::list(root.path()).unwrap().len(), 1);
        assert!(artifact.image_path.exists());
    }

    #[test]
    fn persisted_order_and_per_id_guards_survive_out_of_order_decode() {
        let root = tempfile::tempdir().unwrap();
        let first = preview_artifact(root.path(), [10, 20, 30, 255]);
        let second = preview_artifact(root.path(), [90, 80, 70, 255]);
        let settings = AppSettings::default();
        let mut previews = MiniPreviews::default();

        previews
            .begin_capture(&settings, Some(preview_target()), 1)
            .unwrap();
        let (first_guard, _) = previews.start_artifact(&first).unwrap().unwrap();
        previews
            .begin_capture(&settings, Some(preview_target()), 2)
            .unwrap();
        let (second_guard, _) = previews.start_artifact(&second).unwrap().unwrap();

        assert_eq!(
            previews.stack.ids(),
            [first.entry.id.clone(), second.entry.id.clone()]
        );
        previews.mark_ready(&second_guard.artifact_id);
        assert!(previews.accepts(&first_guard.artifact_id, first_guard.generation));
        assert!(previews.accepts(&second_guard.artifact_id, second_guard.generation));
        assert!(!previews.dismiss(&first_guard.artifact_id, second_guard.generation));
        assert!(previews.dismiss(&first_guard.artifact_id, first_guard.generation));
        assert!(!previews.accepts(&first_guard.artifact_id, first_guard.generation));
        assert!(previews.accepts(&second_guard.artifact_id, second_guard.generation));
        assert_eq!(captures_app::list(root.path()).unwrap().len(), 2);
        assert!(first.image_path.exists());
        assert!(second.image_path.exists());
    }

    #[test]
    fn incoming_capture_preserves_collapse_and_clear_snapshot_spares_later_arrival() {
        let root = tempfile::tempdir().unwrap();
        let first = preview_artifact(root.path(), [10, 10, 10, 255]);
        let second = preview_artifact(root.path(), [20, 20, 20, 255]);
        let later = preview_artifact(root.path(), [30, 30, 30, 255]);
        let settings = AppSettings::default();
        let mut previews = MiniPreviews::default();

        previews
            .begin_capture(&settings, Some(preview_target()), 1)
            .unwrap();
        let (first_guard, _) = previews.start_artifact(&first).unwrap().unwrap();
        previews.mark_ready(&first_guard.artifact_id);
        previews.stack.set_collapsed(true);
        previews
            .begin_capture(&settings, Some(preview_target()), 2)
            .unwrap();
        let (second_guard, _) = previews.start_artifact(&second).unwrap().unwrap();
        previews.mark_ready(&second_guard.artifact_id);
        assert!(previews.stack.is_collapsed());
        let clear_snapshot = previews.stack.ids().to_vec();

        previews
            .begin_capture(&settings, Some(preview_target()), 3)
            .unwrap();
        let (later_guard, _) = previews.start_artifact(&later).unwrap().unwrap();
        assert_eq!(previews.clear(&clear_snapshot), 2);

        assert_eq!(previews.stack.ids(), std::slice::from_ref(&later.entry.id));
        assert!(previews.accepts(&later_guard.artifact_id, later_guard.generation));
        assert_eq!(captures_app::list(root.path()).unwrap().len(), 3);
        assert!(first.image_path.exists());
        assert!(second.image_path.exists());
        assert!(later.image_path.exists());
    }

    #[test]
    fn clearing_pending_card_rejects_its_late_decode() {
        let root = tempfile::tempdir().unwrap();
        let artifact = preview_artifact(root.path(), [44, 55, 66, 255]);
        let mut previews = MiniPreviews::default();
        previews
            .begin_capture(&AppSettings::default(), Some(preview_target()), 1)
            .unwrap();
        let (guard, _) = previews.start_artifact(&artifact).unwrap().unwrap();

        assert_eq!(previews.clear(std::slice::from_ref(&guard.artifact_id)), 1);
        assert!(!previews.accepts(&guard.artifact_id, guard.generation));
        assert!(!previews.visibility.is_suppressed());
        assert!(artifact.image_path.exists());
    }

    #[test]
    fn disabled_mini_previews_retain_card_for_reenable_without_showing_it() {
        let root = tempfile::tempdir().unwrap();
        let artifact = preview_artifact(root.path(), [40, 50, 60, 255]);
        let settings = AppSettings {
            show_mini_previews: false,
            ..AppSettings::default()
        };
        let mut previews = MiniPreviews::default();
        previews
            .begin_capture(&settings, Some(preview_target()), 1)
            .unwrap();

        let (guard, _) = previews.start_artifact(&artifact).unwrap().unwrap();
        assert_eq!(
            previews.stack.ids(),
            std::slice::from_ref(&artifact.entry.id)
        );
        assert!(previews.accepts(&guard.artifact_id, guard.generation));
        assert!(!previews.is_visible());
        previews.mark_ready(&guard.artifact_id);
        assert!(!previews.visibility.is_suppressed());
        previews.show = true;
        assert!(!previews.is_visible(), "decode still gates presentation");
        let context = egui::Context::default();
        let texture = context.load_texture(
            "disabled-preview-test",
            egui::ColorImage::new([1, 1], vec![egui::Color32::WHITE]),
            egui::TextureOptions::LINEAR,
        );
        previews.cards.get_mut(&guard.artifact_id).unwrap().texture = Some(texture);
        assert!(previews.is_visible());
        assert_eq!(captures_app::list(root.path()).unwrap().len(), 1);
    }

    #[test]
    fn unavailable_work_area_skips_preview_without_losing_capture() {
        let root = tempfile::tempdir().unwrap();
        let artifact = preview_artifact(root.path(), [60, 50, 40, 255]);
        let mut target = preview_target();
        target.preview_bounds = None;
        let mut previews = MiniPreviews::default();
        previews
            .begin_capture(&AppSettings::default(), Some(target), 1)
            .unwrap();

        assert_eq!(
            previews.start_artifact(&artifact).unwrap_err(),
            "Mini-preview positioning is unavailable for this display."
        );
        assert!(previews.cards.is_empty());
        assert!(!previews.visibility.is_suppressed());
        assert_eq!(captures_app::list(root.path()).unwrap().len(), 1);
    }

    #[test]
    fn shared_preview_geometry_handles_opposite_placements() {
        let bounds = captures_app::preview::ThumbnailMonitorBounds {
            work_x: -160,
            work_y: 124,
            work_width: 3120,
            work_height: 1700,
            full_x: -200,
            full_y: 100,
            full_width: 3200,
            full_height: 1800,
            scale_factor: 2.,
        };

        let top_left = captures_app::preview::thumbnail_geometry(
            bounds,
            1,
            false,
            None,
            captures_settings::MiniPreviewPlacement::TopLeft,
        );
        let bottom_right = captures_app::preview::thumbnail_geometry(
            bounds,
            1,
            false,
            None,
            captures_settings::MiniPreviewPlacement::BottomRight,
        );
        assert!(top_left.x < bottom_right.x);
        assert!(top_left.y < bottom_right.y);
    }

    #[test]
    fn clear_history_drains_at_shutdown_and_invalidates_selected_preview() {
        let root = tempfile::tempdir().unwrap();
        let artifact = captures_app::persist_screenshot(
            root.path(),
            &image::RgbaImage::new(7, 3),
            captures_capture::CaptureMode::Window,
        )
        .unwrap();
        let mut live = Live::new(egui::Context::default(), Some(root.path().into()));
        live.apply(
            Response::History {
                artifacts: captures_app::list(root.path()).unwrap(),
            },
            true,
        );
        let decoding = live.selection.generation;
        live.decoded_path = Some(artifact.image_path);
        live.confirm_delete = Some(artifact.entry.id);
        live.confirm_clear_history = true;
        live.send(Request::ClearHistory {
            root: root.path().into(),
        });
        live.flush();
        let response = live
            .rx
            .try_iter()
            .find_map(|reply| match reply {
                Reply::HistoryCleared(result) => Some(result.unwrap()),
                _ => None,
            })
            .expect("dedicated clear response");
        live.apply(*response, true);
        assert!(live.artifacts.is_empty());
        assert!(captures_app::list(root.path()).unwrap().is_empty());
        assert!(!live.selection.accepts(decoding));
        assert!(live.selection.id.is_none());
        assert!(live.decoded_path.is_none());
        assert!(!live.preview_loading);
        assert!(live.confirm_delete.is_none());
        assert!(!live.confirm_clear_history);
    }

    #[test]
    fn cancelled_or_stale_prepare_reply_never_opens_the_selector() {
        let phase = Some(CapturePhase::RegionPreparing);
        assert!(accepts_prepare_reply(
            Some(12),
            12,
            true,
            phase,
            CapturePhase::RegionPreparing
        ));
        assert!(!accepts_prepare_reply(
            Some(12),
            10,
            true,
            phase,
            CapturePhase::RegionPreparing
        ));
        assert!(!accepts_prepare_reply(
            Some(12),
            12,
            false,
            phase,
            CapturePhase::RegionPreparing
        ));
        assert!(!accepts_prepare_reply(
            Some(12),
            12,
            true,
            Some(CapturePhase::RegionSelecting),
            CapturePhase::RegionPreparing
        ));
        assert!(!accepts_prepare_reply(
            Some(12),
            12,
            true,
            phase,
            CapturePhase::WindowPreparing
        ));
        assert!(accepts_prepare_reply(
            Some(12),
            12,
            true,
            Some(CapturePhase::ControlsPreparing),
            CapturePhase::ControlsPreparing
        ));
        assert!(!accepts_prepare_reply(
            Some(12),
            11,
            true,
            Some(CapturePhase::ControlsPreparing),
            CapturePhase::ControlsPreparing
        ));
    }

    #[test]
    fn shortcut_scope_exists_only_for_the_matching_presented_selector() {
        assert_eq!(
            active_selector_generation(12, Some(CapturePhase::ControlsSelecting), Some(12), true,),
            Some(12)
        );
        for phase in [
            CapturePhase::ControlsPreparing,
            CapturePhase::ControlsCountdown {
                target: capture_controls::Target::Display,
                after_countdown: false,
            },
            CapturePhase::ControlsCapturing,
        ] {
            assert_eq!(
                active_selector_generation(12, Some(phase), Some(12), true),
                None
            );
        }
        assert_eq!(
            active_selector_generation(0, Some(CapturePhase::ControlsSelecting), Some(12), true,),
            None,
            "same-frame child transitions clear scope before logic changes phase",
        );
        assert_eq!(
            active_selector_generation(10, Some(CapturePhase::ControlsSelecting), Some(12), true,),
            None,
        );
        assert_eq!(
            active_selector_generation(12, Some(CapturePhase::ControlsSelecting), Some(12), false,),
            None,
        );
    }

    #[test]
    fn stale_selector_action_cannot_change_a_newer_flow() {
        let phase = Some(CapturePhase::RegionSelecting);
        assert!(accepts_selector_action(
            Some(12),
            12,
            true,
            phase,
            SelectorKind::Region
        ));
        assert!(accepts_selector_action(
            Some(12),
            12,
            true,
            Some(CapturePhase::ControlsSelecting),
            SelectorKind::Controls
        ));
        assert!(!accepts_selector_action(
            Some(12),
            10,
            true,
            Some(CapturePhase::ControlsSelecting),
            SelectorKind::Controls
        ));
        assert!(!accepts_selector_action(
            Some(12),
            10,
            true,
            phase,
            SelectorKind::Region
        ));
        assert!(!accepts_selector_action(
            Some(12),
            12,
            false,
            phase,
            SelectorKind::Region
        ));
        assert!(!accepts_selector_action(
            Some(12),
            12,
            true,
            Some(CapturePhase::RegionCountdown {
                rect: SelectionRect {
                    x: 1.,
                    y: 2.,
                    width: 3.,
                    height: 4.,
                },
                after_countdown: false,
            }),
            SelectorKind::Region
        ));
        assert!(!accepts_selector_action(
            Some(12),
            12,
            true,
            Some(CapturePhase::WindowSelecting),
            SelectorKind::Region
        ));
        assert!(accepts_selector_action(
            Some(12),
            12,
            true,
            Some(CapturePhase::WindowSelecting),
            SelectorKind::Window
        ));
    }

    #[test]
    fn monitor_matching_compares_the_backend_overlay_contract_to_winit_dips() {
        assert!(monitor_matches_overlay(
            (-100., 50., 1600., 900.),
            (-200, 100, 3200, 1800),
            2.,
            2.,
        ));
        assert!(!monitor_matches_overlay(
            (-100., 50., 1600., 900.),
            (-200, 100, 3000, 1800),
            2.,
            2.,
        ));
        assert!(!monitor_matches_overlay(
            (-100., 50., 1600., 900.),
            (-200, 100, 3200, 1800),
            2.,
            1.5,
        ));
    }

    #[test]
    fn stale_hidden_generation_cannot_resurrect_ended_or_replaced_controls() {
        assert!(recording_controls_hidden(Some(41), Some(41)));
        assert!(!recording_controls_hidden(Some(41), Some(42)));
        assert!(!recording_controls_hidden(Some(41), None));
        assert!(!recording_controls_hidden(None, Some(41)));
    }
}
