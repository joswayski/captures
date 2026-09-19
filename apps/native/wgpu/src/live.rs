use std::{
    borrow::Cow,
    collections::HashMap,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc, Mutex,
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
    window::{Target as WindowCaptureTarget, WindowSession},
};
use captures_capture::{DisplayDescriptor, LogicalRect};
use captures_settings::AppSettings;
use eframe::egui::{self, RichText};

use crate::tokens::Tokens;
use crate::{
    selector,
    selector::Selector,
    window_selector::{self, SelectionTarget, WindowSelector},
};

enum Job {
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
    can_hide: Option<bool>,
    confirm_delete: Option<String>,
    confirm_clear_history: bool,
}

impl Live {
    pub fn new(ctx: egui::Context, root: Option<PathBuf>) -> Self {
        let root = root.unwrap_or_else(captures_app::default_history_root);
        let (tx, jobs) = mpsc::channel();
        let (out, rx) = mpsc::channel();
        let (selector_tx, selector_rx) = mpsc::channel();
        let (preview_tx, preview_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            // Retain ownership on X11. Disk decode and clipboard encoding never
            // block the UI, and the full uncompressed image is not retained by it.
            let mut clipboard = None;
            while let Ok(job) = jobs.recv() {
                let reply = match job {
                    Job::Shutdown => break,
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
                ctx.request_repaint();
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
            can_hide: None,
            confirm_delete: None,
            confirm_clear_history: false,
        };
        live.send(Request::History {
            root: live.root.clone(),
        });
        live.send(Request::Displays);
        live
    }

    pub fn is_capturing(&self) -> bool {
        self.flow.is_some() || self.capture_in_flight
    }

    pub fn take_open_history_requested(&mut self) -> bool {
        std::mem::take(&mut self.open_history_requested)
    }

    pub fn flush(&mut self) {
        // Cancel preparation/countdown before draining work. CaptureFlow::cancel
        // leaves a capture that already crossed its persistence commit point alone.
        if let Some(flow) = &self.flow {
            flow.cancel();
        }
        // Finish accepted capture/export/delete operations before process teardown.
        let _ = self.tx.send(Job::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        self.flow = None;
        self.capture_phase = None;
        self.region_session = None;
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
                            ctx.request_repaint();
                        }
                        Err(error) => {
                            self.error = Some(error);
                            self.finish_capture(ctx, false);
                        }
                    }
                }
                SelectorMessage::ConfirmRegion { .. }
                | SelectorMessage::ConfirmWindow { .. }
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
                        ctx.request_repaint();
                    }
                }
                PreviewMessage::Copy { .. }
                | PreviewMessage::Save { .. }
                | PreviewMessage::OpenHistory { .. } => {}
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
                self.status = "Capture cancelled (Escape or desktop session unavailable).".into();
                self.finish_capture(ctx, false);
            } else {
                // Only active captures poll; settled history/preferences stay event-driven.
                ctx.request_repaint_after(Duration::from_millis(100));
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
                Reply::HistoryCleared(result) => {
                    self.pending = self.pending.saturating_sub(1);
                    match result {
                        Ok(response) => self.apply(*response, true),
                        Err(error) => {
                            // Some files may already have been deleted. Refresh the
                            // remaining history without concealing the operation error.
                            self.send(Request::History {
                                root: self.root.clone(),
                            });
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
                    let accepted = accepts_prepare_reply(
                        self.flow.as_ref().map(CaptureFlow::generation),
                        generation,
                        self.flow.as_ref().is_some_and(CaptureFlow::is_current),
                        self.capture_phase,
                        CapturePhase::WindowPreparing,
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
                            self.window_selector.lock().unwrap().reset();
                            self.capture_phase = Some(CapturePhase::WindowSelecting);
                            self.status =
                                "Choose a window or the display. Press Escape to cancel.".into();
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
                    let accepted = self
                        .flow
                        .as_ref()
                        .is_some_and(|flow| flow.generation() == generation)
                        && self.capture_phase == Some(CapturePhase::WindowCapturing);
                    if !accepted {
                        continue;
                    }
                    self.capture_in_flight = false;
                    let captured = result.is_ok();
                    self.finish_capture(ctx, captured);
                    match result {
                        Ok(artifact) => {
                            self.accept_artifact(*artifact, "Window selection captured as PNG")
                        }
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

    fn finish_capture(&mut self, ctx: &egui::Context, preserve_auto_copy: bool) {
        self.flow = None;
        self.capture_phase = None;
        self.capture_waiting_for_hide = false;
        self.hide_started = None;
        self.hidden_since = None;
        self.capture_in_flight = false;
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
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
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
                self.status = "History loaded".into();
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

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        t: &Tokens,
        frame: &eframe::Frame,
        settings: impl Fn() -> Result<AppSettings, String>,
    ) {
        if self.capture_phase == Some(CapturePhase::RegionSelecting) {
            let t = t.clone();
            let generation = self
                .flow
                .as_ref()
                .expect("selection owns flow")
                .generation();
            let target = self.countdown_target.expect("region target validated");
            let selector = Arc::clone(&self.region_selector);
            let sender = self.selector_tx.clone();
            let texture = self.region_texture.clone();
            let auto_start = self.region_auto_start;
            let (overlay_width, overlay_height) = self
                .region_session
                .as_ref()
                .expect("selection owns region session")
                .display()
                .overlay_size();
            let overlay_bounds = captures_app::selection::Bounds {
                width: overlay_width,
                height: overlay_height,
            };
            ui.ctx().show_viewport_deferred(
                egui::ViewportId::from_hash_of("region-selector"),
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
            ui.ctx().show_viewport_deferred(
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
        if let Some(flow) = &self.flow
            && !self.capture_waiting_for_hide
            && !self.capture_in_flight
            && matches!(
                self.capture_phase,
                Some(
                    CapturePhase::DisplayCountdown
                        | CapturePhase::RegionCountdown { .. }
                        | CapturePhase::WindowCountdown { .. }
                )
            )
        {
            let clock = flow.countdown();
            if clock.remaining(Instant::now()) > 0 {
                let t = t.clone();
                let generation = flow.generation();
                let target = self.countdown_target.expect("countdown target validated");
                ui.ctx().show_viewport_deferred(
                    egui::ViewportId::from_hash_of("screenshot-countdown"),
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
            } else {
                // Stop declaring the child before hiding the root. Hidden-root
                // logic then verifies visibility and waits for compositor settling.
                self.hide_for_capture(ui.ctx());
            }
        }
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
                let capture = ui.add_enabled(self.pending == 0 && self.display_id.is_some() && self.can_hide == Some(true), egui::Button::new("Capture display"));
                if capture.clicked() {
                    match settings() {
                        Ok(settings) => {
                            let target = capture_target(frame, &self.displays, self.display_id.as_deref());
                            self.countdown_target = target;
                            if settings.screenshot_countdown_seconds > 0 && target.is_none() {
                                self.error = Some("The selected display is no longer available for countdown.".into());
                            } else {
                                match CaptureFlow::begin(settings.screenshot_countdown_seconds) {
                                    Ok(flow) => {
                                        match self.previews.begin_capture(
                                            &settings,
                                            target,
                                            ui.ctx().cumulative_frame_nr(),
                                        ) {
                                            Ok(()) => {
                                                self.flow = Some(flow);
                                                self.capture_phase = Some(CapturePhase::DisplayCountdown);
                                                self.auto_copy_on_capture = settings.auto_copy_to_clipboard;
                                                self.include_cursor = settings.show_cursor_in_screenshots;
                                                self.status = "Preparing screenshot… Press Escape to cancel.".into();
                                                ui.ctx().request_repaint();
                                            }
                                            Err(error) => {
                                                flow.cancel();
                                                self.error = Some(error);
                                            }
                                        }
                                    }
                                    Err(error) => self.error = Some(format!("Could not arm capture Escape: {error}")),
                                }
                            }
                        }
                        Err(error) => self.error = Some(error),
                    }
                }
                let region = ui.add_enabled(self.pending == 0 && self.display_id.is_some() && self.can_hide == Some(true), egui::Button::new("Capture region"));
                if region.clicked() {
                    match settings() {
                        Ok(settings) => {
                            let target = capture_target(frame, &self.displays, self.display_id.as_deref());
                            self.countdown_target = target;
                            if target.is_none() {
                                self.error = Some("The selected display is no longer available for region selection.".into());
                            } else {
                                match CaptureFlow::begin(0) {
                                    Ok(flow) => {
                                        match self.previews.begin_capture(
                                            &settings,
                                            target,
                                            ui.ctx().cumulative_frame_nr(),
                                        ) {
                                            Ok(()) => {
                                                self.flow = Some(flow);
                                                self.capture_phase = Some(CapturePhase::RegionPreparing);
                                                self.auto_copy_on_capture = settings.auto_copy_to_clipboard;
                                                self.include_cursor = settings.show_cursor_in_screenshots;
                                                self.region_freeze = settings.freeze_screen;
                                                self.region_auto_start = settings.auto_start_on_selection;
                                                self.region_countdown_seconds = settings.screenshot_countdown_seconds;
                                                self.status = "Preparing region selector… Press Escape to cancel.".into();
                                                self.hide_for_capture(ui.ctx());
                                            }
                                            Err(error) => {
                                                flow.cancel();
                                                self.error = Some(error);
                                            }
                                        }
                                    }
                                    Err(error) => self.error = Some(format!("Could not arm capture Escape: {error}")),
                                }
                            }
                        }
                        Err(error) => self.error = Some(error),
                    }
                }
                let window = ui.add_enabled(self.pending == 0 && self.display_id.is_some() && self.can_hide == Some(true), egui::Button::new("Capture window"));
                if window.clicked() {
                    match settings() {
                        Ok(settings) => {
                            let target = capture_target(frame, &self.displays, self.display_id.as_deref());
                            self.countdown_target = target;
                            if target.is_none() {
                                self.error = Some("The selected display is no longer available for window selection.".into());
                            } else {
                                match CaptureFlow::begin(0) {
                                    Ok(flow) => {
                                        match self.previews.begin_capture(
                                            &settings,
                                            target,
                                            ui.ctx().cumulative_frame_nr(),
                                        ) {
                                            Ok(()) => {
                                                self.flow = Some(flow);
                                                self.capture_phase = Some(CapturePhase::WindowPreparing);
                                                self.auto_copy_on_capture = settings.auto_copy_to_clipboard;
                                                self.include_cursor = settings.show_cursor_in_screenshots;
                                                self.window_freeze = settings.freeze_screen;
                                                self.window_auto_start = settings.auto_start_on_selection;
                                                self.window_countdown_seconds = settings.screenshot_countdown_seconds;
                                                self.status = "Preparing window selector… Press Escape to cancel.".into();
                                                self.hide_for_capture(ui.ctx());
                                            }
                                            Err(error) => {
                                                flow.cancel();
                                                self.error = Some(error);
                                            }
                                        }
                                    }
                                    Err(error) => self.error = Some(format!("Could not arm capture Escape: {error}")),
                                }
                            }
                        }
                        Err(error) => self.error = Some(error),
                    }
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
                                        "{}×{}\n{}",
                                        item.width, item.height, date
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
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        self.decoded_path.is_some() && self.pending == 0,
                        egui::Button::new("Copy pixels"),
                    )
                    .clicked()
                {
                    self.copy();
                }
                if ui
                    .add_enabled(
                        selected.is_some() && self.pending == 0,
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
            })
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
}
