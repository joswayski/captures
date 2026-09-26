use std::{
    borrow::Cow,
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::{Duration, Instant},
};

use captures_app::capture_menu::PreferenceTarget;
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
use crate::{recording, recording_hud, reveal::reveal, tokens::Tokens};

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub(super) enum HistoryFilter {
    #[default]
    All,
    Screenshots,
    Video,
    Gif,
}

impl HistoryFilter {
    const ALL: [Self; 4] = [Self::All, Self::Screenshots, Self::Video, Self::Gif];

    fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Screenshots => "Screenshots",
            Self::Video => "Video",
            Self::Gif => "GIF",
        }
    }

    pub(super) fn matches(self, kind: captures_history::ArtifactKind) -> bool {
        use captures_history::ArtifactKind;
        matches!(
            (self, kind),
            (Self::All, _)
                | (Self::Screenshots, ArtifactKind::Screenshot)
                | (Self::Video, ArtifactKind::Video)
                | (Self::Gif, ArtifactKind::Gif)
        )
    }

    // The live workspace and disposable fixture use the same filter controls.
    pub(super) fn ui(
        &mut self,
        ui: &mut egui::Ui,
        kinds: impl Iterator<Item = captures_history::ArtifactKind>,
    ) -> bool {
        let mut counts = [0usize; 4];
        for kind in kinds {
            for (index, filter) in Self::ALL.iter().enumerate() {
                counts[index] += usize::from(filter.matches(kind));
            }
        }
        let before = *self;
        ui.horizontal_wrapped(|ui| {
            for (filter, count) in Self::ALL.into_iter().zip(counts) {
                let label = format!("{} {count}", filter.label());
                if ui
                    .add_enabled(
                        filter == Self::All || count > 0,
                        egui::Button::new(label).selected(*self == filter),
                    )
                    .clicked()
                {
                    *self = filter;
                }
            }
        });
        *self != before
    }
}

enum Job {
    #[cfg(test)]
    Barrier(Sender<()>),
    LoadHistory {
        root: PathBuf,
        open_recording: Option<(String, PathBuf, u64)>,
    },
    OpenMedia {
        root: PathBuf,
        path: PathBuf,
        open_artifact_ids: Vec<String>,
        output_directory: PathBuf,
    },
    Execute {
        request: Request,
        preview: Option<PreviewGuard>,
        notice: Option<crate::recording_saved_notice::Guard>,
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
    DecodeThumbnail {
        root: PathBuf,
        entry: Box<captures_history::HistoryEntry>,
        key: ThumbnailKey,
    },
    DecodePreview {
        generation: u64,
        artifact_id: String,
        path: PathBuf,
    },
    Copy {
        path: PathBuf,
        preview: Option<PreviewGuard>,
        /// Artifact that owns the clipboard after a successful copy.
        owner: Option<String>,
    },
    VerifyClipboard(captures_app::clipboard::ClipboardVerification),
    Reveal {
        path: PathBuf,
        preview: PreviewGuard,
    },
    CopyPixels {
        pixels: Arc<image::RgbaImage>,
        reply: Sender<Result<(), String>>,
    },
    Shutdown,
}

enum Reply {
    HistoryLoaded {
        result: Result<Vec<Artifact>, String>,
        open_recording: Option<(String, PathBuf, u64)>,
    },
    MediaOpened {
        path: PathBuf,
        result: Result<Box<Artifact>, String>,
        output_directory: PathBuf,
    },
    Executed {
        preview: Option<PreviewGuard>,
        notice: Option<crate::recording_saved_notice::Guard>,
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
        owner: Option<String>,
        result: Result<ClipboardWrite, String>,
    },
    ClipboardVerified {
        verification: captures_app::clipboard::ClipboardVerification,
        result: Result<bool, String>,
    },
    Revealed {
        preview: PreviewGuard,
        result: Result<(), String>,
    },
    ThumbnailDecoded {
        id: String,
        key: ThumbnailKey,
        missing: bool,
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

/// Identifies the History entry a thumbnail and missing check were read for.
/// An editor replacing the original or a save changes it, forcing a reload.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ThumbnailKey {
    path: PathBuf,
    width: u32,
    height: u32,
    size_bytes: u64,
    saved_path: Option<String>,
}

impl ThumbnailKey {
    fn of(artifact: &Artifact) -> Self {
        Self {
            path: artifact.preview_path.clone(),
            width: artifact.entry.width,
            height: artifact.entry.height,
            size_bytes: artifact.entry.size_bytes,
            saved_path: artifact.entry.saved_path.clone(),
        }
    }
}

struct HistoryThumbnail {
    key: ThumbnailKey,
    thumbnail: crate::history::Thumbnail,
    missing: bool,
}

/// Bounded texture residency: offscreen thumbnails beyond this are released.
const HISTORY_THUMBNAIL_CACHE: usize = 96;

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
    /// The engine could not start the take. Shipping keeps the HUD open with the
    /// error, Retry recording and Delete.
    RecordingFailed,
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
    RetiringCaptureUi {
        rect: SelectionRect,
        after_countdown: bool,
        omitted_frame: u64,
    },
    SettlingCaptureUi {
        rect: SelectionRect,
        after_countdown: bool,
        until: Instant,
    },
    Capturing,
}

impl RecordingScreenshotPhase {
    /// Deferred viewports retire only after the root pass that omits them.
    /// Start the compositor settling interval after that pass, not at countdown
    /// expiry, which may still be running alongside a visible countdown window.
    fn capture_after_hide(&mut self, frame: u64, now: Instant) -> Option<(SelectionRect, bool)> {
        match *self {
            Self::RetiringCaptureUi {
                rect,
                after_countdown,
                omitted_frame,
            } if frame > omitted_frame => {
                *self = Self::SettlingCaptureUi {
                    rect,
                    after_countdown,
                    until: now + Duration::from_millis(150),
                };
                None
            }
            Self::SettlingCaptureUi {
                rect,
                after_countdown,
                until,
            } if now >= until => {
                *self = Self::Capturing;
                Some((rect, after_countdown))
            }
            _ => None,
        }
    }
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
    /// Confirmed Delete recording; the HUD first asks with `RequestDeleteRecording`.
    DiscardRecording {
        generation: u64,
    },
    RequestDeleteRecording {
        generation: u64,
    },
    CancelDeleteRecording {
        generation: u64,
    },
    HideRecordingControls {
        generation: u64,
    },
    SwitchControlsDisplay {
        generation: u64,
        display_id: String,
    },
    OpenPreference {
        generation: u64,
        target: PreferenceTarget,
    },
    Cancel {
        generation: u64,
        kind: SelectorKind,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum PreviewMessage {
    DragStarted {
        artifact_id: String,
        generation: u64,
    },
    DragFinished {
        artifact_id: String,
        generation: u64,
        result: Result<captures_app::preview::PreviewDragOutcome, String>,
    },
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
    Reveal {
        artifact_id: String,
        generation: u64,
        path: PathBuf,
    },
    Trash {
        artifact_id: String,
        generation: u64,
        saved_path: Option<PathBuf>,
    },
    Edit {
        artifact_id: String,
        generation: u64,
        directory: PathBuf,
    },
    Dismiss {
        artifact_id: String,
        generation: u64,
    },
    MoveStack {
        artifact_id: String,
        generation: u64,
        position: egui::Pos2,
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

#[derive(Clone, Copy)]
struct CountdownExit {
    until: Instant,
    viewport: egui::ViewportId,
    title: &'static str,
    target: CaptureTarget,
    kind: crate::countdown::Kind,
    remaining: u8,
}

impl CountdownExit {
    fn new(
        viewport: egui::ViewportId,
        title: &'static str,
        target: CaptureTarget,
        kind: crate::countdown::Kind,
        remaining: u8,
    ) -> Self {
        Self {
            until: Instant::now() + Duration::from_millis(crate::countdown::CANCEL_LINGER_MS),
            viewport,
            title,
            target,
            kind,
            remaining,
        }
    }
}

struct PreviewCard {
    generation: u64,
    artifact_id: String,
    image_path: PathBuf,
    width: u32,
    height: u32,
    texture: Option<egui::TextureHandle>,
    size_bytes: u64,
    busy: Option<crate::mini_preview::Busy>,
    message: Option<String>,
    saved_path: Option<PathBuf>,
    /// Start of the brief "Saved" confirmation after an explicit save.
    saved_at: Option<Instant>,
    rejected_at: Option<Instant>,
    /// When the decoded image first painted: shipping `thumbnail-arrive`.
    arrived_at: Option<Instant>,
}

#[derive(Clone)]
struct PreviewRenderCard {
    artifact_id: String,
    generation: u64,
    width: u32,
    height: u32,
    texture: egui::TextureHandle,
    size_bytes: u64,
    busy: Option<crate::mini_preview::Busy>,
    message: Option<String>,
    saved_path: Option<PathBuf>,
    saved_at: Option<Instant>,
    clipboard_current: bool,
    rejected_at: Option<Instant>,
    arrived_at: Option<Instant>,
    layout: captures_app::preview::PreviewCardLayout,
    hover_y: f64,
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
        if !self.show || self.placement != settings.mini_preview_placement {
            self.visibility.clear_stack_origin();
        }
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
                size_bytes: artifact.entry.size_bytes,
                busy: None,
                message: None,
                saved_path: artifact.entry.saved_path.as_deref().map(PathBuf::from),
                saved_at: None,
                rejected_at: None,
                arrived_at: None,
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
            self.visibility.clear_stack_origin();
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
            self.visibility.clear_stack_origin();
        }
        removed
    }

    fn move_stack(&mut self, position: egui::Pos2) {
        use captures_app::preview::{self, ThumbnailStackOrigin};
        if !self.stack.is_collapsed() || !self.is_visible() {
            return;
        }
        let Some(bounds) = self.stack_target.and_then(|target| target.preview_bounds) else {
            return;
        };
        let count = self.stack.ids().len();
        let anchor = self.placement.into();
        let edge_offset = preview::collapsed_padding(count)
            + if self.placement.is_top() {
                -preview::THUMBNAIL_CONTROL_GUTTER
            } else {
                preview::THUMBNAIL_CARD_HEIGHT + preview::THUMBNAIL_CONTROL_GUTTER
            };
        let geometry = preview::thumbnail_geometry(
            bounds,
            count,
            true,
            Some(ThumbnailStackOrigin {
                x: position.x as f64,
                edge: position.y as f64 + edge_offset,
                anchor,
            }),
            self.placement,
        );
        self.visibility.set_stack_origin(ThumbnailStackOrigin {
            x: geometry.x,
            edge: geometry.y + edge_offset,
            anchor,
        });
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

/// How often Linux checks that the clipboard still holds a preview's pixels.
const CLIPBOARD_CHECK_INTERVAL: Duration = Duration::from_secs(1);
/// Shipping `THUMBNAIL_SAVED_FEEDBACK_MS`.
const SAVED_FEEDBACK: Duration = Duration::from_millis(1_000);

/// Shipping keeps the controls-hidden notice window for 6.2 s.
const RECORDING_HIDDEN_NOTICE_MS: f64 = 6_200.;

pub(crate) fn request_hidden_root_paint(ctx: &egui::Context) {
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
    editors: HashMap<String, crate::editor::Editor>,
    recording_editors: HashMap<String, crate::recording_editor::Editor>,
    recovery: crate::recording_recovery::Recovery,
    recovery_selection: u64,
    history_filter: HistoryFilter,
    selection: Selection,
    /// Card presentation and thumbnails, keyed by artifact ID.
    history_cards: HashMap<String, captures_app::history_view::Card>,
    history_thumbnails: HashMap<String, HistoryThumbnail>,
    history_loaded: bool,
    history_scroll_to: Option<String>,
    card_busy: Option<(String, captures_app::history_view::CardAction)>,
    status: String,
    error: Option<String>,
    pending: usize,
    open_media: VecDeque<(PathBuf, PathBuf)>,
    opening_media: bool,
    media_open_errors: Vec<String>,
    capture_waiting_for_hide: bool,
    hide_started: Option<Instant>,
    hidden_since: Option<Instant>,
    capture_in_flight: bool,
    auto_copy_on_capture: bool,
    include_cursor: bool,
    flow: Option<CaptureFlow>,
    capture_phase: Option<CapturePhase>,
    countdown_target: Option<CaptureTarget>,
    /// Keeps a cancelled countdown up briefly with the shipping "Cancelling…" copy.
    countdown_exit: Option<CountdownExit>,
    region_session: Option<Box<RegionSession>>,
    region_texture: Option<egui::TextureHandle>,
    region_selector: Arc<Mutex<Selector>>,
    selector_tx: Sender<SelectorMessage>,
    selector_rx: Receiver<SelectorMessage>,
    preview_tx: Sender<PreviewMessage>,
    preview_rx: Receiver<PreviewMessage>,
    notice_tx: Sender<crate::recording_saved_notice::Action>,
    notice_rx: Receiver<crate::recording_saved_notice::Action>,
    recording_notice: Option<crate::recording_saved_notice::Notice>,
    recording_notice_generation: u64,
    recording_notice_target: Option<CaptureTarget>,
    /// Output folder when `open_editor_after_recording` applies to this take.
    open_editor_after_recording: Option<PathBuf>,
    /// Where the saved notice appears when each recording editor closes.
    recording_editor_notice_targets: HashMap<String, Option<CaptureTarget>>,
    /// Most recent capture display, for editors opened outside a capture.
    last_capture_target: Option<CaptureTarget>,
    previews: MiniPreviews,
    clipboard: captures_app::clipboard::ClipboardOwnership,
    root_hide_deferred: bool,
    region_freeze: bool,
    region_countdown_seconds: u8,
    window_session: Option<Arc<WindowSession>>,
    window_texture: Option<egui::TextureHandle>,
    window_selector: Arc<Mutex<WindowSelector>>,
    window_freeze: bool,
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
    recording_microphone_peak: f32,
    /// Shipping `.recording-hud-error` line: action failures and engine warnings.
    recording_hud_error: captures_app::recording_hud::ErrorLine,
    recording_segment_started: Option<Instant>,
    recording_snapshot_poll_pending: bool,
    recording_last_snapshot_poll: Instant,
    recording_has_started: bool,
    recording_restart_confirmation: bool,
    /// Shipping "Delete recording?" confirmation is open.
    recording_delete_confirmation: bool,
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
    /// Shipping two-step deletion: the armed card and when it reverts.
    confirm_delete: Option<(String, Instant)>,
    confirm_clear_history: Option<Instant>,
    clearing_history: bool,
    requested_capture: Option<CaptureRequest>,
    restore_root_visible: bool,
    permission_recovery_requested: bool,
    /// A capture-menu note link asked the workbench to open Preferences here.
    preference_target_requested: Option<PreferenceTarget>,
    /// A start or display switch failed while New Capture stayed open.
    controls_error: Option<String>,
    permission_recovery_visible: bool,
}

impl Live {
    pub fn new(ctx: egui::Context, root: Option<PathBuf>) -> Self {
        let root = root.unwrap_or_else(captures_app::default_history_root);
        let (tx, jobs) = mpsc::channel();
        let (out, rx) = mpsc::channel();
        let (selector_tx, selector_rx) = mpsc::channel();
        let (preview_tx, preview_rx) = mpsc::channel();
        let (notice_tx, notice_rx) = mpsc::channel();
        let capture_ctx = ctx.clone();
        let drag_root = root.clone();
        let worker = thread::spawn(move || {
            let _ = captures_app::preview_drag::clear_previous_exports(&drag_root);
            // Retain ownership on X11. Disk decode and clipboard encoding never
            // block the UI, and the full uncompressed image is not retained by it.
            let mut clipboard = None;
            while let Ok(job) = jobs.recv() {
                let reply = match job {
                    Job::Shutdown => break,
                    #[cfg(test)]
                    Job::Barrier(done) => {
                        let _ = done.send(());
                        continue;
                    }
                    Job::LoadHistory {
                        root,
                        open_recording,
                    } => Reply::HistoryLoaded {
                        result: load_history(&root),
                        open_recording,
                    },
                    Job::OpenMedia {
                        root,
                        path,
                        open_artifact_ids,
                        output_directory,
                    } => {
                        let result = captures_app::execute(Request::OpenMedia {
                            root,
                            path: path.clone(),
                            open_artifact_ids,
                            ffmpeg: None,
                            ffprobe: None,
                        })
                        .map(|response| match response {
                            Response::OpenedMedia { artifact, .. } => Box::new(artifact),
                            _ => unreachable!("OpenMedia returns OpenedMedia"),
                        })
                        .map_err(|error| error.to_string());
                        Reply::MediaOpened {
                            path,
                            result,
                            output_directory,
                        }
                    }
                    Job::Execute {
                        request,
                        preview,
                        notice,
                    } => {
                        let clearing = matches!(request, Request::ClearHistory { .. });
                        let result = captures_app::execute(request)
                            .map(Box::new)
                            .map_err(|error| error.to_string());
                        if clearing {
                            Reply::HistoryCleared(result)
                        } else {
                            Reply::Executed {
                                preview,
                                notice,
                                result,
                            }
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
                    Job::DecodeThumbnail { root, entry, key } => Reply::ThumbnailDecoded {
                        id: entry.id.clone(),
                        missing: captures_app::history_view::media_missing(&root, &entry),
                        result: decode(&key.path),
                        key,
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
                    Job::Copy {
                        path,
                        preview,
                        owner,
                    } => Reply::Copied {
                        preview,
                        owner,
                        result: copy_image(&path, &mut clipboard),
                    },
                    Job::VerifyClipboard(verification) => Reply::ClipboardVerified {
                        verification,
                        result: clipboard_matches(verification.fingerprint, &mut clipboard),
                    },
                    Job::Reveal { path, preview } => Reply::Revealed {
                        preview,
                        result: reveal(&path).map_err(|error| error.to_string()),
                    },
                    Job::CopyPixels { pixels, reply } => {
                        // The workspace owns X11 clipboard data beyond any editor's lifetime.
                        let _ = reply.send(copy_pixels(&pixels, &mut clipboard).map(|_| ()));
                        continue;
                    }
                };
                if out.send(reply).is_err() {
                    break;
                }
                // Only the root drains replies and advances queued media imports.
                // A child editor may be the active viewport when this finishes.
                capture_ctx.request_repaint_of(egui::ViewportId::ROOT);
            }
        });
        let mut live = Self {
            recovery: crate::recording_recovery::Recovery::new(ctx.clone(), root.clone()),
            recovery_selection: 0,
            root,
            tx,
            rx,
            worker: Some(worker),
            displays: vec![],
            display_id: None,
            artifacts: vec![],
            editors: HashMap::new(),
            recording_editors: HashMap::new(),
            history_filter: HistoryFilter::All,
            selection: Selection::default(),
            history_cards: HashMap::new(),
            history_thumbnails: HashMap::new(),
            history_loaded: false,
            history_scroll_to: None,
            card_busy: None,
            status: "Loading capture workspace…".into(),
            error: None,
            pending: 0,
            open_media: VecDeque::new(),
            opening_media: false,
            media_open_errors: Vec::new(),
            capture_waiting_for_hide: false,
            hide_started: None,
            hidden_since: None,
            capture_in_flight: false,
            auto_copy_on_capture: false,
            include_cursor: false,
            flow: None,
            capture_phase: None,
            countdown_target: None,
            countdown_exit: None,
            region_session: None,
            region_texture: None,
            region_selector: Arc::new(Mutex::new(Selector::default())),
            selector_tx,
            selector_rx,
            preview_tx,
            preview_rx,
            notice_tx,
            notice_rx,
            recording_notice: None,
            recording_notice_generation: 0,
            recording_notice_target: None,
            open_editor_after_recording: None,
            recording_editor_notice_targets: HashMap::new(),
            last_capture_target: None,
            previews: MiniPreviews::default(),
            clipboard: Default::default(),
            root_hide_deferred: false,
            region_freeze: false,
            region_countdown_seconds: 0,
            window_session: None,
            window_texture: None,
            window_selector: Arc::new(Mutex::new(WindowSelector::default())),
            window_freeze: false,
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
            recording_microphone_peak: 0.,
            recording_hud_error: Default::default(),
            recording_segment_started: None,
            recording_snapshot_poll_pending: false,
            recording_last_snapshot_poll: Instant::now(),
            recording_has_started: false,
            recording_restart_confirmation: false,
            recording_delete_confirmation: false,
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
            confirm_clear_history: None,
            clearing_history: false,
            requested_capture: None,
            restore_root_visible: true,
            permission_recovery_requested: false,
            preference_target_requested: None,
            controls_error: None,
            permission_recovery_visible: false,
        };
        live.load_history();
        live.send(Request::Displays);
        live
    }

    pub fn queue_open_media(
        &mut self,
        paths: impl IntoIterator<Item = PathBuf>,
        output_directory: Result<PathBuf, String>,
    ) {
        if self.open_media.is_empty() && !self.opening_media {
            self.media_open_errors.clear();
            self.error = None;
        }
        for path in paths {
            match &output_directory {
                Ok(directory) => self.open_media.push_back((path, directory.clone())),
                Err(error) => self
                    .media_open_errors
                    .push(format!("Could not open {}: {error}", path.display())),
            }
        }
        if !self.media_open_errors.is_empty() {
            self.error = Some(self.media_open_errors.join("\n"));
        }
    }

    fn start_next_media(&mut self) {
        if self.pending != 0
            || self.opening_media
            || self.is_capturing()
            || self.recovery.blocking()
            || self.permission_recovery_visible
        {
            return;
        }
        let Some((path, output_directory)) = self.open_media.pop_front() else {
            return;
        };
        self.opening_media = true;
        self.pending += 1;
        self.status = format!("Opening {}…", path.display());
        // Snapshot active editors only when dispatching. The pending gate also
        // blocks History from opening a competing editor until this reply arrives.
        let _ = self.tx.send(Job::OpenMedia {
            root: self.root.clone(),
            path,
            open_artifact_ids: self
                .editors
                .keys()
                .chain(self.recording_editors.keys())
                .cloned()
                .collect(),
            output_directory,
        });
    }

    fn open_screenshot_editor(
        &mut self,
        ctx: &egui::Context,
        id: String,
        output_directory: PathBuf,
        mode: captures_capture::CaptureMode,
    ) {
        let clipboard = self.tx.clone();
        self.editors
            .entry(id.clone())
            .or_insert_with(|| {
                crate::editor::Editor::open(
                    ctx,
                    self.root.clone(),
                    id,
                    output_directory,
                    mode,
                    move |pixels| {
                        let (reply, rx) = mpsc::channel();
                        clipboard
                            .send(Job::CopyPixels { pixels, reply })
                            .map_err(|_| "Clipboard worker stopped.".to_owned())?;
                        rx.recv()
                            .map_err(|_| "Clipboard worker stopped.".to_owned())?
                    },
                )
            })
            .focus(ctx);
    }

    fn open_recording_editor(
        &mut self,
        ctx: &egui::Context,
        id: String,
        output_directory: PathBuf,
    ) {
        self.recording_editors
            .entry(id.clone())
            .or_insert_with(|| {
                crate::recording_editor::Editor::open(ctx, self.root.clone(), id, output_directory)
            })
            .focus(ctx);
    }

    fn media_opened(
        &mut self,
        ctx: &egui::Context,
        path: &Path,
        result: Result<Box<Artifact>, String>,
        output_directory: PathBuf,
    ) {
        let _span = crate::diagnostics::span("media-opened");
        self.pending = self.pending.saturating_sub(1);
        self.opening_media = false;
        match result {
            Ok(artifact) => {
                let id = artifact.entry.id.clone();
                let recording = artifact.entry.kind.is_recording();
                let mode = artifact
                    .entry
                    .mode
                    .unwrap_or(captures_capture::CaptureMode::Display);
                self.artifacts.retain(|item| item.entry.id != id);
                self.artifacts.insert(0, *artifact);
                self.select(id.clone());
                if recording {
                    self.open_recording_editor(ctx, id, output_directory);
                } else {
                    self.open_screenshot_editor(ctx, id, output_directory, mode);
                }
                self.status = format!("Opened {}", path.display());
            }
            Err(error) => {
                self.media_open_errors
                    .push(format!("Could not open {}: {error}", path.display()));
                self.error = Some(self.media_open_errors.join("\n"));
            }
        }
    }

    pub fn is_capturing(&self) -> bool {
        self.flow.is_some() || self.capture_in_flight
    }

    pub fn can_launch_capture(&self) -> bool {
        self.pending == 0
            && !self.recovery.blocking()
            && !self.permission_recovery_visible
            && !self.is_capturing()
            && self.requested_capture.is_none()
    }

    pub fn take_permission_recovery_requested(&mut self) -> bool {
        std::mem::take(&mut self.permission_recovery_requested)
    }

    pub fn preference_target_pending(&self) -> bool {
        self.preference_target_requested.is_some()
    }

    pub fn take_preference_target_requested(&mut self) -> Option<PreferenceTarget> {
        self.preference_target_requested.take()
    }

    pub fn set_permission_recovery_visible(&mut self, visible: bool) {
        self.permission_recovery_visible = visible;
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

    pub fn request_capture(&mut self, request: CaptureRequest) {
        self.recording_notice = None;
        self.recording_notice_target = None;
        if request == CaptureRequest::NewCapture && self.recording_controls_hidden() {
            // The shipping New Capture action restores a hidden active HUD; it
            // never starts a second capture or replaces the accepted take.
            self.requested_capture = None;
            return;
        }
        if !self.can_launch_capture() {
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
        // Shipping shortcut, tray and New Capture flows start on the display
        // under the pointer. Keep the current display when it is unknown
        // (for example Wayland, where the pointer position is unavailable).
        if let Some(id) = captures_capture::pointer_position()
            .and_then(|point| captures_capture::XcapBackend.display_id_at_point(point))
            .filter(|id| self.displays.iter().any(|display| &display.id == id))
        {
            self.display_id = Some(id);
        }
        let target = capture_target(frame, &self.displays, self.display_id.as_deref());
        self.countdown_target = target;
        if target.is_some() {
            self.last_capture_target = target;
        }
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
        self.open_editor_after_recording = settings
            .recording
            .open_editor_after_recording
            .then(|| PathBuf::from(&settings.output_directory));
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
                self.controls_error = None;
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
                self.region_countdown_seconds = settings.screenshot_countdown_seconds;
                self.status = "Preparing region selector… Press Escape to cancel.".into();
                self.hide_for_capture(ctx);
            }
            CaptureRequest::Window => {
                self.capture_phase = Some(CapturePhase::WindowPreparing);
                self.window_freeze = settings.freeze_screen;
                self.window_countdown_seconds = settings.screenshot_countdown_seconds;
                self.status = "Preparing window selector… Press Escape to cancel.".into();
                self.hide_for_capture(ctx);
            }
        }
    }

    pub fn flush_editors(&self, ctx: &egui::Context) -> Result<(), String> {
        self.recovery.can_quit()?;
        for editor in self.recording_editors.values() {
            editor.flush(ctx)?;
        }
        for editor in self.editors.values() {
            editor.flush(ctx)?;
        }
        Ok(())
    }

    pub fn flush(&mut self) {
        self.open_media.clear();
        self.editors.clear();
        self.recording_editors.clear();
        self.recording_notice = None;
        self.recording_notice_target = None;
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
            notice: None,
        });
    }

    fn load_history(&mut self) {
        self.recovery.refresh();
        self.pending += 1;
        let _ = self.tx.send(Job::LoadHistory {
            root: self.root.clone(),
            open_recording: None,
        });
    }

    fn send_preview(&mut self, request: Request, preview: PreviewGuard) {
        self.pending += 1;
        let _ = self.tx.send(Job::Execute {
            request,
            preview: Some(preview),
            notice: None,
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

    fn select(&mut self, id: String) {
        if let Some(item) = self.artifacts.iter().find(|item| item.entry.id == id)
            && !self.history_filter.matches(item.entry.kind)
        {
            self.history_filter = HistoryFilter::All;
        }
        self.history_scroll_to = Some(id.clone());
        self.selection.begin(id);
        self.invalidate_history_cards();
    }

    /// Drop cached card presentation and any thumbnail read for an older
    /// version of an entry (or for an entry no longer in History).
    fn invalidate_history_cards(&mut self) {
        self.history_cards.clear();
        let current: HashMap<&str, ThumbnailKey> = self
            .artifacts
            .iter()
            .map(|artifact| (artifact.entry.id.as_str(), ThumbnailKey::of(artifact)))
            .collect();
        self.history_thumbnails
            .retain(|id, thumbnail| current.get(id.as_str()) == Some(&thumbnail.key));
    }

    fn artifact_index(&self, id: &str) -> Option<usize> {
        self.artifacts.iter().position(|item| item.entry.id == id)
    }

    /// Keep an explicit selection that is still visible. Like shipping, loading
    /// or filtering History never selects a card on the user's behalf.
    fn refresh_history_selection(&mut self) {
        let keep = self
            .selection
            .id
            .as_deref()
            .filter(|id| {
                self.artifacts.iter().any(|item| {
                    item.entry.id == *id && self.history_filter.matches(item.entry.kind)
                })
            })
            .map(str::to_owned);
        self.selection.clear();
        self.confirm_delete = None;
        self.invalidate_history_cards();
        if let Some(id) = keep {
            self.selection.begin(id);
        }
    }

    pub fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if let Some((outcome, directory)) = self.recovery.receive() {
            self.previews.remove(&outcome.entry.id);
            self.pending += 1;
            let _ = self.tx.send(Job::LoadHistory {
                root: self.root.clone(),
                open_recording: Some((outcome.entry.id, directory, self.recovery_selection)),
            });
        }
        let mut editor_history_changed = false;
        for (id, editor) in &self.recording_editors {
            editor.receive(ctx);
            editor_history_changed |= editor.take_history_changed();
            if editor.take_original_replaced() {
                self.previews.remove(id);
            }
        }
        let closed = self
            .recording_editors
            .iter()
            .filter(|(_, editor)| editor.closed())
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for id in closed {
            // Shipping shows the saved notice whenever a recording editor closes.
            let target = self
                .recording_editor_notice_targets
                .remove(&id)
                .flatten()
                .or(self.last_capture_target);
            self.recording_notice_generation = self.recording_notice_generation.wrapping_add(1);
            self.recording_notice = Some(crate::recording_saved_notice::Notice::new(
                id,
                self.recording_notice_generation,
                Instant::now(),
            ));
            self.recording_notice_target = target;
            request_hidden_root_paint(ctx);
        }
        self.recording_editors.retain(|_, editor| !editor.closed());
        for (id, editor) in &self.editors {
            editor.receive(ctx);
            editor_history_changed |= editor.take_history_changed();
            if editor.take_original_replaced() {
                self.previews.remove(id);
            }
        }
        self.editors.retain(|_, editor| !editor.closed());
        if editor_history_changed {
            self.load_history();
        }
        if self
            .recording_notice
            .as_ref()
            .is_some_and(|notice| notice.expired(Instant::now()))
        {
            self.recording_notice = None;
            self.recording_notice_target = None;
        }
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
                SelectorMessage::OpenPreference { generation, target }
                    if accepts_selector_action(
                        self.flow.as_ref().map(CaptureFlow::generation),
                        generation,
                        self.flow.as_ref().is_some_and(CaptureFlow::is_current),
                        self.capture_phase,
                        SelectorKind::Controls,
                    ) =>
                {
                    // Shipping `openCapturePreference`: dismiss the menu, then
                    // show Preferences at the linked row.
                    let Some(flow) = &self.flow else { continue };
                    flow.cancel();
                    self.preference_target_requested = Some(target);
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
                        self.controls_error.clone_from(&self.error);
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
                    self.hud_action_started();
                    self.capture_phase = Some(CapturePhase::RecordingPausing);
                    self.status = "Pausing recording…".into();
                    self.recording_worker
                        .send(recording::Command::Pause { generation });
                }
                SelectorMessage::ResumeRecording { generation }
                    if self.flow.as_ref().map(CaptureFlow::generation) == Some(generation)
                        && self.capture_phase == Some(CapturePhase::RecordingPaused) =>
                {
                    self.hud_action_started();
                    self.capture_phase = Some(CapturePhase::RecordingStarting);
                    self.status = "Resuming recording…".into();
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
                    self.hud_action_started();
                    self.capture_phase = Some(CapturePhase::RecordingMuting { paused });
                    self.status = if muted {
                        "Muting microphone…".into()
                    } else {
                        "Unmuting microphone…".into()
                    };
                    request_hidden_root_paint(ctx);
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
                        && self.capture_phase == Some(CapturePhase::RecordingFailed) =>
                {
                    // Shipping "Retry recording" restarts a failed take without asking.
                    self.recording_delete_confirmation = false;
                    self.restart_recording(ctx, generation);
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
                    self.hud_action_started();
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
                    self.restart_recording(ctx, generation);
                }
                SelectorMessage::StopRecording { generation }
                    if self.flow.as_ref().map(CaptureFlow::generation) == Some(generation)
                        && matches!(
                            self.capture_phase,
                            Some(CapturePhase::Recording | CapturePhase::RecordingPaused)
                        ) =>
                {
                    self.hud_action_started();
                    // Shipping keeps the HUD up as "Saving…" with a frozen timer.
                    if let Some(snapshot) = &mut self.recording_snapshot {
                        snapshot.elapsed_ms = interpolated_recording_elapsed(
                            snapshot.elapsed_ms,
                            self.recording_segment_started
                                .map(|started| started.elapsed()),
                        );
                    }
                    self.recording_segment_started = None;
                    self.capture_phase = Some(CapturePhase::RecordingFinalizing);
                    self.status = "Finalizing recording…".into();
                    request_hidden_root_paint(ctx);
                    self.recording_worker.send(recording::Command::Finish {
                        generation,
                        history_root: self.root.clone(),
                    });
                }
                SelectorMessage::DiscardRecording { generation }
                    if self.flow.as_ref().map(CaptureFlow::generation) == Some(generation)
                        && matches!(
                            self.capture_phase,
                            Some(
                                CapturePhase::Recording
                                    | CapturePhase::RecordingPaused
                                    | CapturePhase::RecordingFailed
                            )
                        ) =>
                {
                    self.hud_action_started();
                    self.recording_delete_confirmation = false;
                    self.capture_phase = Some(CapturePhase::RecordingDiscarding);
                    self.status = "Discarding recording…".into();
                    self.recording_worker
                        .send(recording::Command::Discard { generation });
                }
                SelectorMessage::RequestDeleteRecording { generation }
                    if self.flow.as_ref().map(CaptureFlow::generation) == Some(generation)
                        && matches!(
                            self.capture_phase,
                            Some(
                                CapturePhase::Recording
                                    | CapturePhase::RecordingPaused
                                    | CapturePhase::RecordingFailed
                            )
                        ) =>
                {
                    self.recording_restart_confirmation = false;
                    self.recording_delete_confirmation = true;
                    request_hidden_root_paint(ctx);
                }
                SelectorMessage::CancelDeleteRecording { generation }
                    if self.flow.as_ref().map(CaptureFlow::generation) == Some(generation) =>
                {
                    self.recording_delete_confirmation = false;
                    request_hidden_root_paint(ctx);
                }
                SelectorMessage::HideRecordingControls { generation }
                    if self.flow.as_ref().map(CaptureFlow::generation) == Some(generation)
                        && self.recording_restore_available
                        && matches!(
                            self.capture_phase,
                            Some(CapturePhase::Recording | CapturePhase::RecordingPaused)
                        ) =>
                {
                    self.hud_action_started();
                    self.recording_controls_hidden = Some(generation);
                    self.recording_hidden_notice_until = Some(
                        Instant::now()
                            + Duration::from_secs_f64(RECORDING_HIDDEN_NOTICE_MS / 1000.),
                    );
                    self.recording_restart_confirmation = false;
                    self.recording_delete_confirmation = false;
                    request_hidden_root_paint(ctx);
                    ctx.request_repaint_after(Duration::from_secs_f64(
                        RECORDING_HIDDEN_NOTICE_MS / 1000.,
                    ));
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
                        self.controls_error.clone_from(&self.error);
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
                | SelectorMessage::RequestDeleteRecording { .. }
                | SelectorMessage::CancelDeleteRecording { .. }
                | SelectorMessage::HideRecordingControls { .. }
                | SelectorMessage::SwitchControlsDisplay { .. }
                | SelectorMessage::OpenPreference { .. }
                | SelectorMessage::Cancel { .. } => {}
            }
        }
        while let Ok(message) = self.preview_rx.try_recv() {
            match message {
                PreviewMessage::DragStarted {
                    artifact_id,
                    generation,
                } if self.previews.accepts(&artifact_id, generation) => {
                    let card = self.previews.cards.get_mut(&artifact_id).unwrap();
                    card.busy = Some(crate::mini_preview::Busy::Drag);
                    card.message = None;
                }
                PreviewMessage::DragFinished {
                    artifact_id,
                    generation,
                    result,
                } if self.previews.accepts(&artifact_id, generation) => {
                    let card = self.previews.cards.get_mut(&artifact_id).unwrap();
                    card.busy = None;
                    card.message = result.as_ref().err().cloned();
                    card.rejected_at = matches!(
                        result,
                        Ok(captures_app::preview::PreviewDragOutcome::Reject)
                    )
                    .then(Instant::now);
                    if matches!(
                        result,
                        Ok(captures_app::preview::PreviewDragOutcome::Dismiss)
                    ) {
                        self.previews.dismiss(&artifact_id, generation);
                    }
                    request_hidden_root_paint(ctx);
                    ctx.request_repaint();
                }
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
                        card.message = None;
                        card.image_path.clone()
                    };
                    self.pending += 1;
                    let _ = self.tx.send(Job::Copy {
                        path,
                        owner: Some(artifact_id.clone()),
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
                        if card.busy.is_some() || card.saved_path.is_some() {
                            continue;
                        }
                        card.busy = Some(crate::mini_preview::Busy::Save);
                        card.message = None;
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
                PreviewMessage::Reveal {
                    artifact_id,
                    generation,
                    path,
                } if self.previews.accepts(&artifact_id, generation) => {
                    let card = self
                        .previews
                        .cards
                        .get_mut(&artifact_id)
                        .expect("accepted preview exists");
                    if card.busy.is_some() || card.saved_path.as_ref() != Some(&path) {
                        continue;
                    }
                    card.busy = Some(crate::mini_preview::Busy::Reveal);
                    card.message = Some("Showing saved file in folder…".into());
                    self.pending += 1;
                    let _ = self.tx.send(Job::Reveal {
                        path,
                        preview: PreviewGuard {
                            artifact_id,
                            generation,
                        },
                    });
                }
                PreviewMessage::Trash {
                    artifact_id,
                    generation,
                    saved_path,
                } if self.previews.accepts(&artifact_id, generation) => {
                    let card = self
                        .previews
                        .cards
                        .get_mut(&artifact_id)
                        .expect("accepted preview exists");
                    if card.busy.is_some() || card.saved_path != saved_path {
                        continue;
                    }
                    card.busy = Some(crate::mini_preview::Busy::Trash);
                    card.message = Some("Moving saved export to Trash…".into());
                    self.send_preview(
                        Request::TrashPreview {
                            root: self.root.clone(),
                            id: artifact_id.clone(),
                            saved_path,
                        },
                        PreviewGuard {
                            artifact_id,
                            generation,
                        },
                    );
                }
                PreviewMessage::Edit {
                    artifact_id,
                    generation,
                    directory,
                } if self.previews.accepts(&artifact_id, generation) => {
                    if self.previews.cards[&artifact_id].busy.is_some()
                        || self.pending > 0
                        || self.recovery.blocking()
                        || self.permission_recovery_visible
                    {
                        continue;
                    }
                    let Some(artifact) = self.artifacts.iter().find(|item| {
                        item.entry.id == artifact_id
                            && item.entry.kind == captures_history::ArtifactKind::Screenshot
                    }) else {
                        self.previews.cards.get_mut(&artifact_id).unwrap().message =
                            Some("Capture is no longer available".into());
                        continue;
                    };
                    let mode = artifact
                        .entry
                        .mode
                        .unwrap_or(captures_capture::CaptureMode::Region);
                    self.open_screenshot_editor(ctx, artifact_id, directory, mode);
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
                PreviewMessage::MoveStack {
                    artifact_id,
                    generation,
                    position,
                } if self.previews.accepts(&artifact_id, generation)
                    && self.previews.stack.ids().last() == Some(&artifact_id) =>
                {
                    self.previews.move_stack(position);
                    request_hidden_root_paint(ctx);
                    ctx.request_repaint();
                }
                PreviewMessage::ClearAll { artifact_ids } => {
                    if self.previews.clear(&artifact_ids) > 0 {
                        request_hidden_root_paint(ctx);
                        ctx.request_repaint();
                    }
                }
                PreviewMessage::DragStarted { .. }
                | PreviewMessage::DragFinished { .. }
                | PreviewMessage::Copy { .. }
                | PreviewMessage::Save { .. }
                | PreviewMessage::Reveal { .. }
                | PreviewMessage::Trash { .. }
                | PreviewMessage::MoveStack { .. }
                | PreviewMessage::Edit { .. } => {}
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
                recording::Event::Snapshot {
                    generation,
                    microphone_peak,
                    result,
                } if self.flow.as_ref().map(CaptureFlow::generation) == Some(generation) => {
                    self.recording_snapshot_poll_pending = false;
                    let hud_changed = match result {
                        Ok(snapshot) => {
                            let warning_changed = self.recording_hud_error.apply(
                                captures_app::recording_hud::ErrorEvent::Warning {
                                    warning: snapshot.warning.clone(),
                                },
                            );
                            let changed = self.recording_microphone_peak != microphone_peak
                                || warning_changed;
                            self.recording_microphone_peak = microphone_peak;
                            self.recording_segment_started =
                                snapshot_interpolation_origin(snapshot.state, Instant::now());
                            self.recording_snapshot = Some(snapshot);
                            changed
                        }
                        Err(error) => {
                            let changed = self.recording_microphone_peak != 0.;
                            self.recording_microphone_peak = 0.;
                            self.error = Some(error);
                            changed
                        }
                    };
                    if hud_changed
                        && recording_hud_state(self.capture_phase, self.recording_has_started)
                            .is_some()
                        && self.recording_controls_hidden != Some(generation)
                        && self.recording_screenshot_flow.is_none()
                    {
                        // The hidden root must replace the deferred HUD callback;
                        // repainting its child alone would retain the old sample.
                        request_hidden_root_paint(ctx);
                        ctx.request_repaint_of(egui::ViewportId::from_hash_of(
                            "recording-controls",
                        ));
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
                            self.recording_hud_error
                                .apply(captures_app::recording_hud::ErrorEvent::SessionChanged);
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
                            self.recording_hud_error.apply(
                                captures_app::recording_hud::ErrorEvent::Warning {
                                    warning: snapshot.warning.clone(),
                                },
                            );
                            self.recording_snapshot = Some(snapshot);
                            self.recording_microphone_peak = 0.;
                            self.recording_segment_started = Some(Instant::now());
                            self.recording_snapshot_poll_pending = false;
                            self.recording_last_snapshot_poll = Instant::now();
                            self.previews.restore_capture();
                            self.capture_phase = Some(CapturePhase::Recording);
                            self.status = "Recording in progress".into();
                            request_hidden_root_paint(ctx);
                        }
                        Err(failure) => {
                            let current = self.flow.as_ref().is_some_and(CaptureFlow::is_current);
                            let snapshot = failure.snapshot.map(|snapshot| *snapshot);
                            match snapshot {
                                Some(snapshot)
                                    if snapshot.state == RecordingState::Failed
                                        && !self.recording_has_started
                                        && current =>
                                {
                                    self.enter_failed_recording(ctx, snapshot, failure.error);
                                    continue;
                                }
                                Some(snapshot)
                                    if snapshot.state == RecordingState::Paused
                                        && self.recording_has_started
                                        && current =>
                                {
                                    // Shipping leaves a take paused when resume cannot
                                    // reopen the engine; the error shows on the HUD.
                                    self.recording_snapshot = Some(snapshot);
                                    self.recording_segment_started = None;
                                    self.capture_phase = Some(CapturePhase::RecordingPaused);
                                    self.status = "Recording paused; resume failed.".into();
                                    self.hud_action_failed(ctx, failure.error);
                                    continue;
                                }
                                _ => {}
                            }
                            self.error = Some(failure.error);
                            if snapshot
                                .is_some_and(|snapshot| snapshot.state == RecordingState::Failed)
                            {
                                // Failed sessions retain their completed media for
                                // recovery; stop/finalize would only replace this error.
                                self.finish_capture(ctx, false);
                            } else if self.recording_has_started {
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
                            self.recording_microphone_peak = 0.;
                            self.recording_segment_started = None;
                            self.capture_phase = Some(CapturePhase::RecordingPaused);
                            self.status = "Recording paused".into();
                            request_hidden_root_paint(ctx);
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
                            self.recording_microphone_peak = 0.;
                            request_hidden_root_paint(ctx);
                        }
                        Err(failure)
                            if self.recording_has_started
                                && self.flow.as_ref().is_some_and(CaptureFlow::is_current)
                                && failure.snapshot.as_ref().is_some_and(|snapshot| {
                                    snapshot.state == RecordingState::Paused
                                }) =>
                        {
                            // Shipping keeps the take paused when the replacement
                            // segment cannot open (for example, the microphone is
                            // gone) and shows the error on the HUD.
                            let snapshot = *failure.snapshot.expect("guarded snapshot");
                            self.recording_snapshot = Some(snapshot);
                            self.recording_microphone_peak = 0.;
                            self.recording_segment_started = None;
                            self.capture_phase = Some(CapturePhase::RecordingPaused);
                            self.status = "Recording paused; the microphone change failed.".into();
                            self.hud_action_failed(ctx, failure.error);
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
                            self.recording_microphone_peak = 0.;
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
                            let id = finalized.entry.id.clone();
                            let target = self.countdown_target;
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
                            // Shipping opens the recording editor when the
                            // preference is on; the saved notice follows its close.
                            if let Some(directory) = self.open_editor_after_recording.take() {
                                self.recording_editor_notice_targets
                                    .insert(id.clone(), target);
                                self.open_recording_editor(ctx, id, directory);
                            }
                            request_hidden_root_paint(ctx);
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
                recording::Event::Retired => {
                    self.pending = self.pending.saturating_sub(1);
                    self.recovery.refresh();
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
                let remaining = flow.countdown().remaining(Instant::now());
                if remaining > 0
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
                    && let Some(target) = self.countdown_target
                {
                    let (title, kind) = main_countdown_presentation(self.capture_phase);
                    self.countdown_exit = Some(CountdownExit::new(
                        main_countdown_viewport(),
                        title,
                        target,
                        kind,
                        remaining,
                    ));
                }
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
                let live_microphone = self.capture_phase == Some(CapturePhase::Recording)
                    && self.recording_controls_hidden != Some(flow.generation())
                    && self.recording_screenshot_flow.is_none()
                    && !self.recording_restart_confirmation
                    && !self.recording_delete_confirmation
                    && self.recording_snapshot.as_ref().is_some_and(|snapshot| {
                        snapshot.options.audio.microphone_device_id.is_some()
                            && !snapshot.options.audio.microphone_muted
                    });
                let snapshot_interval =
                    Duration::from_millis(if live_microphone { 100 } else { 250 });
                if matches!(
                    self.capture_phase,
                    Some(CapturePhase::Recording | CapturePhase::RecordingPaused)
                ) && !self.recording_snapshot_poll_pending
                    && self.recording_last_snapshot_poll.elapsed() >= snapshot_interval
                {
                    self.recording_snapshot_poll_pending = true;
                    self.recording_last_snapshot_poll = Instant::now();
                    self.recording_worker.send(recording::Command::Snapshot {
                        generation: flow.generation(),
                    });
                }
                // Only active captures poll; settled history/preferences stay event-driven.
                // Visible microphones sample at 10Hz with at most one request in flight;
                // paused/hidden/muted recordings retain the existing warning cadence.
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
                let remaining = flow.countdown().remaining(Instant::now());
                if remaining > 0
                    && let Some(RecordingScreenshotPhase::Countdown { .. }) =
                        self.recording_screenshot_phase
                    && let Some(target) = self.countdown_target
                {
                    self.countdown_exit = Some(CountdownExit::new(
                        recording_screenshot_countdown_viewport(flow.generation()),
                        "Captures Screenshot Countdown",
                        target,
                        crate::countdown::Kind::Screenshot,
                        remaining,
                    ));
                }
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
                        self.recording_screenshot_texture = None;
                        self.recording_screenshot_phase =
                            Some(RecordingScreenshotPhase::RetiringCaptureUi {
                                rect,
                                after_countdown,
                                omitted_frame: ctx.cumulative_frame_nr(),
                            });
                        request_hidden_root_paint(ctx);
                        ctx.request_repaint();
                    }
                    Some(
                        RecordingScreenshotPhase::RetiringCaptureUi { .. }
                        | RecordingScreenshotPhase::SettlingCaptureUi { .. },
                    ) => {
                        let ready = self.recording_screenshot_phase.as_mut().and_then(|phase| {
                            phase.capture_after_hide(ctx.cumulative_frame_nr(), Instant::now())
                        });
                        if let Some((rect, after_countdown)) = ready {
                            let Some(session) = self.recording_screenshot_session.take() else {
                                self.error =
                                    Some("Region preparation was lost before capture.".into());
                                self.finish_recording_screenshot(ctx, false);
                                return;
                            };
                            self.pending += 1;
                            let _ = self.tx.send(Job::CaptureRegion {
                                root: self.root.clone(),
                                generation,
                                session,
                                rect,
                                after_countdown,
                            });
                        } else {
                            request_hidden_root_paint(ctx);
                            ctx.request_repaint_after(Duration::from_millis(16));
                        }
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
                Reply::MediaOpened {
                    path,
                    result,
                    output_directory,
                } => self.media_opened(ctx, &path, result, output_directory),
                Reply::HistoryLoaded {
                    result,
                    open_recording,
                } => {
                    self.pending = self.pending.saturating_sub(1);
                    let open_recording = open_recording
                        .filter(|(_, _, generation)| self.selection.accepts(*generation));
                    self.history_loaded = true;
                    match result {
                        Ok(artifacts) => {
                            self.apply(Response::History { artifacts }, false);
                            if let Some((id, directory, _)) = open_recording
                                && self
                                    .artifacts
                                    .iter()
                                    .any(|artifact| artifact.entry.id == id)
                            {
                                self.select(id.clone());
                                self.open_recording_editor(ctx, id, directory);
                            }
                        }
                        Err(error) => {
                            self.error = Some(captures_app::history_view::load_error(&error));
                        }
                    }
                }
                Reply::HistoryCleared(result) => {
                    self.pending = self.pending.saturating_sub(1);
                    self.clearing_history = false;
                    match result {
                        Ok(response) => self.apply(*response, true),
                        Err(error) => {
                            // Some files may already have been deleted. Refresh the
                            // remaining history without concealing the operation error.
                            self.load_history();
                            self.error = Some(captures_app::history_view::clear_error(&error));
                        }
                    }
                }
                Reply::Copied {
                    preview,
                    owner,
                    result,
                } => {
                    self.pending = self.pending.saturating_sub(1);
                    if let (Ok(write), Some(owner)) = (&result, owner) {
                        self.clipboard
                            .record(write.revision, owner, write.fingerprint);
                        request_hidden_root_paint(ctx);
                    }
                    if let Some(preview) = preview {
                        if let Some(card) = self
                            .previews
                            .cards
                            .get_mut(&preview.artifact_id)
                            .filter(|card| card.generation == preview.generation)
                        {
                            card.busy = None;
                            // Success shows the shipping clipboard confirmation chip.
                            card.message = result
                                .as_ref()
                                .err()
                                .map(|error| format!("Copy failed: {error}"));
                        }
                    } else {
                        match result {
                            Ok(_) => self.status = "Copied actual capture pixels".into(),
                            Err(error) => self.error = Some(error),
                        }
                    }
                }
                Reply::ClipboardVerified {
                    verification,
                    result,
                } => match result {
                    Ok(true) => {}
                    Ok(false) => {
                        if crate::clipboard_revision::invalidate(verification.revision)
                            && self.clipboard.clear_if_revision(verification.revision)
                        {
                            request_hidden_root_paint(ctx);
                        }
                    }
                    Err(error) => eprintln!("Could not verify the clipboard owner: {error}"),
                },
                Reply::Revealed { preview, result } => {
                    self.pending = self.pending.saturating_sub(1);
                    if let Some(card) = self
                        .previews
                        .cards
                        .get_mut(&preview.artifact_id)
                        .filter(|card| card.generation == preview.generation)
                    {
                        card.busy = None;
                        card.message = result.err().map(|error| format!("Reveal failed: {error}"));
                    }
                }
                Reply::Executed {
                    preview,
                    notice,
                    result,
                } => {
                    self.pending = self.pending.saturating_sub(1);
                    if self.capture_phase == Some(CapturePhase::DisplayCapturing) {
                        self.capture_in_flight = false;
                        let captured = matches!(result.as_deref(), Ok(Response::Captured { .. }));
                        self.finish_capture(ctx, captured);
                    }
                    match result {
                        Err(error) => {
                            if let Some(guard) = notice {
                                if self.recording_notice.as_mut().is_some_and(|current| {
                                    current.save_result(&guard, Err(error.clone()), Instant::now())
                                }) {
                                    request_hidden_root_paint(ctx);
                                }
                                continue;
                            }
                            if let Some(preview) = preview {
                                if let Some(card) = self
                                    .previews
                                    .cards
                                    .get_mut(&preview.artifact_id)
                                    .filter(|card| card.generation == preview.generation)
                                {
                                    let action =
                                        if card.busy == Some(crate::mini_preview::Busy::Trash) {
                                            "Trash"
                                        } else {
                                            "Save"
                                        };
                                    card.busy = None;
                                    card.message = Some(format!("{action} failed: {error}"));
                                }
                            } else {
                                self.error = Some(error);
                            }
                        }
                        Ok(response) => {
                            if let Some(guard) = notice {
                                let path = match response.as_ref() {
                                    Response::Saved { artifact, path }
                                        if artifact.entry.id == guard.artifact_id =>
                                    {
                                        Some(path.clone())
                                    }
                                    _ => None,
                                };
                                self.apply(*response, false);
                                if let Some(path) = path
                                    && self.recording_notice.as_mut().is_some_and(|current| {
                                        current.save_result(&guard, Ok(path), Instant::now())
                                    })
                                {
                                    request_hidden_root_paint(ctx);
                                }
                                continue;
                            }
                            if let Some(guard) = &preview
                                && let Response::PreviewTrashed { id } = response.as_ref()
                            {
                                if *id == guard.artifact_id {
                                    self.previews.dismiss(id, guard.generation);
                                    request_hidden_root_paint(ctx);
                                }
                                continue;
                            }
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
                                card.message = None;
                                card.saved_at = Some(Instant::now());
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
                Reply::ThumbnailDecoded {
                    id,
                    key,
                    missing,
                    result,
                } => {
                    // Accept only the request for the entry version still listed.
                    if let Some(entry) = self
                        .history_thumbnails
                        .get_mut(&id)
                        .filter(|entry| entry.key == key)
                    {
                        entry.missing = missing;
                        entry.thumbnail = match result {
                            Ok(decoded) => crate::history::Thumbnail::Ready(ctx.load_texture(
                                format!("history:{id}"),
                                decoded.image,
                                egui::TextureOptions::LINEAR,
                            )),
                            Err(_) => crate::history::Thumbnail::Failed,
                        };
                        self.history_cards.remove(&id);
                    }
                }
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
                        card.arrived_at = Some(Instant::now());
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
        self.start_next_media();
    }

    /// Shipping clears the HUD error line whenever a HUD action starts.
    fn hud_action_started(&mut self) {
        self.recording_hud_error
            .apply(captures_app::recording_hud::ErrorEvent::ActionStarted);
    }

    /// Show a HUD action or engine failure on the HUD's inline error line.
    fn hud_action_failed(&mut self, ctx: &egui::Context, message: String) {
        self.recording_hud_error
            .apply(captures_app::recording_hud::ErrorEvent::ActionFailed { message });
        request_hidden_root_paint(ctx);
        ctx.request_repaint_of(egui::ViewportId::from_hash_of("recording-controls"));
    }

    /// Replace the take with a fresh countdown (confirmed Restart, or Retry
    /// recording on a failed take). The same flow rearms Escape first.
    fn restart_recording(&mut self, ctx: &egui::Context, generation: u64) {
        let seconds = self
            .recording_snapshot
            .as_ref()
            .map(|snapshot| snapshot.options.countdown_seconds)
            .unwrap_or(0);
        let Some(flow) = &mut self.flow else {
            return;
        };
        match flow.restart_countdown(seconds) {
            Ok(()) => {
                self.hud_action_started();
                self.recording_controls_hidden = None;
                self.recording_hidden_notice_until = None;
                self.capture_phase = Some(CapturePhase::RecordingRestarting);
                self.status = "Restarting recording…".into();
                self.recording_worker
                    .send(recording::Command::Restart { generation });
                request_hidden_root_paint(ctx);
            }
            Err(error) => {
                self.hud_action_failed(
                    ctx,
                    format!("Could not arm restarted recording Escape: {error}"),
                );
            }
        }
    }

    /// The initial engine start failed. Keep the take and HUD like shipping:
    /// the error inline, Retry recording and Delete. Escape is released because
    /// nothing is counting down; Retry rearms it for the new countdown.
    fn enter_failed_recording(
        &mut self,
        ctx: &egui::Context,
        snapshot: RecordingSessionSnapshot,
        error: String,
    ) {
        if let Some(flow) = &mut self.flow
            && let Err(disarm) = flow.disarm_escape()
        {
            self.status = format!("Recording failed; Escape could not be released: {disarm}");
        } else {
            self.status =
                "Recording failed. Retry or delete it from the recording controls.".into();
        }
        self.recording_snapshot = Some(snapshot);
        self.recording_microphone_peak = 0.;
        self.recording_segment_started = None;
        self.recording_snapshot_poll_pending = false;
        self.recording_controls_hidden = None;
        self.recording_hidden_notice_until = None;
        self.capture_phase = Some(CapturePhase::RecordingFailed);
        self.hud_action_failed(ctx, error);
    }

    fn start_recording_screenshot(&mut self, ctx: &egui::Context) {
        let Some(parent) = self.flow.as_ref() else {
            return;
        };
        let Some(settings) = self.recording_screenshot_settings.as_ref().cloned() else {
            self.hud_action_failed(
                ctx,
                "Screenshot preferences are unavailable for this recording.".into(),
            );
            return;
        };
        let Some(target) = self.countdown_target else {
            self.hud_action_failed(ctx, "The recording display is no longer available.".into());
            return;
        };
        let flow = match parent.begin_recording_screenshot(0) {
            Ok(flow) => flow,
            Err(error) => {
                self.hud_action_failed(
                    ctx,
                    format!("Could not start recording screenshot: {error}"),
                );
                return;
            }
        };
        if let Err(error) =
            self.previews
                .begin_capture(&settings, Some(target), ctx.cumulative_frame_nr())
        {
            flow.cancel();
            self.hud_action_failed(ctx, error);
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
        if is_recording_phase(self.capture_phase) {
            // A failed session deliberately holds its recovery-root lease until
            // the owner is dropped. List only after that worker acknowledges it.
            self.pending += 1;
            self.recording_worker.send(recording::Command::Retire);
        }
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
        self.recording_microphone_peak = 0.;
        self.recording_hud_error = Default::default();
        self.recording_segment_started = None;
        self.recording_snapshot_poll_pending = false;
        self.recording_has_started = false;
        self.recording_restart_confirmation = false;
        self.recording_delete_confirmation = false;
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
        self.select(id.clone());
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
                owner: Some(id),
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
                self.confirm_clear_history = None;
                self.confirm_delete = None;
                self.history_loaded = true;
                for card in self.previews.cards.values_mut() {
                    if let Some(artifact) = artifacts
                        .iter()
                        .find(|artifact| artifact.entry.id == card.artifact_id)
                    {
                        card.saved_path = artifact.entry.saved_path.as_deref().map(PathBuf::from);
                    }
                }
                self.artifacts = artifacts;
                self.refresh_history_selection();
                self.status = self
                    .history_refresh_status
                    .take()
                    .unwrap_or_else(|| "History loaded".into());
            }
            Response::Captured { artifact } => {
                self.accept_artifact(artifact, "Full display captured as PNG");
            }
            Response::Saved { artifact, path } => {
                let artifact_id = artifact.entry.id.clone();
                if announce && let Some(notice) = &mut self.recording_notice {
                    notice.mark_saved(&artifact_id, path.clone(), Instant::now());
                }
                if let Some(item) = self
                    .artifacts
                    .iter_mut()
                    .find(|item| item.entry.id == artifact_id)
                {
                    *item = artifact;
                }
                if let Some(card) = self.previews.cards.get_mut(&artifact_id) {
                    card.saved_path = Some(path.clone());
                }
                self.invalidate_history_cards();
                if announce {
                    self.status = format!("Saved {}", path.display());
                }
            }
            Response::PreviewTrashed { id } => {
                self.previews.remove(&id);
                if announce {
                    self.status = "Preview removed; the private History copy was kept".into();
                }
            }
            Response::Deleted { id } => {
                self.previews.remove(&id);
                self.artifacts.retain(|item| item.entry.id != id);
                self.confirm_delete = None;
                self.refresh_history_selection();
                self.status = "Removed from capture history; exported files were kept".into();
            }
            Response::PermissionGranted => {
                self.status = "Capture permission granted".into();
                self.send(Request::Displays);
            }
            Response::HistoryRoot { .. }
            | Response::OpenedImage { .. }
            | Response::OpenedMedia { .. }
            | Response::PreviewDragPrepared { .. }
            | Response::PreviousPreviewDragsCleared => {}
        }
    }

    pub fn viewports(
        &mut self,
        ctx: &egui::Context,
        tokens: &Tokens,
        settings: Result<AppSettings, String>,
        reduced_motion: bool,
    ) {
        for editor in self.editors.values() {
            editor.show(ctx, tokens);
        }
        for editor in self.recording_editors.values() {
            editor.show(ctx, tokens);
        }
        self.capture_viewports(ctx, tokens, reduced_motion);
        while let Ok(action) = self.notice_rx.try_recv() {
            use crate::recording_saved_notice::Action;
            match action {
                Action::Dismiss(guard)
                    if self
                        .recording_notice
                        .as_ref()
                        .is_some_and(|n| n.guard == guard) =>
                {
                    self.recording_notice = None;
                    self.recording_notice_target = None;
                }
                Action::Reveal(guard, path)
                    if self
                        .recording_notice
                        .as_ref()
                        .is_some_and(|n| n.guard == guard) =>
                {
                    if let Err(error) = reveal(&path) {
                        if let Some(notice) = &mut self.recording_notice {
                            notice.fail(
                                format!("Could not show the recording in its folder: {error}"),
                                Instant::now(),
                            );
                        }
                    } else {
                        self.recording_notice = None;
                        self.recording_notice_target = None;
                    }
                }
                Action::Save(guard)
                    if self
                        .recording_notice
                        .as_ref()
                        .is_some_and(|n| n.guard == guard) =>
                {
                    let directory = settings
                        .as_ref()
                        .map(|s| PathBuf::from(&s.output_directory));
                    match directory {
                        Ok(directory) => {
                            let accepted =
                                self.recording_notice.as_mut().and_then(|n| n.begin_save());
                            if let Some(guard) = accepted {
                                self.pending += 1;
                                let _ = self.tx.send(Job::Execute {
                                    request: Request::SaveRecording {
                                        root: self.root.clone(),
                                        id: guard.artifact_id.clone(),
                                        directory,
                                    },
                                    preview: None,
                                    notice: Some(guard),
                                });
                            }
                        }
                        Err(error) => {
                            if let Some(notice) = &mut self.recording_notice {
                                notice.fail(
                                    format!("Could not save the recording: {error}"),
                                    Instant::now(),
                                );
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        self.recording_notice_viewport(ctx, tokens, reduced_motion);
        if self.flow.is_none()
            && let Ok(settings) = &settings
        {
            if self.previews.show && !settings.show_mini_previews {
                self.previews.stack.set_collapsed(false);
                self.previews.visibility.reset_session_placement();
            }
            if self.previews.placement != settings.mini_preview_placement {
                self.previews.visibility.clear_stack_origin();
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
        let clipboard_owner = self
            .clipboard
            .current_artifact(crate::clipboard_revision::current());
        if clipboard_owner
            .as_ref()
            .is_some_and(|owner| self.previews.cards.contains_key(owner))
        {
            // Notice other apps replacing the clipboard while the chip shows.
            if crate::clipboard_revision::NEEDS_PIXEL_CHECK
                && let Some(verification) = self
                    .clipboard
                    .verification(Instant::now(), CLIPBOARD_CHECK_INTERVAL)
            {
                let _ = self.tx.send(Job::VerifyClipboard(verification));
            }
            request_hidden_root_paint(ctx);
            ctx.request_repaint_after(CLIPBOARD_CHECK_INTERVAL);
        }
        let now = Instant::now();
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
                    size_bytes: card.size_bytes,
                    busy: card.busy,
                    message: card.message.clone(),
                    saved_path: card.saved_path.clone(),
                    saved_at: card
                        .saved_at
                        .filter(|at| now.saturating_duration_since(*at) < SAVED_FEEDBACK),
                    clipboard_current: clipboard_owner.as_deref() == Some(artifact_id.as_str()),
                    rejected_at: card.rejected_at,
                    arrived_at: card.arrived_at,
                    layout: self.previews.stack.card_layout(index, top_anchor)?,
                    hover_y: self
                        .previews
                        .stack
                        .card_layout_hovered(index, top_anchor, true)?
                        .y,
                })
            })
            .collect::<Vec<_>>();
        if cards.is_empty() {
            return;
        }
        let tokens = tokens.clone();
        let drag_root = self.root.clone();
        let geometry = captures_app::preview::thumbnail_geometry(
            preview_bounds,
            count,
            collapsed,
            self.previews.visibility.stack_origin(),
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
                // No idle polling. Only sample while a compact pile has an
                // active pointer gesture; local winit positions can lag a move.
                let desktop_pointer = if collapsed && ui.input(|input| input.pointer.primary_down())
                {
                    captures_capture::pointer_position().map(|(x, y)| {
                        let scale = preview_bounds.scale_factor.max(1.) as f32;
                        egui::pos2(x as f32 / scale, y as f32 / scale)
                    })
                } else {
                    None
                };
                let arrive = tokens.motion(captures_app::motion::Motion::PreviewCardArrive);
                let mut show_card = |ui: &mut egui::Ui, card: &PreviewRenderCard| {
                    // Shipping `thumbnail-arrive`: the card rises and fades in.
                    let arrival = card
                        .arrived_at
                        .map_or(captures_app::motion::Pose::REST, |at| {
                            let elapsed = crate::motion::elapsed_ms(at, Instant::now());
                            if arrive.running(elapsed, reduced_motion) {
                                ui.ctx().request_repaint();
                            }
                            arrive.pose_at(elapsed, reduced_motion)
                        });
                    let card_rect = egui::Rect::from_min_size(
                        ui.cursor().min,
                        egui::vec2(
                            ui.available_width(),
                            captures_app::preview::THUMBNAIL_CARD_HEIGHT as f32,
                        ),
                    );
                    let reject_offset = card.rejected_at.map_or(0., |start| {
                        let elapsed = start.elapsed().as_secs_f32();
                        if !reduced_motion && elapsed < 0.420 {
                            ui.ctx().request_repaint();
                        }
                        crate::mini_preview::reject_offset(elapsed, reduced_motion)
                    });
                    if let Some(saved_at) = card.saved_at {
                        // Clear "Saved" when the shipping confirmation window ends.
                        request_hidden_root_paint(ui.ctx());
                        ui.ctx().request_repaint_after(
                            SAVED_FEEDBACK.saturating_sub(saved_at.elapsed()),
                        );
                    }
                    let action = crate::motion::with_pose(ui, arrival, card_rect, |ui| {
                        crate::mini_preview::show(
                            ui,
                            &tokens,
                            crate::mini_preview::View {
                                artifact_id: &card.artifact_id,
                                texture: &card.texture,
                                width: card.width,
                                height: card.height,
                                size_bytes: card.size_bytes,
                                busy: card.busy,
                                message: card.message.as_deref(),
                                clipboard_current: card.clipboard_current,
                                saved_feedback: card.saved_at.is_some(),
                                can_save: save.is_some(),
                                saved: card.saved_path.is_some(),
                                interactive: card.layout.interactive,
                                collapsed,
                                stack_count: count,
                                depth: card.layout.depth,
                                desktop_pointer,
                                reject_offset,
                                right_anchor: placement.is_right(),
                            },
                        )
                    });
                    let next_message = match action {
                        Some(crate::mini_preview::Action::DragFile) => {
                            let artifact_id = card.artifact_id.clone();
                            let generation = card.generation;
                            let completed = sender.clone();
                            let wake = ui.ctx().clone();
                            crate::outbound_drag::request(
                                ui.ctx(),
                                drag_root.clone(),
                                artifact_id.clone(),
                                Box::new(move |result| {
                                    let _ = completed.send(PreviewMessage::DragFinished {
                                        artifact_id,
                                        generation,
                                        result,
                                    });
                                    request_hidden_root_paint(&wake);
                                    wake.request_repaint();
                                }),
                            )
                            .then(|| PreviewMessage::DragStarted {
                                artifact_id: card.artifact_id.clone(),
                                generation,
                            })
                        }
                        Some(crate::mini_preview::Action::ExpandStack) => {
                            Some(PreviewMessage::ToggleCollapsed)
                        }
                        Some(crate::mini_preview::Action::MoveStack(position)) => {
                            Some(PreviewMessage::MoveStack {
                                artifact_id: card.artifact_id.clone(),
                                generation: card.generation,
                                position,
                            })
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
                        Some(crate::mini_preview::Action::Reveal) => {
                            card.saved_path.as_ref().map(|path| PreviewMessage::Reveal {
                                artifact_id: card.artifact_id.clone(),
                                generation: card.generation,
                                path: path.clone(),
                            })
                        }
                        Some(crate::mini_preview::Action::Trash) => Some(PreviewMessage::Trash {
                            artifact_id: card.artifact_id.clone(),
                            generation: card.generation,
                            saved_path: card.saved_path.clone(),
                        }),
                        Some(crate::mini_preview::Action::Edit) => {
                            save.as_ref().map(|(directory, _)| {
                                // The independently repainting card must wake the
                                // minimized/hidden owner without showing its workspace.
                                request_hidden_root_paint(ui.ctx());
                                PreviewMessage::Edit {
                                    artifact_id: card.artifact_id.clone(),
                                    generation: card.generation,
                                    directory: directory.clone(),
                                }
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
                    let front = cards.iter().find(|card| card.layout.interactive);
                    let front_rect = front.map(|card| {
                        egui::Rect::from_min_size(
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
                        )
                    });
                    let fan_open = front_rect.is_some_and(|rect| {
                        ui.input(|input| {
                            input
                                .pointer
                                .hover_pos()
                                .is_some_and(|point| rect.contains(point))
                                || (input.pointer.primary_down()
                                    && input
                                        .pointer
                                        .press_origin()
                                        .is_some_and(|point| rect.contains(point)))
                        })
                    });
                    // Zero duration also updates the stored endpoint. Bypassing
                    // the animator would revive a stale fan when motion returns.
                    let fan = ui.ctx().animate_bool_with_time(
                        egui::Id::unique("mini-preview-hover-fan"),
                        fan_open,
                        if reduced_motion {
                            0.
                        } else {
                            tokens.number("dur-3") / 1000.
                        },
                    );
                    for card in &cards {
                        let y = egui::lerp(card.layout.y as f32..=card.hover_y as f32, fan);
                        let rect = egui::Rect::from_min_size(
                            egui::pos2(captures_app::preview::THUMBNAIL_PADDING as f32, y),
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
                    // Overflow cues request whole-slot scrolls; apply them
                    // inside the scroll area on the next pass so egui
                    // animates the move and releases stick-to-bottom.
                    let cue_scroll_id = egui::Id::unique("preview-stack-cue-scroll");
                    let cue_scroll = ui.data_mut(|data| data.remove_temp::<f32>(cue_scroll_id));
                    let scroll =
                        ui.scope_builder(egui::UiBuilder::new().max_rect(card_area), |ui| {
                            egui::ScrollArea::vertical()
                                .id_salt("preview-stack-scroll")
                                .auto_shrink([false, false])
                                .stick_to_bottom(!top_anchor)
                                .show(ui, |ui| {
                                    if let Some(delta) = cue_scroll {
                                        ui.scroll_with_delta(egui::vec2(0., -delta));
                                    }
                                    let (content, _) = ui.allocate_exact_size(
                                        egui::vec2(ui.available_width(), scroll_content_height),
                                        egui::Sense::hover(),
                                    );
                                    for card in &cards {
                                        let y = card.layout.y as f32
                                            - if top_anchor { gutter } else { 0. };
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
                                        ui.scope_builder(
                                            egui::UiBuilder::new().max_rect(rect),
                                            |ui| show_card(ui, card),
                                        );
                                    }
                                })
                        });
                    let scroll = scroll.inner;
                    let (offset, content_height, viewport_height) = (
                        f64::from(scroll.state.offset.y),
                        f64::from(scroll.content_size.y),
                        f64::from(scroll.inner_rect.height()),
                    );
                    let overflow = captures_app::preview::stack_overflow(
                        offset,
                        content_height,
                        viewport_height,
                    );
                    // Cues paint over the cards and under the stack toolbar,
                    // matching the shipping z-order.
                    if let Some(slots) = crate::mini_preview::show_overflow_cues(
                        ui,
                        &tokens,
                        egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(
                                captures_app::preview::THUMBNAIL_WIDTH as f32,
                                geometry.height as f32,
                            ),
                        ),
                        overflow,
                        top_anchor,
                    ) {
                        let target = captures_app::preview::stack_scroll_target(
                            offset,
                            content_height,
                            viewport_height,
                            slots,
                        );
                        ui.data_mut(|data| {
                            data.insert_temp(cue_scroll_id, (target - offset) as f32)
                        });
                        ui.ctx().request_repaint();
                    }
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
                            ui,
                            &tokens,
                            placement.is_right(),
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

    fn recording_notice_viewport(
        &self,
        ctx: &egui::Context,
        tokens: &Tokens,
        reduced_motion: bool,
    ) {
        let (Some(notice), Some(target)) = (&self.recording_notice, self.recording_notice_target)
        else {
            return;
        };
        let Some(bounds) = target.preview_bounds else {
            return;
        };
        let scale = bounds.scale_factor.max(1.0) as f32;
        let work_right = (bounds.work_x + bounds.work_width as i32) as f32 / scale;
        let work_top = bounds.work_y as f32 / scale;
        let position = egui::pos2(
            work_right - crate::recording_saved_notice::SIZE.x - 16.,
            work_top + 16.,
        );
        let sender = self.notice_tx.clone();
        let notice = notice.clone();
        let tokens = tokens.clone();
        let builder = egui::ViewportBuilder::default()
            .with_title(notice.title())
            .with_position(position)
            .with_inner_size(crate::recording_saved_notice::SIZE)
            .with_min_inner_size(crate::recording_saved_notice::SIZE)
            .with_max_inner_size(crate::recording_saved_notice::SIZE)
            .with_decorations(false)
            .with_resizable(false)
            .with_transparent(true)
            .with_has_shadow(false)
            .with_always_on_top()
            .with_taskbar(false)
            .with_active(false);
        #[cfg(target_os = "linux")]
        let builder = builder
            .with_window_type(egui::X11WindowType::Notification)
            .with_override_redirect(true);
        let viewport =
            egui::ViewportId::from_hash_of(("recording-saved-notice", notice.guard.generation));
        ctx.show_viewport_deferred(viewport, builder, move |ui, _| {
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::ContentProtected(true));
            if notice.expired(Instant::now())
                || ui.input(|input| input.viewport().close_requested())
            {
                let _ = sender.send(crate::recording_saved_notice::Action::Dismiss(
                    notice.guard.clone(),
                ));
                request_hidden_root_paint(ui.ctx());
                ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
            } else {
                // Shipping `recording-saved-lifecycle`: frames only while it moves.
                let lifecycle =
                    tokens.motion(captures_app::motion::Motion::RecordingSavedLifecycle);
                let (pose, moving) = notice.pose(&lifecycle, Instant::now(), reduced_motion);
                let card = egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    crate::recording_saved_notice::SIZE,
                );
                let action = crate::motion::with_pose(ui, pose, card, |ui| {
                    crate::recording_saved_notice::show(ui, &tokens, &notice)
                });
                if let Some(action) = action {
                    let _ = sender.send(action);
                    request_hidden_root_paint(ui.ctx());
                    ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
                }
                if moving {
                    ui.ctx().request_repaint();
                } else if !reduced_motion
                    && let Some(wake) = notice.exit_wake(&lifecycle, Instant::now())
                    && !wake.is_zero()
                {
                    ui.ctx().request_repaint_after(wake);
                }
            }
            if let Some(remaining) = notice.remaining(Instant::now()) {
                ui.ctx().request_repaint_after(remaining);
            }
        });
        // Worker replies update this deferred callback on the root. Paint the
        // child too: it must not require pointer motion to leave "Saving…".
        // This does not schedule another root frame or an idle repaint loop.
        ctx.request_repaint_of(viewport);
    }

    fn capture_viewports(&mut self, ctx: &egui::Context, t: &Tokens, reduced_motion: bool) {
        // The guide belongs to the recording, not the HUD. Keep it during
        // countdown, pause, restart and Hide; dropping the snapshot closes it.
        if let (Some(snapshot), Some(target)) = (&self.recording_snapshot, self.countdown_target)
            && let RecordingTarget::Region { rect, .. } = snapshot.options.target
            && self.flow.as_ref().is_some_and(CaptureFlow::is_current)
            // Shipping removes the region indicator when a take fails.
            && self.capture_phase != Some(CapturePhase::RecordingFailed)
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
        if let Some((hud_state, phase_busy)) =
            recording_hud_state(self.capture_phase, self.recording_has_started)
            && let (Some(flow), Some(snapshot), Some(target)) = (
                &self.flow,
                self.recording_snapshot.clone(),
                self.countdown_target,
            )
        {
            let generation = flow.generation();
            let running = matches!(
                hud_state,
                RecordingState::Recording | RecordingState::Finalizing
            );
            let restart_confirmation = self.recording_restart_confirmation;
            let delete_confirmation = self.recording_delete_confirmation;
            let controls_hidden = self.recording_controls_hidden == Some(generation)
                || self.recording_screenshot_flow.is_some();
            let hide_available = self.recording_restore_available;
            let busy = restart_confirmation || delete_confirmation || phase_busy;
            let elapsed_ms = interpolated_recording_elapsed(
                snapshot.elapsed_ms,
                self.recording_segment_started
                    .map(|started| started.elapsed()),
            );
            let sender = self.selector_tx.clone();
            let confirmation_sender = self.selector_tx.clone();
            let tokens = t.clone();
            let include_controls = self.include_recording_controls;
            let error_line = self.recording_hud_error.line(snapshot.error.as_deref());
            let microphone_peak = self.recording_microphone_peak;
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
                    let notice = if cfg!(target_os = "linux") || include_controls {
                        "These controls will show in recordings · Use Hide controls to keep them out"
                    } else {
                        "These controls won’t show in recordings"
                    };
                    if let Some(action) = recording_hud::show(
                        ui,
                        &tokens,
                        recording_hud::View {
                            state: hud_state,
                            busy,
                            has_microphone: snapshot.options.audio.microphone_device_id.is_some(),
                            microphone_muted: snapshot.options.audio.microphone_muted,
                            microphone_peak,
                            elapsed_ms,
                            notice,
                            error: error_line.as_deref(),
                            hide_available,
                            reduced_motion,
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
                                SelectorMessage::RequestDeleteRecording { generation }
                            }
                            recording_hud::Action::Hide => {
                                SelectorMessage::HideRecordingControls { generation }
                            }
                        };
                        let _ = sender.send(message);
                        ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
                    }
                    if running {
                        // 30 fps keeps the shipping status pulse smooth; the timer only needs 10.
                        ui.ctx().request_repaint_after(Duration::from_millis(
                            if reduced_motion { 100 } else { 33 },
                        ));
                    }
                },
            );
            if controls_hidden
                && let Some(deadline) = self
                    .recording_hidden_notice_until
                    .filter(|deadline| *deadline > Instant::now())
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
                        // Shipping `recording-controls-hidden-lifecycle` (6 s) ends
                        // 200 ms before the 6.2 s window closes.
                        let lifecycle = notice_tokens.motion(
                            captures_app::motion::Motion::RecordingControlsHiddenLifecycle,
                        );
                        let now = Instant::now();
                        let until = deadline.saturating_duration_since(now).as_secs_f64() * 1000.;
                        let since = RECORDING_HIDDEN_NOTICE_MS - until;
                        let until = (until - 200.).max(0.);
                        let pose = lifecycle.lifecycle_pose(since, Some(until), reduced_motion);
                        if lifecycle.lifecycle_running(since, Some(until), reduced_motion) {
                            ui.ctx().request_repaint();
                        } else if !reduced_motion {
                            ui.ctx().request_repaint_after(Duration::from_secs_f64(
                                lifecycle.lifecycle_exit_in(until) / 1000.,
                            ));
                        }
                        let rect = ui.max_rect();
                        crate::motion::with_pose(ui, pose, rect, |ui| {
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
            if delete_confirmation {
                let tokens = t.clone();
                let sender = self.selector_tx.clone();
                // Shipping `deleteRecording` message dialog.
                ctx.show_viewport_deferred(
                    egui::ViewportId::from_hash_of("recording-delete-confirmation"),
                    egui::ViewportBuilder::default()
                        .with_title("Delete recording?")
                        .with_inner_size([360., 150.])
                        .with_position(position + egui::vec2(35., -170.))
                        .with_always_on_top()
                        .with_resizable(false),
                    move |ui, _| {
                        tokens.glass_controls(ui);
                        if ui.input(|input| input.viewport().close_requested()) {
                            let _ =
                                sender.send(SelectorMessage::CancelDeleteRecording { generation });
                            ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
                            return;
                        }
                        ui.vertical_centered(|ui| {
                            ui.add_space(14.);
                            ui.heading("Delete recording?");
                            ui.label("This recording will be deleted permanently.");
                            ui.add_space(10.);
                            ui.horizontal(|ui| {
                                if ui.button("Cancel").clicked() {
                                    let _ = sender.send(SelectorMessage::CancelDeleteRecording {
                                        generation,
                                    });
                                }
                                if ui.button("Delete").clicked() {
                                    let _ = sender
                                        .send(SelectorMessage::DiscardRecording { generation });
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
            let controls_error = self.controls_error.clone();
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
                            error: controls_error.as_deref(),
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
                            capture_controls::Action::OpenPreference(target) => {
                                SelectorMessage::OpenPreference { generation, target }
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
            // Shipping direct overlays commit on release; auto-start applies
            // only to the New Capture menu.
            let (texture, display) = if recording_screenshot {
                (
                    self.recording_screenshot_texture.clone(),
                    self.recording_screenshot_session
                        .as_ref()
                        .expect("recording screenshot selection owns region session")
                        .display(),
                )
            } else {
                (
                    self.region_texture.clone(),
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
            // Shipping direct window overlay commits the clicked target.
            let auto_start = true;
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
        let mut countdown_declared = false;
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
                    recording_screenshot_countdown_viewport(generation),
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
                        crate::countdown::show(
                            ui,
                            &t,
                            clock.remaining(Instant::now()).max(1),
                            crate::countdown::Kind::Screenshot,
                            !captures_app::capture_flow::is_current(generation),
                        );
                        ui.ctx().request_repaint_after(Duration::from_millis(100));
                    },
                );
                countdown_declared = true;
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
                let (title, kind) = main_countdown_presentation(self.capture_phase);
                ctx.show_viewport_deferred(
                    main_countdown_viewport(),
                    capture_viewport(title, target.monitor, target.position, target.size),
                    move |ui, _| {
                        if ui.input(|i| {
                            i.viewport().close_requested() || i.key_pressed(egui::Key::Escape)
                        }) {
                            captures_app::capture_flow::cancel(generation);
                            ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
                        }
                        crate::countdown::show(
                            ui,
                            &t,
                            clock.remaining(Instant::now()).max(1),
                            kind,
                            !captures_app::capture_flow::is_current(generation),
                        );
                        ui.ctx().request_repaint_after(Duration::from_millis(100));
                    },
                );
                countdown_declared = true;
            } else {
                // Stop declaring the child before hiding the root. Hidden-root
                // logic then verifies visibility and waits for compositor settling.
                self.hide_for_capture(ctx);
            }
        }
        // A cancelled countdown stays up briefly with "Cancelling…", matching
        // the shipping fade-out window. A new countdown always takes over.
        if let Some(exit) = self.countdown_exit {
            let now = Instant::now();
            if countdown_declared || now >= exit.until {
                self.countdown_exit = None;
            } else {
                let t = t.clone();
                ctx.show_viewport_deferred(
                    exit.viewport,
                    capture_viewport(
                        exit.title,
                        exit.target.monitor,
                        exit.target.position,
                        exit.target.size,
                    ),
                    move |ui, _| {
                        crate::countdown::show(ui, &t, exit.remaining, exit.kind, true);
                    },
                );
                ctx.request_repaint_after(exit.until - now);
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
        if self.flow.is_some() || self.capture_in_flight || self.permission_recovery_visible {
            ui.disable();
        }
        let now = Instant::now();
        self.expire_history_confirmations(ui.ctx(), now);
        // Escape backs out of an armed Delete / Delete all without deleting.
        if (self.confirm_delete.is_some() || self.confirm_clear_history.is_some())
            && !self.recovery.blocking()
            && ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            self.confirm_delete = None;
            self.confirm_clear_history = None;
        }
        if self.pending == 0 {
            self.card_busy = None;
        }
        let copy = captures_app::history_view::copy();
        let busy = self.pending > 0 || self.recovery.blocking();
        let margin = |name: &str| t.number(name) as i8;
        let mut header_event = None;
        egui::Panel::top("live-header")
            .show_separator_line(false)
            .frame(
                egui::Frame::new()
                    .fill(t.color("surface-canvas"))
                    .inner_margin(egui::Margin {
                        left: margin("s-8"),
                        right: margin("s-8"),
                        top: margin("s-8"),
                        bottom: margin("s-5"),
                    }),
            )
            .show(ui, |ui| {
            header_event = crate::history::header(
                ui,
                t,
                crate::history::DeleteAll {
                    visible: self.history_loaded && !self.artifacts.is_empty(),
                    confirming: self.confirm_clear_history.is_some(),
                    busy: self.clearing_history,
                    enabled: !busy,
                },
            );
            ui.add_space(t.number("s-5"));
            let can_start_capture = self.can_start_capture();
            let (action, _) =
                capture_actions(ui, &self.displays, &mut self.display_id, can_start_capture);
            match action {
                Some(CaptureAction::RefreshDisplays) => self.send(Request::Displays),
                Some(CaptureAction::Capture(request)) => {
                    self.request_capture(request);
                    self.launch_requested_capture(ui.ctx(), frame, settings());
                }
                Some(CaptureAction::Permissions) => self.permission_recovery_requested = true,
                None => {}
            }
            if self.can_hide == Some(false) {
                ui.colored_label(t.color("theme-signal"), "Display, region and window capture unavailable: this Wayland backend cannot hide and verify the root window.");
            }
            ui.label(RichText::new(&self.status).small().color(t.color("text-muted")));
        });
        match header_event {
            Some(crate::history::HeaderEvent::DeleteAll) => self.delete_all_history(now),
            Some(crate::history::HeaderEvent::Cancel) => self.confirm_clear_history = None,
            None => {}
        }

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(t.color("surface-canvas"))
                    .inner_margin(egui::Margin {
                        left: margin("s-8"),
                        right: margin("s-8"),
                        top: 0,
                        bottom: 0,
                    }),
            )
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = t.number("s-6");
                if self.history_loaded && !self.artifacts.is_empty() {
                    let mut counts = [0usize; 4];
                    for item in &self.artifacts {
                        for (index, filter) in HistoryFilter::ALL.iter().enumerate() {
                            counts[index] += usize::from(filter.matches(item.entry.kind));
                        }
                    }
                    let options: Vec<_> = HistoryFilter::ALL
                        .iter()
                        .zip(counts)
                        .map(|(filter, count)| {
                            (
                                filter.label(),
                                count,
                                *filter == self.history_filter,
                                !busy && (*filter == HistoryFilter::All || count > 0),
                            )
                        })
                        .collect();
                    let toolbar = ui.scope(|ui| {
                        ui.spacing_mut().item_spacing.y = t.number("s-5");
                        let clicked = crate::history::filters(ui, t, &options);
                        ui.add_space(0.);
                        clicked
                    });
                    let line = toolbar.response.rect.bottom();
                    ui.painter().hline(
                        ui.max_rect().x_range(),
                        line,
                        egui::Stroke::new(1., t.color("border-subtle")),
                    );
                    if let Some(index) = toolbar.inner {
                        self.history_filter = HistoryFilter::ALL[index];
                        self.confirm_clear_history = None;
                        self.refresh_history_selection();
                    }
                }
                if let Some(error) = &self.error {
                    crate::history::error(ui, t, error);
                }
                let recovery_enabled = self.pending == 0
                    && !self.is_capturing()
                    && self.requested_capture.is_none()
                    && self.confirm_clear_history.is_none()
                    && self.confirm_delete.is_none();
                if let Some(target) = self.recovery.ui(ui, t, recovery_enabled) {
                    match settings() {
                        Ok(settings) => {
                            self.recovery_selection = self.selection.generation;
                            self.recovery
                                .recover(target, settings.output_directory.into());
                        }
                        Err(error) => self.error = Some(error),
                    }
                }
                if !self.history_loaded {
                    crate::history::empty(ui, t, copy.loading, None);
                    return;
                }
                if self.artifacts.is_empty() {
                    if !self.recovery.has_drafts() {
                        crate::history::empty(ui, t, copy.empty_title, Some(copy.empty_body));
                    }
                    return;
                }
                let visible: Vec<usize> = self
                    .artifacts
                    .iter()
                    .enumerate()
                    .filter(|(_, item)| self.history_filter.matches(item.entry.kind))
                    .map(|(index, _)| index)
                    .collect();
                if visible.is_empty() {
                    ui.label(RichText::new(copy.filtered_empty).color(t.color("text-subtle")));
                    return;
                }
                for &index in &visible {
                    let artifact = &self.artifacts[index];
                    if !self.history_cards.contains_key(&artifact.entry.id) {
                        let missing = self
                            .history_thumbnails
                            .get(&artifact.entry.id)
                            .is_some_and(|thumbnail| thumbnail.missing);
                        self.history_cards.insert(
                            artifact.entry.id.clone(),
                            captures_app::history_view::card(&artifact.entry, missing),
                        );
                    }
                }
                let scroll_to = self.history_scroll_to.take().and_then(|id| {
                    visible
                        .iter()
                        .position(|index| self.artifacts[*index].entry.id == id)
                });
                let output = {
                    let items: Vec<_> = visible
                        .iter()
                        .map(|&index| {
                            let id = self.artifacts[index].entry.id.as_str();
                            crate::history::Item {
                                id,
                                card: &self.history_cards[id],
                                thumbnail: self
                                    .history_thumbnails
                                    .get(id)
                                    .map(|entry| &entry.thumbnail),
                                selected: self.selection.id.as_deref() == Some(id),
                                confirming_delete: self
                                    .confirm_delete
                                    .as_ref()
                                    .is_some_and(|(armed, _)| armed == id),
                                busy: self
                                    .card_busy
                                    .as_ref()
                                    .filter(|(busy, _)| busy == id)
                                    .map(|(_, action)| *action),
                            }
                        })
                        .collect();
                    crate::history::grid(ui, t, &items, !busy, scroll_to)
                };
                let ids: Vec<String> = visible
                    .iter()
                    .map(|&index| self.artifacts[index].entry.id.clone())
                    .collect();
                for event in output.events {
                    match event {
                        crate::history::Event::NeedsThumbnail(slot) => {
                            self.request_thumbnail(&ids[slot])
                        }
                        crate::history::Event::Select(slot) => self.select_card(&ids[slot]),
                        crate::history::Event::Open(slot) => {
                            self.select_card(&ids[slot]);
                            self.card_action(
                                ui.ctx(),
                                &ids[slot],
                                captures_app::history_view::CardAction::Edit,
                                settings(),
                            );
                        }
                        crate::history::Event::Action(slot, action) => {
                            self.card_action(ui.ctx(), &ids[slot], action, settings());
                        }
                        crate::history::Event::Delete(slot) => self.delete_card(&ids[slot], now),
                    }
                }
                if self.history_thumbnails.len() > HISTORY_THUMBNAIL_CACHE {
                    let resident: std::collections::HashSet<&str> = ids[output.visible.clone()]
                        .iter()
                        .map(String::as_str)
                        .collect();
                    self.history_thumbnails
                        .retain(|id, _| resident.contains(id.as_str()));
                }
                // Keyboard: arrows move the selection (also while actions are
                // busy), Return opens it. Only when no control owns focus, so
                // focused buttons keep their own navigation.
                if ui.ctx().memory(|memory| memory.focused().is_none()) {
                    let key = ui.input(|input| {
                        [
                            (egui::Key::ArrowRight, 1, 0),
                            (egui::Key::ArrowLeft, -1, 0),
                            (egui::Key::ArrowDown, 0, 1),
                            (egui::Key::ArrowUp, 0, -1),
                        ]
                        .into_iter()
                        .find(|(key, _, _)| input.key_pressed(*key))
                    });
                    let current = self
                        .selection
                        .id
                        .as_deref()
                        .and_then(|id| ids.iter().position(|visible| visible == id));
                    if let Some((_, columns, rows)) = key {
                        let layout = captures_app::history_view::grid(output.width);
                        let next =
                            current.map_or(0, |index| layout.step(index, ids.len(), columns, rows));
                        self.select(ids[next].clone());
                    } else if let Some(index) = current.filter(|_| !busy)
                        && ui.input(|input| input.key_pressed(egui::Key::Enter))
                    {
                        let id = ids[index].clone();
                        self.card_action(
                            ui.ctx(),
                            &id,
                            captures_app::history_view::CardAction::Edit,
                            settings(),
                        );
                    }
                }
            });
    }

    /// Revert shipping two-step confirmations after four seconds.
    fn expire_history_confirmations(&mut self, ctx: &egui::Context, now: Instant) {
        let timeout = Duration::from_millis(captures_app::history_view::CONFIRM_TIMEOUT_MS);
        for armed in [
            self.confirm_delete.as_ref().map(|(_, at)| *at),
            self.confirm_clear_history,
        ]
        .into_iter()
        .flatten()
        {
            let remaining = timeout.saturating_sub(now.saturating_duration_since(armed));
            if !remaining.is_zero() {
                ctx.request_repaint_after(remaining);
            }
        }
        if self
            .confirm_delete
            .as_ref()
            .is_some_and(|(_, at)| now.saturating_duration_since(*at) >= timeout)
        {
            self.confirm_delete = None;
        }
        if self
            .confirm_clear_history
            .is_some_and(|at| now.saturating_duration_since(at) >= timeout)
        {
            self.confirm_clear_history = None;
        }
    }

    /// First click arms "Delete all forever"; the second deletes every kind.
    fn delete_all_history(&mut self, now: Instant) {
        if self.pending > 0 || self.recovery.blocking() || self.artifacts.is_empty() {
            return;
        }
        if self.confirm_clear_history.take().is_none() {
            self.confirm_clear_history = Some(now);
            self.confirm_delete = None;
            return;
        }
        self.clearing_history = true;
        self.send(Request::ClearHistory {
            root: self.root.clone(),
        });
    }

    /// Shipping card trash: arm, then delete on a second click within the
    /// timeout. Missing recordings are removed immediately.
    fn delete_card(&mut self, id: &str, now: Instant) {
        if self.pending > 0 || self.recovery.blocking() {
            return;
        }
        let confirm = self
            .history_cards
            .get(id)
            .is_none_or(|card| card.delete_requires_confirmation);
        let armed = self
            .confirm_delete
            .as_ref()
            .is_some_and(|(armed, _)| armed == id);
        if confirm && !armed {
            self.confirm_delete = Some((id.to_owned(), now));
            self.confirm_clear_history = None;
            return;
        }
        self.send(Request::Delete {
            root: self.root.clone(),
            id: id.to_owned(),
        });
    }

    fn select_card(&mut self, id: &str) {
        if self.selection.id.as_deref() != Some(id) {
            self.selection.begin(id.to_owned());
        }
    }

    fn request_thumbnail(&mut self, id: &str) {
        let Some(artifact) = self.artifacts.iter().find(|item| item.entry.id == id) else {
            return;
        };
        let key = ThumbnailKey::of(artifact);
        self.history_thumbnails.insert(
            id.to_owned(),
            HistoryThumbnail {
                key: key.clone(),
                thumbnail: crate::history::Thumbnail::Loading,
                missing: false,
            },
        );
        let _ = self.tx.send(Job::DecodeThumbnail {
            root: self.root.clone(),
            entry: Box::new(artifact.entry.clone()),
            key,
        });
    }

    fn card_action(
        &mut self,
        ctx: &egui::Context,
        id: &str,
        action: captures_app::history_view::CardAction,
        settings: Result<AppSettings, String>,
    ) {
        use captures_app::history_view::CardAction;
        if self.pending > 0 || self.recovery.blocking() {
            return;
        }
        let Some(entry) = self
            .artifact_index(id)
            .map(|index| self.artifacts[index].entry.clone())
        else {
            return;
        };
        let recording = entry.kind.is_recording();
        match action {
            CardAction::Copy => self.copy(id),
            CardAction::ShowInFolder => {
                if let Some(path) = entry.saved_path.as_deref()
                    && let Err(error) = reveal(Path::new(path))
                {
                    self.error = Some(format!("Could not reveal export: {error}"));
                }
            }
            CardAction::Edit => match settings {
                Ok(settings) if recording => {
                    self.open_recording_editor(
                        ctx,
                        id.to_owned(),
                        settings.output_directory.into(),
                    );
                }
                Ok(settings) => {
                    let mode = entry.mode.unwrap_or(captures_capture::CaptureMode::Region);
                    self.open_screenshot_editor(
                        ctx,
                        id.to_owned(),
                        settings.output_directory.into(),
                        mode,
                    );
                }
                Err(error) => self.error = Some(error),
            },
            CardAction::SaveImage | CardAction::SaveFile => match settings {
                Ok(settings) => {
                    self.card_busy = Some((id.to_owned(), action));
                    self.send(if recording {
                        Request::SaveRecording {
                            root: self.root.clone(),
                            id: id.to_owned(),
                            directory: settings.output_directory.into(),
                        }
                    } else {
                        Request::SaveScreenshot {
                            root: self.root.clone(),
                            id: id.to_owned(),
                            directory: settings.output_directory.into(),
                            format: settings.screenshot_format,
                        }
                    });
                }
                Err(error) => self.error = Some(error),
            },
        }
    }

    fn copy(&mut self, id: &str) {
        let Some(index) = self.artifact_index(id) else {
            return;
        };
        if self.artifacts[index].entry.kind.is_recording() {
            return;
        }
        self.pending += 1;
        self.error = None;
        self.card_busy = Some((id.to_owned(), captures_app::history_view::CardAction::Copy));
        let _ = self.tx.send(Job::Copy {
            path: self.artifacts[index].image_path.clone(),
            preview: None,
            owner: Some(id.to_owned()),
        });
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

fn main_countdown_viewport() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("screenshot-countdown")
}

fn recording_screenshot_countdown_viewport(generation: u64) -> egui::ViewportId {
    egui::ViewportId::from_hash_of(("recording-screenshot-countdown", generation))
}

fn main_countdown_presentation(
    phase: Option<CapturePhase>,
) -> (&'static str, crate::countdown::Kind) {
    if phase == Some(CapturePhase::RecordingCountdown) {
        (
            "Captures Recording Countdown",
            crate::countdown::Kind::Recording,
        )
    } else {
        (
            "Captures Screenshot Countdown",
            crate::countdown::Kind::Screenshot,
        )
    }
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

/// The HUD state for a capture phase, and whether an action is in flight.
/// Shipping keeps the HUD up (controls disabled) while pausing, resuming,
/// changing the microphone and saving, and shows failed takes; it is hidden
/// only during countdown and restart.
fn recording_hud_state(
    phase: Option<CapturePhase>,
    has_started: bool,
) -> Option<(RecordingState, bool)> {
    Some(match phase? {
        CapturePhase::Recording => (RecordingState::Recording, false),
        CapturePhase::RecordingPaused => (RecordingState::Paused, false),
        CapturePhase::RecordingPausing => (RecordingState::Recording, true),
        CapturePhase::RecordingMuting { paused: true } => (RecordingState::Paused, true),
        CapturePhase::RecordingMuting { paused: false } => (RecordingState::Recording, true),
        CapturePhase::RecordingStarting if has_started => (RecordingState::Paused, true),
        CapturePhase::RecordingFinalizing if has_started => (RecordingState::Finalizing, true),
        CapturePhase::RecordingFailed => (RecordingState::Failed, false),
        _ => return None,
    })
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
                | CapturePhase::RecordingFailed
        )
    )
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

#[derive(Clone, Copy)]
struct ClipboardWrite {
    revision: i64,
    fingerprint: captures_app::clipboard::ClipboardFingerprint,
}

fn copy_image(
    path: &Path,
    clipboard: &mut Option<arboard::Clipboard>,
) -> Result<ClipboardWrite, String> {
    let image = image::open(path).map_err(|e| e.to_string())?.into_rgba8();
    copy_pixels(&image, clipboard)
}

fn copy_pixels(
    image: &image::RgbaImage,
    clipboard: &mut Option<arboard::Clipboard>,
) -> Result<ClipboardWrite, String> {
    if clipboard.is_none() {
        *clipboard = Some(arboard::Clipboard::new().map_err(|e| e.to_string())?);
    }
    clipboard
        .as_mut()
        .expect("clipboard initialized")
        .set_image(arboard::ImageData {
            width: image.width() as usize,
            height: image.height() as usize,
            bytes: Cow::Borrowed(image.as_raw()),
        })
        .map_err(|e| format!("Could not copy image: {e}"))?;
    Ok(ClipboardWrite {
        revision: crate::clipboard_revision::after_write(),
        fingerprint: captures_app::clipboard::ClipboardFingerprint::of_rgba(
            image.width(),
            image.height(),
            image.as_raw(),
        ),
    })
}

/// Whether the clipboard still holds exactly the recorded pixels.
fn clipboard_matches(
    expected: captures_app::clipboard::ClipboardFingerprint,
    clipboard: &mut Option<arboard::Clipboard>,
) -> Result<bool, String> {
    if clipboard.is_none() {
        *clipboard = Some(arboard::Clipboard::new().map_err(|e| e.to_string())?);
    }
    match clipboard
        .as_mut()
        .expect("clipboard initialized")
        .get_image()
    {
        Ok(image) => {
            let width = u32::try_from(image.width).unwrap_or(u32::MAX);
            let height = u32::try_from(image.height).unwrap_or(u32::MAX);
            Ok(
                captures_app::clipboard::ClipboardFingerprint::of_rgba(width, height, &image.bytes)
                    == expected,
            )
        }
        // Retry later when another app briefly holds the clipboard.
        Err(arboard::Error::ClipboardOccupied) => {
            Err(arboard::Error::ClipboardOccupied.to_string())
        }
        // No image, or contents that are not a decodable image (for example
        // text offered for an image type), cannot be this capture.
        Err(_) => Ok(false),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CaptureAction {
    RefreshDisplays,
    Capture(CaptureRequest),
    Permissions,
}

/// The History header's display and capture actions. The row wraps instead of
/// running past the window edge, as shipping's History header reflows when it
/// runs out of width: under the token fonts (DejaVu Sans on Linux) it is wider
/// than the 1000px root window. Returns the clicked action and the row's rect.
fn capture_actions(
    ui: &mut egui::Ui,
    displays: &[DisplayDescriptor],
    display_id: &mut Option<String>,
    can_start_capture: bool,
) -> (Option<CaptureAction>, egui::Rect) {
    let mut action = None;
    let row = ui.horizontal_wrapped(|ui| {
        egui::ComboBox::from_label("Display")
            .selected_text(
                displays
                    .iter()
                    .find(|d| Some(&d.id) == display_id.as_ref())
                    .map_or("No display", |d| d.name.as_str()),
            )
            .show_ui(ui, |ui| {
                for display in displays {
                    ui.selectable_value(
                        display_id,
                        Some(display.id.clone()),
                        format!(
                            "{} — {}×{}{}",
                            display.name,
                            display.width,
                            display.height,
                            if display.is_primary { " (Primary)" } else { "" }
                        ),
                    );
                }
            });
        if ui.button("Refresh displays").clicked() {
            action = Some(CaptureAction::RefreshDisplays);
        }
        for (label, request) in [
            ("New Capture", CaptureRequest::NewCapture),
            ("Capture display", CaptureRequest::Display),
            ("Capture region", CaptureRequest::Region),
            ("Capture window", CaptureRequest::Window),
        ] {
            if ui
                .add_enabled(can_start_capture, egui::Button::new(label))
                .clicked()
            {
                action = Some(CaptureAction::Capture(request));
            }
        }
        if ui.button("Capture permissions…").clicked() {
            action = Some(CaptureAction::Permissions);
        }
    });
    (action, row.response.rect)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn history_capture_actions_wrap_inside_the_root_window() {
        // Token fonts (DejaVu Sans on Linux CI) are wider than egui's default.
        let ctx = egui::Context::default();
        crate::ui_fonts::install(&ctx);
        let tokens = crate::tokens::load().remove("light-mustard").unwrap();
        tokens.apply(&ctx, true);
        let displays = [DisplayDescriptor {
            id: "0".into(),
            name: "screen".into(),
            x: 0,
            y: 0,
            width: 1280,
            height: 720,
            scale_factor: 1.,
            is_primary: true,
        }];
        let side = tokens.number("s-8");
        let layout = |width: f32| {
            let mut display_id = Some("0".into());
            let mut layout = (egui::Rect::NOTHING, egui::Rect::NOTHING);
            for _ in 0..2 {
                // The first pass loads the fonts.
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width, 720.),
                        )),
                        ..Default::default()
                    },
                    |ui| {
                        egui::Panel::top("live-header")
                            .frame(egui::Frame::new().inner_margin(side))
                            .show(ui, |ui| {
                                let available = ui.max_rect();
                                let (_, row) =
                                    capture_actions(ui, &displays, &mut display_id, true);
                                layout = (available, row);
                            });
                    },
                );
                output.textures_delta.clear();
            }
            layout
        };
        // The root window's fixed minimum size: whether the row wraps here
        // depends on the platform font, but it never runs past the margin.
        let (available, row) = layout(1000.);
        assert_eq!(available.right(), 1000. - side);
        assert!(
            row.right() <= available.right(),
            "row {row:?} runs past {available:?}"
        );
        // Narrower than the actions in any font: the row must wrap.
        let (available, row) = layout(700.);
        assert!(
            row.right() <= available.right(),
            "row {row:?} runs past {available:?}"
        );
        assert!(row.height() > 2. * tokens.number("h-md"));
    }

    #[test]
    fn shared_worker_wakes_root_while_an_editor_viewport_is_active() {
        let root = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut live = Live::new(ctx.clone(), Some(root.path().into()));
        // Finish startup replies AND their wake calls, then consume the initial
        // root paints. Startup must not satisfy the later wake assertion.
        let (done, completed) = mpsc::channel();
        live.tx.send(Job::Barrier(done)).unwrap();
        completed.recv_timeout(Duration::from_secs(5)).unwrap();
        while live.rx.try_recv().is_ok() {}
        for _ in 0..3 {
            ctx.begin_pass(Default::default());
            let mut output = ctx.end_pass();
            output.textures_delta.clear();
        }

        let child = egui::ViewportId::from_hash_of("recording-editor");
        let mut input = egui::RawInput {
            viewport_id: child,
            ..Default::default()
        };
        input.viewports.insert(
            child,
            egui::ViewportInfo {
                parent: Some(egui::ViewportId::ROOT),
                ..Default::default()
            },
        );
        ctx.begin_pass(input);
        let (wakes, wake_receiver) = mpsc::channel();
        ctx.set_request_repaint_callback(move |info| {
            let _ = wakes.send(info.viewport_id);
        });

        live.load_history();
        let (done, completed) = mpsc::channel();
        live.tx.send(Job::Barrier(done)).unwrap();
        completed.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(
            live.rx.try_recv().unwrap(),
            Reply::HistoryLoaded { result: Ok(_), .. }
        ));
        assert_eq!(wake_receiver.try_recv().unwrap(), egui::ViewportId::ROOT);
        let mut output = ctx.end_pass();
        output.textures_delta.clear();
        live.flush();
    }

    #[test]
    fn external_media_serialize_after_startup_keep_errors_and_snapshot_active_editors() {
        let root = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut live = Live::new(ctx.clone(), Some(root.path().into()));
        live.flush();
        live.recovery = crate::recording_recovery::Recovery::new(ctx.clone(), root.path().into());
        let (tx, jobs) = mpsc::channel();
        live.tx = tx;
        live.pending = 1;
        let output = root.path().join("exports");
        live.queue_open_media(
            ["bad.png", "good.png", "alias.png", "closed.png"].map(PathBuf::from),
            Ok(output.clone()),
        );
        live.start_next_media();
        assert!(
            jobs.try_recv().is_err(),
            "initial History work must finish first"
        );
        live.pending = 0;
        live.permission_recovery_visible = true;
        live.start_next_media();
        assert!(
            jobs.try_recv().is_err(),
            "permission recovery must hold queued media"
        );
        assert!(
            !live.can_launch_capture(),
            "permission recovery must block capture and shortcut routing"
        );
        live.permission_recovery_visible = false;
        live.capture_in_flight = true;
        live.start_next_media();
        assert!(
            jobs.try_recv().is_err(),
            "capture ownership wins over queued file opens"
        );
        live.capture_in_flight = false;
        live.start_next_media();
        let Job::OpenMedia {
            path,
            root: actual_root,
            open_artifact_ids,
            output_directory,
        } = jobs.try_recv().unwrap()
        else {
            panic!("expected open")
        };
        assert_eq!(path, Path::new("bad.png"));
        assert_eq!(actual_root, root.path());
        assert_eq!(output_directory, output);
        assert!(open_artifact_ids.is_empty());
        assert!(!live.can_launch_capture());
        live.start_next_media();
        assert!(
            jobs.try_recv().is_err(),
            "only one media open may be in flight"
        );
        live.media_opened(&ctx, &path, Err("corrupt image".into()), output.clone());
        live.start_next_media();
        assert!(
            matches!(jobs.try_recv().unwrap(), Job::OpenMedia { path, .. } if path == Path::new("good.png"))
        );
        let artifact = captures_app::persist_screenshot(
            root.path(),
            &image::RgbaImage::from_pixel(9, 5, image::Rgba([31, 102, 207, 255])),
            captures_capture::CaptureMode::Display,
        )
        .unwrap();
        let id = artifact.entry.id.clone();
        live.media_opened(
            &ctx,
            Path::new("good.png"),
            Ok(Box::new(artifact)),
            output.clone(),
        );
        assert_eq!(live.selection.id.as_deref(), Some(id.as_str()));
        assert_eq!(live.editors.len(), 1);
        // History thumbnails are requested by visible cards, not selection.
        live.start_next_media();
        let Job::OpenMedia {
            path,
            open_artifact_ids,
            ..
        } = jobs.try_recv().unwrap()
        else {
            panic!("expected alias open")
        };
        assert_eq!(path, Path::new("alias.png"));
        assert_eq!(
            open_artifact_ids,
            std::slice::from_ref(&id),
            "IDs are collected after the prior editor opened"
        );
        let artifact = captures_app::list(root.path()).unwrap().pop().unwrap();
        live.media_opened(&ctx, &path, Ok(Box::new(artifact)), output.clone());
        assert_eq!(live.editors.len(), 1);
        assert_eq!(
            live.artifacts.len(),
            1,
            "refocusing must not duplicate History rows"
        );
        assert!(
            live.error
                .as_ref()
                .unwrap()
                .contains("bad.png: corrupt image"),
            "later successes must retain earlier file errors"
        );
        // History thumbnails are requested by visible cards, not selection.
        live.editors.clear();
        live.start_next_media();
        let Job::OpenMedia {
            path,
            open_artifact_ids,
            ..
        } = jobs.try_recv().unwrap()
        else {
            panic!("expected closed-source open")
        };
        assert_eq!(path, Path::new("closed.png"));
        assert!(
            open_artifact_ids.is_empty(),
            "closed editors must not suppress reload"
        );
        live.media_opened(&ctx, &path, Err("source missing".into()), output.clone());
        assert_eq!(live.media_open_errors.len(), 2);
        live.queue_open_media(
            [PathBuf::from("settings.png")],
            Err("settings unreadable".into()),
        );
        live.start_next_media();
        assert!(
            jobs.try_recv().is_err(),
            "failed settings must not publish an image without an editor"
        );
        assert_eq!(
            live.error.as_deref(),
            Some("Could not open settings.png: settings unreadable")
        );
        live.queue_open_media([PathBuf::from("never-started.png")], Ok(output));
        live.flush();
        assert!(
            live.open_media.is_empty(),
            "quit drops unstarted file requests"
        );
    }

    #[test]
    fn recording_screenshot_retires_ui_before_settling_and_captures_once() {
        let rect = SelectionRect {
            x: 140.,
            y: 180.,
            width: 310.,
            height: 170.,
        };
        let start = Instant::now();
        for after_countdown in [false, true] {
            let mut phase = RecordingScreenshotPhase::RetiringCaptureUi {
                rect,
                after_countdown,
                omitted_frame: 42,
            };
            // A slow or repeated layout pass is not evidence of native removal.
            assert_eq!(
                phase.capture_after_hide(42, start + Duration::from_secs(1)),
                None
            );
            let retired = start + Duration::from_secs(2);
            assert_eq!(phase.capture_after_hide(43, retired), None);
            assert_eq!(
                phase.capture_after_hide(44, retired + Duration::from_millis(149)),
                None,
            );
            assert_eq!(
                phase.capture_after_hide(45, retired + Duration::from_millis(150)),
                Some((rect, after_countdown)),
            );
            assert_eq!(phase, RecordingScreenshotPhase::Capturing);
            assert_eq!(
                phase.capture_after_hide(46, retired + Duration::from_secs(1)),
                None
            );
        }
    }

    #[test]
    fn recording_hud_stays_up_while_busy_saving_and_failed() {
        use CapturePhase as Phase;
        use RecordingState as State;
        for (phase, started, expected) in [
            (Phase::Recording, true, Some((State::Recording, false))),
            (Phase::RecordingPaused, true, Some((State::Paused, false))),
            (
                Phase::RecordingPausing,
                true,
                Some((State::Recording, true)),
            ),
            (Phase::RecordingStarting, true, Some((State::Paused, true))),
            (Phase::RecordingStarting, false, None),
            (
                Phase::RecordingMuting { paused: true },
                true,
                Some((State::Paused, true)),
            ),
            (
                Phase::RecordingFinalizing,
                true,
                Some((State::Finalizing, true)),
            ),
            (Phase::RecordingFailed, false, Some((State::Failed, false))),
            (Phase::RecordingCountdown, false, None),
            (Phase::RecordingRestarting, true, None),
            (Phase::RecordingDiscarding, true, None),
        ] {
            assert_eq!(
                recording_hud_state(Some(phase), started),
                expected,
                "{phase:?}"
            );
        }
        assert!(is_recording_phase(Some(Phase::RecordingFailed)));
    }

    #[test]
    fn failed_start_keeps_the_take_with_an_inline_error_until_the_next_action() {
        let root = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut live = Live::new(ctx.clone(), Some(root.path().into()));
        live.capture_phase = Some(CapturePhase::RecordingStarting);
        let snapshot: RecordingSessionSnapshot = serde_json::from_value(serde_json::json!({
            "id": "take", "state": "failed", "elapsed_ms": 0,
            "countdown_remaining_seconds": null, "warning": null,
            "error": "no microphone device is available",
            "options": {
                "kind": "video", "target": {"type": "display", "display_id": "fixture"},
                "frames_per_second": 15, "max_resolution": "original",
                "countdown_seconds": 3, "show_cursor": false
            }
        }))
        .unwrap();
        live.enter_failed_recording(
            &ctx,
            snapshot,
            "Error: no microphone device is available".into(),
        );
        assert_eq!(live.capture_phase, Some(CapturePhase::RecordingFailed));
        assert!(live.status.starts_with("Recording failed"));
        let error = live.recording_snapshot.as_ref().unwrap().error.clone();
        assert_eq!(
            live.recording_hud_error.line(error.as_deref()).as_deref(),
            Some("no microphone device is available")
        );
        live.hud_action_started();
        // The session's own failure remains until Retry replaces the take.
        assert_eq!(
            live.recording_hud_error.line(error.as_deref()).as_deref(),
            Some("no microphone device is available")
        );
        assert_eq!(live.recording_hud_error.line(None), None);
        live.capture_phase = None;
        live.flush();
    }

    #[test]
    fn cancelling_screenshot_hide_wait_preserves_paused_recording() {
        let root = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut live = Live::new(ctx.clone(), Some(root.path().into()));
        live.capture_phase = Some(CapturePhase::RecordingPaused);
        live.recording_screenshot_phase = Some(RecordingScreenshotPhase::SettlingCaptureUi {
            rect: SelectionRect {
                x: 140.,
                y: 180.,
                width: 310.,
                height: 170.,
            },
            after_countdown: true,
            until: Instant::now() + Duration::from_millis(150),
        });
        live.finish_recording_screenshot(&ctx, false);
        assert!(live.recording_screenshot_phase.is_none());
        assert_eq!(live.capture_phase, Some(CapturePhase::RecordingPaused));
        assert_eq!(live.status, "Recording paused");
        live.flush();
    }

    #[test]
    fn history_filters_preserve_ids_and_invalidate_hidden_preview_work() {
        use captures_history::ArtifactKind;
        let root = tempfile::tempdir().unwrap();
        let original = preview_artifact(root.path(), [47, 83, 129, 255]);
        let artifacts = || {
            [
                ("v1", ArtifactKind::Video),
                ("s1", ArtifactKind::Screenshot),
                ("g1", ArtifactKind::Gif),
                ("s2", ArtifactKind::Screenshot),
                ("v2", ArtifactKind::Video),
            ]
            .into_iter()
            .map(|(id, kind)| {
                let mut item = Artifact {
                    entry: original.entry.clone(),
                    image_path: original.image_path.clone(),
                    preview_path: original.preview_path.clone(),
                };
                item.entry.id = id.into();
                item.entry.kind = kind;
                item
            })
            .collect::<Vec<_>>()
        };
        let mut live = Live::new(egui::Context::default(), Some(root.path().into()));
        live.apply(
            Response::History {
                artifacts: artifacts(),
            },
            false,
        );
        assert!(live.history_loaded);
        assert!(
            live.selection.id.is_none(),
            "loading History never selects a card"
        );
        live.select("v1".into());
        let original_selection = live.selection.generation;
        live.confirm_delete = Some(("v1".into(), Instant::now()));
        live.history_filter = HistoryFilter::Screenshots;
        live.refresh_history_selection();
        assert!(live.selection.id.is_none(), "a hidden selection is cleared");
        assert!(!live.selection.accepts(original_selection));
        assert!(live.confirm_delete.is_none());
        live.select("s1".into());

        live.select("s2".into());
        live.apply(
            Response::History {
                artifacts: artifacts(),
            },
            false,
        );
        assert_eq!(live.history_filter, HistoryFilter::Screenshots);
        assert_eq!(live.selection.id.as_deref(), Some("s2"));
        live.apply(Response::Deleted { id: "s2".into() }, false);
        assert!(
            live.selection.id.is_none(),
            "deletion never selects another card"
        );
        live.apply(Response::Deleted { id: "s1".into() }, false);
        assert_eq!(live.history_filter, HistoryFilter::Screenshots);
        assert!(live.selection.id.is_none());
        assert_eq!(
            live.artifacts.len(),
            3,
            "filtering must not remove other kinds"
        );

        live.history_filter = HistoryFilter::Gif;
        live.refresh_history_selection();
        live.select("g1".into());
        assert_eq!(live.selection.id.as_deref(), Some("g1"));
        live.apply(
            Response::History {
                artifacts: artifacts().into_iter().take(1).collect(),
            },
            false,
        );
        assert_eq!(live.history_filter, HistoryFilter::Gif);
        assert!(live.selection.id.is_none());
        // An explicit capture/preview reveal must never select a hidden row.
        live.select("v1".into());
        assert_eq!(live.history_filter, HistoryFilter::All);
        assert_eq!(live.selection.id.as_deref(), Some("v1"));
        live.flush();
    }

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
    fn preview_saved_path_starts_from_history_and_tracks_authoritative_save_response() {
        let root = tempfile::tempdir().unwrap();
        let mut artifact = preview_artifact(root.path(), [12, 34, 56, 255]);
        let original = root.path().join("original-export.png");
        artifact.entry.saved_path = Some(original.to_string_lossy().into_owned());
        let mut previews = MiniPreviews::default();
        previews
            .begin_capture(&AppSettings::default(), Some(preview_target()), 1)
            .unwrap();
        let (guard, _) = previews.start_artifact(&artifact).unwrap().unwrap();
        assert_eq!(
            previews.cards[&guard.artifact_id].saved_path.as_deref(),
            Some(original.as_path())
        );

        let ctx = egui::Context::default();
        let mut live = Live::new(ctx, Some(root.path().into()));
        live.previews = previews;
        let current = root.path().join("current-export.png");
        artifact.entry.saved_path = Some(current.to_string_lossy().into_owned());
        live.apply(
            Response::Saved {
                artifact,
                path: current.clone(),
            },
            false,
        );
        assert_eq!(
            live.previews.cards[&guard.artifact_id]
                .saved_path
                .as_deref(),
            Some(current.as_path())
        );
        live.flush();
    }

    #[test]
    fn reveal_dispatch_guards_identity_path_busy_and_dismissed_completions() {
        let root = tempfile::tempdir().unwrap();
        let artifact = preview_artifact(root.path(), [65, 43, 21, 255]);
        let original = artifact.image_path.clone();
        let missing = root.path().join("Café shots 東京.png");
        let ctx = egui::Context::default();
        let mut frame = eframe::Frame::_new_kittest();
        let mut live = Live::new(ctx.clone(), Some(root.path().into()));
        live.flush();
        let (jobs, requests) = mpsc::channel();
        live.tx = jobs;
        let (results, replies) = mpsc::channel();
        live.rx = replies;
        live.pending = 0;
        live.previews
            .begin_capture(&AppSettings::default(), Some(preview_target()), 1)
            .unwrap();
        let (guard, _) = live.previews.start_artifact(&artifact).unwrap().unwrap();
        live.previews
            .cards
            .get_mut(&guard.artifact_id)
            .unwrap()
            .saved_path = Some(missing.clone());
        live.artifacts.push(artifact);
        ctx.begin_pass(Default::default());

        for (generation, path) in [
            (guard.generation + 1, missing.clone()),
            (guard.generation, root.path().join("another capture.png")),
        ] {
            live.preview_tx
                .send(PreviewMessage::Reveal {
                    artifact_id: guard.artifact_id.clone(),
                    generation,
                    path,
                })
                .unwrap();
        }
        live.logic(&ctx, &mut frame);
        assert!(requests.try_recv().is_err());
        for dismiss in [false, true] {
            // Two queued clicks dispatch only once while the first is busy.
            for _ in 0..2 {
                live.preview_tx
                    .send(PreviewMessage::Reveal {
                        artifact_id: guard.artifact_id.clone(),
                        generation: guard.generation,
                        path: missing.clone(),
                    })
                    .unwrap();
            }
            live.logic(&ctx, &mut frame);
            let Job::Reveal { path, preview } = requests.try_recv().unwrap() else {
                panic!("not a reveal")
            };
            assert_eq!(path, missing);
            assert_eq!(preview.artifact_id, guard.artifact_id);
            assert_eq!(preview.generation, guard.generation);
            assert!(requests.try_recv().is_err());
            assert_eq!(live.pending, 1);
            if dismiss {
                assert!(live.previews.dismiss(&guard.artifact_id, guard.generation));
            }
            results
                .send(Reply::Revealed {
                    preview,
                    result: Err(reveal(&missing).unwrap_err().to_string()),
                })
                .unwrap();
            live.logic(&ctx, &mut frame);
            assert_eq!(live.pending, 0);
            if dismiss {
                assert!(live.previews.cards.is_empty());
            } else {
                let card = &live.previews.cards[&guard.artifact_id];
                assert_eq!(card.saved_path.as_ref(), Some(&missing));
                assert!(card.busy.is_none());
                assert!(
                    card.message
                        .as_deref()
                        .unwrap()
                        .contains("no longer exists")
                );
            }
            assert_eq!(live.artifacts.len(), 1);
            assert!(original.exists());
        }
        ctx.end_pass().textures_delta.clear();
    }

    #[test]
    fn trash_dispatch_retries_preserves_history_and_rejects_stale_callbacks() {
        let root = tempfile::tempdir().unwrap();
        let artifact = preview_artifact(root.path(), [65, 43, 21, 255]);
        let original = artifact.image_path.clone();
        let saved_path = root.path().join("Café export.png");
        let ctx = egui::Context::default();
        let mut frame = eframe::Frame::_new_kittest();
        let mut live = Live::new(ctx.clone(), Some(root.path().into()));
        live.flush();
        let (jobs, requests) = mpsc::channel();
        live.tx = jobs;
        let (results, replies) = mpsc::channel();
        live.rx = replies;
        live.pending = 0;
        live.previews
            .begin_capture(&AppSettings::default(), Some(preview_target()), 1)
            .unwrap();
        let (guard, _) = live.previews.start_artifact(&artifact).unwrap().unwrap();
        live.previews
            .cards
            .get_mut(&guard.artifact_id)
            .unwrap()
            .saved_path = Some(saved_path.clone());
        live.artifacts.push(artifact);
        ctx.begin_pass(Default::default());
        for (generation, path) in [
            (guard.generation + 1, Some(saved_path.clone())),
            (guard.generation, None),
        ] {
            live.preview_tx
                .send(PreviewMessage::Trash {
                    artifact_id: guard.artifact_id.clone(),
                    generation,
                    saved_path: path,
                })
                .unwrap();
        }
        live.logic(&ctx, &mut frame);
        assert!(requests.try_recv().is_err());
        for attempt in 0..3 {
            let generation = live.previews.cards[&guard.artifact_id].generation;
            for _ in 0..2 {
                live.preview_tx
                    .send(PreviewMessage::Trash {
                        artifact_id: guard.artifact_id.clone(),
                        generation,
                        saved_path: Some(saved_path.clone()),
                    })
                    .unwrap();
            }
            live.logic(&ctx, &mut frame);
            let Job::Execute {
                request:
                    Request::TrashPreview {
                        root: actual_root,
                        id,
                        saved_path: path,
                    },
                preview,
                notice,
            } = requests.try_recv().unwrap()
            else {
                panic!("not preview Trash")
            };
            assert_eq!(actual_root, root.path());
            assert_eq!(id, guard.artifact_id);
            assert_eq!(path, Some(saved_path.clone()));
            assert!(notice.is_none());
            assert!(
                requests.try_recv().is_err(),
                "busy Trash dispatches only once"
            );
            assert_eq!(live.pending, 1);
            if attempt == 1 {
                // Same ID, different presentation: a late reply cannot remove it.
                let card = live.previews.cards.get_mut(&id).unwrap();
                card.generation += 1;
                card.busy = None;
            }
            results
                .send(Reply::Executed {
                    preview,
                    notice: None,
                    result: if attempt == 0 {
                        Err("fixture denied".into())
                    } else {
                        Ok(Box::new(Response::PreviewTrashed { id }))
                    },
                })
                .unwrap();
            live.logic(&ctx, &mut frame);
            assert_eq!(live.pending, 0);
            assert_eq!(live.artifacts.len(), 1);
            assert!(original.exists());
            if attempt == 0 {
                let card = &live.previews.cards[&guard.artifact_id];
                assert!(card.busy.is_none());
                assert_eq!(
                    card.message.as_deref(),
                    Some("Trash failed: fixture denied")
                );
            } else if attempt == 1 {
                assert!(live.previews.cards.contains_key(&guard.artifact_id));
            } else {
                assert!(live.previews.cards.is_empty());
            }
        }
        ctx.end_pass().textures_delta.clear();
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
    fn preview_edit_targets_its_artifact_without_showing_root_or_changing_history_selection() {
        let root = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap();
        let artifact = preview_artifact(root.path(), [65, 43, 21, 255]);
        let other = preview_artifact(root.path(), [11, 155, 241, 255]);
        let id = artifact.entry.id.clone();
        let other_id = other.entry.id.clone();
        let original = fs::read(&artifact.image_path).unwrap();
        let ctx = egui::Context::default();
        let mut frame = eframe::Frame::_new_kittest();
        let mut live = Live::new(ctx.clone(), Some(root.path().into()));
        live.flush();
        live.previews
            .begin_capture(&AppSettings::default(), Some(preview_target()), 1)
            .unwrap();
        let (guard, _) = live.previews.start_artifact(&artifact).unwrap().unwrap();
        live.artifacts = vec![artifact, other];
        live.selection.begin(other_id.clone());
        ctx.begin_pass(Default::default());
        live.logic(&ctx, &mut frame);
        // flush joins the History worker, not the independent recovery scan.
        // Editing is intentionally blocked until that startup scan completes.
        let deadline = Instant::now() + Duration::from_secs(5);
        while live.recovery.blocking() {
            live.recovery.receive();
            assert!(
                Instant::now() < deadline,
                "initial recovery list did not settle"
            );
            thread::sleep(Duration::from_millis(5));
        }
        for busy in [true, false] {
            live.previews.cards.get_mut(&id).unwrap().busy =
                busy.then_some(crate::mini_preview::Busy::Save);
            live.preview_tx
                .send(PreviewMessage::Edit {
                    artifact_id: id.clone(),
                    generation: guard.generation + u64::from(!busy),
                    directory: output.path().into(),
                })
                .unwrap();
            live.logic(&ctx, &mut frame);
            assert!(
                live.editors.is_empty(),
                "busy and stale actions must not open an editor"
            );
        }
        for _ in 0..2 {
            live.preview_tx
                .send(PreviewMessage::Edit {
                    artifact_id: id.clone(),
                    generation: guard.generation,
                    directory: output.path().into(),
                })
                .unwrap();
            live.logic(&ctx, &mut frame);
            assert_eq!(live.editors.keys().collect::<Vec<_>>(), [&id]);
            live.editors[&id].flush(&ctx).unwrap();
            assert_eq!(live.selection.id.as_ref(), Some(&other_id));
            assert_eq!(live.artifacts.len(), 2);
        }
        let mut output = ctx.end_pass();
        let commands = &output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .expect("root viewport output")
            .commands;
        assert!(!commands.iter().any(|command| matches!(
            command,
            egui::ViewportCommand::Minimized(false)
                | egui::ViewportCommand::Visible(true)
                | egui::ViewportCommand::Focus
        )));
        output.textures_delta.clear();
        assert_eq!(
            fs::read(
                &live
                    .artifacts
                    .iter()
                    .find(|item| item.entry.id == id)
                    .unwrap()
                    .image_path
            )
            .unwrap(),
            original
        );
        live.flush();
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
        let mut live = Live::new(egui::Context::default(), Some(root.path().join("history")));
        let deadline = Instant::now() + Duration::from_secs(5);
        while live.recovery.blocking() {
            live.recovery.receive();
            assert!(
                Instant::now() < deadline,
                "initial recovery list did not settle"
            );
            thread::sleep(Duration::from_millis(5));
        }
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
    fn recording_countdown_uses_shipping_heading_and_title() {
        assert_eq!(
            main_countdown_presentation(Some(CapturePhase::RecordingCountdown)),
            (
                "Captures Recording Countdown",
                crate::countdown::Kind::Recording
            )
        );
        assert_eq!(
            main_countdown_presentation(Some(CapturePhase::DisplayCountdown)),
            (
                "Captures Screenshot Countdown",
                crate::countdown::Kind::Screenshot
            )
        );
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
        live.viewports(&ctx, &tokens, Ok(AppSettings::default()), false);
        let mut output = ctx.end_pass();

        let declared = output
            .viewport_output
            .contains_key(&egui::ViewportId::from_hash_of("screenshot-countdown"));
        output.textures_delta.clear();
        assert!(declared);
        live.flush();
    }

    #[test]
    fn cancelled_countdown_lingers_with_cancelling_copy_then_closes() {
        let root = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        ctx.set_embed_viewports(false);
        let mut live = Live::new(ctx.clone(), Some(root.path().into()));
        let target = CaptureTarget {
            monitor: 0,
            position: egui::pos2(0., 0.),
            size: egui::vec2(800., 600.),
            preview_bounds: None,
        };
        let tokens = crate::tokens::load()["dark-mustard"].clone();
        let declared = |live: &mut Live| {
            ctx.begin_pass(Default::default());
            live.viewports(&ctx, &tokens, Ok(AppSettings::default()), false);
            let mut output = ctx.end_pass();
            output.textures_delta.clear();
            output
                .viewport_output
                .contains_key(&main_countdown_viewport())
        };

        live.countdown_exit = Some(CountdownExit::new(
            main_countdown_viewport(),
            "Captures Recording Countdown",
            target,
            crate::countdown::Kind::Recording,
            2,
        ));
        assert!(declared(&mut live), "cancelled countdown stays up briefly");
        live.countdown_exit.as_mut().unwrap().until = Instant::now();
        assert!(!declared(&mut live), "the exit window elapsed");
        assert!(live.countdown_exit.is_none());
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
    fn reduced_motion_snaps_stored_preview_animation_before_reenabling() {
        let root = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        ctx.set_embed_viewports(true);
        let mut live = Live::new(ctx.clone(), Some(root.path().into()));
        let settings = AppSettings::default();
        let texture = ctx.load_texture(
            "motion",
            egui::ColorImage::filled([2, 2], egui::Color32::WHITE),
            egui::TextureOptions::LINEAR,
        );
        for color in [[31, 59, 127, 255], [171, 23, 91, 255]] {
            let artifact = preview_artifact(root.path(), color);
            live.previews
                .begin_capture(&settings, Some(preview_target()), 1)
                .unwrap();
            let (guard, _) = live.previews.start_artifact(&artifact).unwrap().unwrap();
            live.previews
                .cards
                .get_mut(&guard.artifact_id)
                .unwrap()
                .texture = Some(texture.clone());
            live.previews.mark_ready(&guard.artifact_id);
        }
        live.previews.stack.set_collapsed(true);
        assert!(live.previews.is_visible());
        let tokens = crate::tokens::load()["dark-mustard"].clone();
        ctx.begin_pass(Default::default());
        let animation = egui::Id::unique("mini-preview-hover-fan");
        assert_eq!(ctx.animate_bool_with_time(animation, true, 0.), 1.);
        // While reduction is enabled the pointer left the card. Re-enabling
        // motion must not resurrect the old open fan from the animation cache.
        live.viewports(&ctx, &tokens, Ok(settings), true);
        assert_eq!(ctx.animate_bool_with_time(animation, false, 0.2), 0.);
        let mut output = ctx.end_pass();
        output.textures_delta.clear();
        live.flush();
    }

    #[test]
    fn compact_drag_stores_clamped_edge_and_clears_only_on_session_reset() {
        use captures_settings::MiniPreviewPlacement;
        let root = tempfile::tempdir().unwrap();
        let artifact = preview_artifact(root.path(), [31, 59, 127, 255]);
        let ctx = egui::Context::default();
        let texture = ctx.load_texture(
            "drag",
            egui::ColorImage::filled([2, 2], egui::Color32::WHITE),
            egui::TextureOptions::LINEAR,
        );
        for (placement, edge) in [
            (MiniPreviewPlacement::TopLeft, 170.),
            (MiniPreviewPlacement::BottomRight, 434.),
        ] {
            let settings = AppSettings {
                mini_preview_placement: placement,
                ..Default::default()
            };
            let mut previews = MiniPreviews::default();
            previews
                .begin_capture(&settings, Some(preview_target()), 1)
                .unwrap();
            let (guard, _) = previews.start_artifact(&artifact).unwrap().unwrap();
            previews.cards.get_mut(&guard.artifact_id).unwrap().texture = Some(texture.clone());
            previews.mark_ready(&guard.artifact_id);
            previews.stack.set_collapsed(true);
            previews.move_stack(egui::pos2(240., 170.));
            let origin = previews.visibility.stack_origin().unwrap();
            assert_eq!((origin.x, origin.edge), (240., edge));
            previews.stack.set_collapsed(false);
            previews.move_stack(egui::pos2(100., 100.));
            assert_eq!(previews.visibility.stack_origin().unwrap().edge, edge);
            previews
                .begin_capture(&settings, Some(preview_target()), 2)
                .unwrap();
            previews.restore_capture();
            assert_eq!(previews.visibility.stack_origin().unwrap().edge, edge);
            previews.stack.set_collapsed(true);
            previews.move_stack(egui::pos2(9999., 9999.));
            let clamped = previews.visibility.stack_origin().unwrap();
            assert_eq!(clamped.x, 940.);
            assert!(
                clamped.edge <= 660.,
                "keep auto-hide taskbar and chrome gap"
            );
            previews.clear(&[guard.artifact_id]);
            assert!(previews.visibility.stack_origin().is_none());
        }
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

    /// Drain the startup History/display loads and recovery listing so card
    /// actions are not blocked by them.
    fn settle_startup(live: &mut Live) {
        let (done, barrier) = mpsc::channel();
        live.tx.send(Job::Barrier(done)).unwrap();
        barrier.recv_timeout(Duration::from_secs(5)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while live.recovery.blocking() && Instant::now() < deadline {
            let _ = live.recovery.receive();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!live.recovery.blocking());
        live.rx.try_iter().for_each(drop);
        live.pending = 0;
    }

    #[test]
    fn card_delete_needs_a_second_click_except_missing_recordings() {
        let root = tempfile::tempdir().unwrap();
        let screenshot = preview_artifact(root.path(), [10, 20, 30, 255]);
        let id = screenshot.entry.id.clone();
        let mut live = Live::new(egui::Context::default(), Some(root.path().into()));
        live.apply(
            Response::History {
                artifacts: load_history(root.path()).unwrap(),
            },
            false,
        );
        settle_startup(&mut live);
        live.history_cards.insert(
            id.clone(),
            captures_app::history_view::card(&screenshot.entry, false),
        );
        let now = Instant::now();
        live.delete_card(&id, now);
        assert_eq!(
            live.confirm_delete
                .as_ref()
                .map(|(armed, _)| armed.as_str()),
            Some(id.as_str())
        );
        assert_eq!(live.pending, 0, "the first click only arms deletion");
        // The shipping revert after four seconds.
        live.expire_history_confirmations(&egui::Context::default(), now + Duration::from_secs(5));
        assert!(live.confirm_delete.is_none());
        live.delete_card(&id, now);
        live.delete_card(&id, now);
        assert_eq!(live.pending, 1, "the second click deletes");
        live.flush();
        let deleted = live
            .rx
            .try_iter()
            .find_map(|reply| match reply {
                Reply::Executed { result, .. } => Some(result.unwrap()),
                _ => None,
            })
            .expect("delete response");
        live.apply(*deleted, true);
        assert!(live.artifacts.is_empty());
        assert!(live.confirm_delete.is_none());

        let mut missing = screenshot.entry.clone();
        missing.id = "missing".into();
        missing.kind = captures_history::ArtifactKind::Video;
        live.history_cards.insert(
            missing.id.clone(),
            captures_app::history_view::card(&missing, true),
        );
        live.delete_card("missing", now);
        assert!(
            live.confirm_delete.is_none(),
            "missing entries are removed at once"
        );
        assert_eq!(live.pending, 1);
    }

    #[test]
    fn delete_all_arms_then_clears_history() {
        let root = tempfile::tempdir().unwrap();
        preview_artifact(root.path(), [10, 20, 30, 255]);
        let mut live = Live::new(egui::Context::default(), Some(root.path().into()));
        live.apply(
            Response::History {
                artifacts: load_history(root.path()).unwrap(),
            },
            false,
        );
        settle_startup(&mut live);
        let now = Instant::now();
        live.delete_all_history(now);
        assert_eq!(live.confirm_clear_history, Some(now));
        assert!(!live.clearing_history);
        assert_eq!(live.pending, 0);
        live.delete_all_history(now);
        assert!(live.confirm_clear_history.is_none());
        assert!(live.clearing_history);
        assert_eq!(live.pending, 1);
        live.flush();
    }

    #[test]
    fn thumbnails_are_rejected_or_dropped_when_their_entry_changes() {
        let root = tempfile::tempdir().unwrap();
        let artifact = preview_artifact(root.path(), [10, 20, 30, 255]);
        let id = artifact.entry.id.clone();
        let ctx = egui::Context::default();
        let mut live = Live::new(ctx.clone(), Some(root.path().into()));
        live.apply(
            Response::History {
                artifacts: vec![Artifact {
                    entry: artifact.entry.clone(),
                    image_path: artifact.image_path.clone(),
                    preview_path: artifact.preview_path.clone(),
                }],
            },
            false,
        );
        assert!(live.history_loaded);
        live.request_thumbnail(&id);
        let stale = ThumbnailKey::of(&live.artifacts[0]);
        // An editor replaced the original: same ID, new dimensions.
        live.artifacts[0].entry.width += 1;
        live.invalidate_history_cards();
        assert!(
            !live.history_thumbnails.contains_key(&id),
            "outdated request is dropped"
        );
        live.request_thumbnail(&id);
        live.flush();
        for reply in live.rx.try_iter().collect::<Vec<_>>() {
            if let Reply::ThumbnailDecoded {
                id: reply_id,
                key,
                missing,
                result,
            } = reply
            {
                assert_eq!(reply_id, id);
                assert!(!missing, "screenshots are never missing");
                // Deliver both the stale-key and current-key replies.
                let accepted = live
                    .history_thumbnails
                    .get(&reply_id)
                    .is_some_and(|entry| entry.key == key);
                assert_eq!(accepted, key != stale);
                if accepted {
                    let entry = live.history_thumbnails.get_mut(&reply_id).unwrap();
                    entry.thumbnail = match result {
                        Ok(decoded) => crate::history::Thumbnail::Ready(ctx.load_texture(
                            "test",
                            decoded.image,
                            egui::TextureOptions::LINEAR,
                        )),
                        Err(_) => crate::history::Thumbnail::Failed,
                    };
                }
            }
        }
        assert!(matches!(
            live.history_thumbnails
                .get(&id)
                .map(|entry| &entry.thumbnail),
            Some(crate::history::Thumbnail::Ready(_))
        ));
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
        let source = root.path().join("export.mp4");
        std::fs::write(&source, b"exported recording").unwrap();
        let mut recording = artifact.entry.clone();
        recording.id = "67e55044-10b1-426f-9247-bb680e5fe0c8".into();
        recording.kind = captures_history::ArtifactKind::Video;
        recording.mime_type = Some("video/mp4".into());
        recording.duration_ms = Some(1_200);
        recording.target = Some(RecordingTarget::Display {
            display_id: "fixture".into(),
        });
        let poster = std::fs::read(&artifact.preview_path).unwrap();
        captures_history::save_recording(root.path(), &recording, &poster, &source).unwrap();
        let mut live = Live::new(egui::Context::default(), Some(root.path().into()));
        live.history_filter = HistoryFilter::Screenshots;
        live.apply(
            Response::History {
                artifacts: load_history(root.path()).unwrap(),
            },
            true,
        );
        assert_eq!(live.artifacts.len(), 2);
        live.select(artifact.entry.id.clone());
        let decoding = live.selection.generation;
        live.confirm_delete = Some((artifact.entry.id, Instant::now()));
        live.confirm_clear_history = Some(Instant::now());
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
        assert!(load_history(root.path()).unwrap().is_empty());
        assert_eq!(std::fs::read(source).unwrap(), b"exported recording");
        assert_eq!(live.history_filter, HistoryFilter::Screenshots);
        assert!(!live.selection.accepts(decoding));
        assert!(live.selection.id.is_none());
        assert!(live.confirm_delete.is_none());
        assert!(live.confirm_clear_history.is_none());
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
