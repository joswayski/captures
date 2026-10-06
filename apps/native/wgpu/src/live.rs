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
    #[cfg(target_os = "linux")]
    CapturePortal {
        root: PathBuf,
        generation: u64,
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
    /// A display screenshot beside a running recording, under its child flow.
    CaptureDisplay {
        root: PathBuf,
        display_id: String,
        generation: u64,
        include_cursor: bool,
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
    /// The display list is a capture input, not History: a failure waits for
    /// the next capture, which lists the displays again.
    DisplaysListed(Result<Box<Response>, String>),
    RegionPrepared {
        generation: u64,
        result: Result<Box<RegionSession>, String>,
    },
    RegionCaptured {
        generation: u64,
        result: Result<Box<Artifact>, String>,
    },
    DisplayCaptured {
        generation: u64,
        result: Result<Box<Artifact>, String>,
    },
    #[cfg(target_os = "linux")]
    PortalCaptured {
        generation: u64,
        result: Result<Option<Box<Artifact>>, String>,
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
        /// Card-sized pre-blurred copy for the hover treatment.
        blurred: Option<egui::ColorImage>,
        /// Card-sized copy at the arrival's starting `blur(3px)`.
        arrive_blurred: Option<egui::ColorImage>,
        /// Card-sized sharp media for later CSS blurs (pile depth, streak).
        media: Option<egui::ColorImage>,
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
        /// None when display selection belongs to the desktop portal.
        target: Option<capture_controls::Target>,
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

/// What a region or window screenshot beside a recording selected.
#[derive(Clone, Copy, Debug, PartialEq)]
enum RecordingShotChoice {
    Region(SelectionRect),
    Window(SelectionTarget),
}

/// The selector a screenshot beside a recording opens: shipping
/// `start_capture_inner(Region | Window)` from the recording controls'
/// Screenshot button, the region and window shortcuts and the tray.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RecordingSelectorKind {
    Region,
    Window,
}

/// Where a region or window screenshot beside a recording opens its selector.
#[derive(Clone)]
struct RecordingSelectorShot {
    kind: RecordingSelectorKind,
    /// The display under the pointer for a shortcut or tray screenshot, or
    /// the recording's display for its controls' Screenshot button.
    display_id: String,
    target: CaptureTarget,
    /// A recapture froze the previous selector into this one's snapshot, so
    /// its selection never counts down (shipping `includes_capture_ui`).
    includes_capture_ui: bool,
}

/// The capture UI an action puts in place of the open one, on a fresh
/// snapshot that shows it (`capture_error::busy_route`).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Recapture {
    Selector(RecordingSelectorKind),
    Menu {
        record: bool,
        target: capture_controls::TargetMode,
    },
    /// Beside a running take: the display under the pointer, at once.
    Display,
}

/// A recapture whose snapshot is being taken while the open UI stays up.
struct PendingRecapture {
    kind: Recapture,
    generation: u64,
    /// The screenshot beside a recording, not the main capture flow.
    child: bool,
    /// The main flow's phase whose UI stays up until the snapshot is ready.
    from_phase: Option<CapturePhase>,
    display_id: String,
    target: CaptureTarget,
    settings: AppSettings,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum RecordingScreenshotPhase {
    WaitingForHud,
    Preparing,
    Selecting,
    Countdown {
        choice: RecordingShotChoice,
        after_countdown: bool,
    },
    RetiringCaptureUi {
        choice: RecordingShotChoice,
        after_countdown: bool,
        omitted_frame: u64,
    },
    SettlingCaptureUi {
        choice: RecordingShotChoice,
        after_countdown: bool,
        until: Instant,
    },
    Capturing,
    /// Shipping `hide_capture_huds_before_snapshot`: the display screenshot
    /// waits for the recording controls to leave before its countdown.
    DisplayHidingControls,
    DisplayCountdown,
    /// The countdown window retires, then the compositor settles, before the
    /// display is captured (as the region path does).
    DisplayRetiring {
        omitted_frame: u64,
    },
    DisplaySettling {
        until: Instant,
    },
    DisplayCapturing,
}

/// A direct display screenshot taken while a recording keeps running
/// (shipping `start_capture_inner(Display)` during a recording session).
#[derive(Clone)]
struct RecordingDisplayShot {
    /// The display under the pointer, which need not be the recording's.
    display_id: String,
    target: Option<CaptureTarget>,
    countdown_seconds: u8,
    include_cursor: bool,
    /// Shipping keeps the controls in the screenshot only when the user opted
    /// them into captures (`include_recording_controls_in_captures`).
    keeps_controls: bool,
}

impl RecordingDisplayShot {
    /// How long the hidden controls get to leave before the countdown starts.
    fn controls_settle(&self) -> Duration {
        if self.keeps_controls {
            Duration::ZERO
        } else {
            Duration::from_millis(300)
        }
    }
}

impl RecordingScreenshotPhase {
    /// The display countdown's window retires only after the root pass that
    /// omits it; then the compositor gets the same settling interval as a
    /// region. Returns true once the display may be captured.
    fn display_capture_after_hide(&mut self, frame: u64, now: Instant) -> bool {
        match *self {
            Self::DisplayRetiring { omitted_frame } if frame > omitted_frame => {
                *self = Self::DisplaySettling {
                    until: now + Duration::from_millis(150),
                };
                false
            }
            Self::DisplaySettling { until } if now >= until => {
                *self = Self::DisplayCapturing;
                true
            }
            _ => false,
        }
    }

    /// Deferred viewports retire only after the root pass that omits them.
    /// Start the compositor settling interval after that pass, not at countdown
    /// expiry, which may still be running alongside a visible countdown window.
    fn capture_after_hide(
        &mut self,
        frame: u64,
        now: Instant,
    ) -> Option<(RecordingShotChoice, bool)> {
        match *self {
            Self::RetiringCaptureUi {
                choice,
                after_countdown,
                omitted_frame,
            } if frame > omitted_frame => {
                *self = Self::SettlingCaptureUi {
                    choice,
                    after_countdown,
                    until: now + Duration::from_millis(150),
                };
                None
            }
            Self::SettlingCaptureUi {
                choice,
                after_countdown,
                until,
            } if now >= until => {
                *self = Self::Capturing;
                Some((choice, after_countdown))
            }
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaptureRequest {
    NewCapture,
    Recording(capture_controls::TargetMode),
    /// Shipping Screenshot Display: the capture menu on Full screen.
    DisplayMenu,
    /// Capture the display under the pointer directly, beside a running or
    /// paused recording.
    Display,
    Region,
    Window,
}

/// A capture that failed after the tray, a shortcut or the capture menu
/// started it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CaptureFailure {
    pub error: String,
    /// Recording failures use shipping `report_recording_error`.
    pub recording: bool,
}

/// How the host reports a [`CaptureFailure`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FailureReport {
    /// Shipping answers a denied screenshot with permission recovery.
    PermissionRecovery,
    Dialog {
        title: &'static str,
        message: String,
    },
}

impl CaptureFailure {
    pub fn report(&self) -> FailureReport {
        use captures_app::capture_error;
        if self.recording {
            FailureReport::Dialog {
                title: capture_error::RECORDING_TITLE,
                message: self.error.clone(),
            }
        } else if captures_app::permission_recovery::is_permission_denied(&self.error) {
            FailureReport::PermissionRecovery
        } else {
            FailureReport::Dialog {
                title: capture_error::TITLE,
                message: capture_error::message(&self.error),
            }
        }
    }
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
    /// The menu first shows Record: enumerate microphones (shipping
    /// `loadAudioDevices`).
    ListMicrophones {
        generation: u64,
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
    Share {
        artifact_id: String,
        generation: u64,
    },
    /// Close, or an unsaved card's Delete (`delete`), which dissolves.
    Dismiss {
        artifact_id: String,
        generation: u64,
        delete: bool,
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

/// History drags deliberately bypass `PreviewMessage`: their source remains in
/// History regardless of the native drop outcome.
#[derive(Debug)]
struct HistoryDragFinished {
    artifact_id: String,
    result: Result<captures_app::preview::PreviewDragOutcome, String>,
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
    target: Option<CaptureTarget>,
    kind: crate::countdown::Kind,
    remaining: u8,
}

impl CountdownExit {
    fn new(
        viewport: egui::ViewportId,
        title: &'static str,
        target: impl Into<Option<CaptureTarget>>,
        kind: crate::countdown::Kind,
        remaining: u8,
    ) -> Self {
        Self {
            until: Instant::now() + Duration::from_millis(crate::countdown::CANCEL_LINGER_MS),
            viewport,
            title,
            target: target.into(),
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
    /// Pre-blurred copy for the shipping hover blur.
    blurred: Option<egui::TextureHandle>,
    /// Copy at the arrival's starting blur, cross-faded out as it lands.
    arrive_blurred: Option<egui::TextureHandle>,
    /// Card-sized sharp media at 2 px per point, the source of later blurs.
    media: Option<std::sync::Arc<egui::ColorImage>>,
    /// Pile depth blurs by radius (points), rebuilt as depths change.
    depth_blurred: Vec<(f32, egui::TextureHandle)>,
    size_bytes: u64,
    busy: Option<crate::mini_preview::Busy>,
    message: Option<String>,
    saved_path: Option<PathBuf>,
    /// Start of the brief "Saved" confirmation after an explicit save.
    saved_at: Option<Instant>,
    rejected_at: Option<Instant>,
    /// When the decoded image first painted: shipping `thumbnail-arrive`.
    arrived_at: Option<Instant>,
    /// Whether an editor window shows this capture ("In editor" pill, ring).
    editor: captures_app::preview_chrome::EditorPresence,
    /// The last clipboard copy of this capture failed ("Clipboard unavailable").
    copy_failed: bool,
}

impl PreviewCard {
    /// Build the pile media blurs for `depth` (fanned and resting radii),
    /// dropping radii no longer in use.
    fn prepare_depth_blurs(&mut self, ctx: &egui::Context, depth: usize) {
        let Some(media) = self.media.clone() else {
            return;
        };
        let radii = [true, false]
            .map(|hovered| captures_app::preview::collapsed_media_blur(depth, hovered) as f32);
        self.depth_blurred
            .retain(|(radius, _)| radii.contains(radius));
        for radius in radii {
            if radius > 0. && self.depth_blur(radius).is_none() {
                let image = crate::mini_preview::blurred_card_media(&media, radius);
                let texture = ctx.load_texture(
                    "mini-preview-depth-blur",
                    image,
                    egui::TextureOptions::LINEAR,
                );
                self.depth_blurred.push((radius, texture));
            }
        }
    }

    fn depth_blur(&self, radius: f32) -> Option<egui::TextureHandle> {
        self.depth_blurred
            .iter()
            .find(|(prepared, _)| *prepared == radius)
            .map(|(_, texture)| texture.clone())
    }
}

/// A card playing its exit animation after leaving the stack.
struct ExitingCard {
    texture: egui::TextureHandle,
    blurred: Option<egui::TextureHandle>,
    dust: std::sync::Arc<Vec<captures_app::preview_motion::DustParticle>>,
    media: Option<std::sync::Arc<egui::ColorImage>>,
    /// The Close streak's stepped blurs, built when the exit first paints.
    streak: Vec<egui::TextureHandle>,
    /// Delete's individually blurred chips, built when the exit first paints.
    dust_atlas: Option<(egui::TextureHandle, std::sync::Arc<Vec<egui::Rect>>)>,
}

#[derive(Clone)]
struct PreviewExitRender {
    texture: egui::TextureHandle,
    blurred: Option<egui::TextureHandle>,
    streak: Vec<egui::TextureHandle>,
    dust_atlas: Option<(egui::TextureHandle, std::sync::Arc<Vec<egui::Rect>>)>,
    kind: captures_app::preview_motion::ExitKind,
    elapsed_ms: f64,
    dust: std::sync::Arc<Vec<captures_app::preview_motion::DustParticle>>,
    layout: captures_app::preview::PreviewCardLayout,
    shift_y: f32,
}

/// The stack flying between the list and the compact pile.
#[derive(Clone, Copy, Debug)]
struct StackFly {
    collapsing: bool,
    started: Instant,
}

#[derive(Clone)]
struct PreviewRenderCard {
    artifact_id: String,
    generation: u64,
    width: u32,
    height: u32,
    texture: egui::TextureHandle,
    blurred: Option<egui::TextureHandle>,
    arrive_blurred: Option<egui::TextureHandle>,
    /// Pile media blurs at the hover and rest radii, ascending.
    depth_blurred: [(f32, Option<egui::TextureHandle>); 2],
    size_bytes: u64,
    busy: Option<crate::mini_preview::Busy>,
    message: Option<String>,
    saved_path: Option<PathBuf>,
    saved_at: Option<Instant>,
    clipboard_current: bool,
    editor_phase: captures_app::preview_chrome::EditorPhase,
    /// When `editor_phase` began.
    editor_since: Instant,
    rejected_at: Option<Instant>,
    arrived_at: Option<Instant>,
    /// Depth in the collapsed pile (0 = the front card).
    pile_depth: usize,
    layout: captures_app::preview::PreviewCardLayout,
    hover_y: f64,
    /// Collapsed pile position, for the list ↔ pile fly.
    pile_y: f64,
    /// Shipping pile depth poses (spin, recession, jitter) at rest and fanned.
    pile_rest: captures_app::preview::CollapsedCardPose,
    pile_hover: captures_app::preview::CollapsedCardPose,
    /// Survivor settle toward the stack anchor, points.
    shift_y: f32,
    copy_failed: bool,
}

/// How [`MiniPreviews::restore_artifact`] proceeds.
enum RestoreStart {
    AlreadyShowing,
    /// Decode this preview for the new card.
    Decode(PreviewGuard, PathBuf),
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
    /// Monotonic origin for the shared editor-presence clock.
    epoch: Instant,
    /// Bumped when an expand or a new card should hold card hover off until
    /// the pointer moves (shipping stale-pointer suppression).
    hover_lock_generation: u64,
    /// Cards holding their slots while their exits play.
    exits: captures_app::preview_motion::StackExits,
    exiting: HashMap<String, ExitingCard>,
    /// Saved cards whose Delete dissolves before their export moves to the
    /// Trash, as shipping orders it; a failed Trash puts them back.
    trashing: HashMap<String, TrashingCard>,
    toolbar: captures_app::preview_motion::StackToolbar,
    fly: Option<StackFly>,
}

/// A saved card between its dust and the Trash request's reply.
struct TrashingCard {
    card: PreviewCard,
    /// The card's slot in the stack, for a failed Trash.
    index: usize,
    /// The Trash request went out (after the dust played).
    sent: bool,
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
            epoch: Instant::now(),
            hover_lock_generation: 0,
            exits: Default::default(),
            exiting: HashMap::new(),
            trashing: HashMap::new(),
            toolbar: Default::default(),
            fly: None,
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
        let guard = self.insert_card(artifact, false, 0.);
        if target.is_some() {
            self.stack_target = target;
        }
        self.waiting_artifact = Some(artifact_id);
        Ok(Some((guard, artifact.preview_path.clone())))
    }

    /// Add the card for an artifact already inserted into `stack`.
    fn insert_card(&mut self, artifact: &Artifact, editor_open: bool, now_ms: f64) -> PreviewGuard {
        let artifact_id = artifact.entry.id.clone();
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
                blurred: None,
                arrive_blurred: None,
                media: None,
                depth_blurred: Vec::new(),
                size_bytes: artifact.entry.size_bytes,
                busy: None,
                message: None,
                saved_path: artifact.entry.saved_path.as_deref().map(PathBuf::from),
                saved_at: None,
                rejected_at: None,
                arrived_at: None,
                editor: captures_app::preview_chrome::EditorPresence::new(editor_open, now_ms),
                copy_failed: false,
            },
        );
        PreviewGuard {
            artifact_id,
            generation,
        }
    }

    /// Shipping History Restore (`restore_history_artifact`): bring a
    /// screenshot back as the front card without a capture generation or
    /// clipboard copy. A card already in the stack stays where it is, like
    /// shipping, which never duplicates or reorders it. An empty stack opens
    /// on `target`; otherwise the pile keeps its display and position.
    fn restore_artifact(
        &mut self,
        artifact: &Artifact,
        settings: &AppSettings,
        target: Option<CaptureTarget>,
        editor_open: bool,
    ) -> Result<RestoreStart, String> {
        if self.cards.contains_key(&artifact.entry.id) {
            return Ok(RestoreStart::AlreadyShowing);
        }
        // The per-frame settings sync applies show/placement changes.
        if self.stack.ids().is_empty() {
            let target = target.filter(|target| target.preview_bounds.is_some());
            if target.is_none() && settings.show_mini_previews {
                return Err("Mini-preview positioning is unavailable for this display.".into());
            }
            self.stack_target = target;
        }
        if !self.stack.insert(artifact.entry.id.clone()) {
            return Ok(RestoreStart::AlreadyShowing);
        }
        let now_ms = crate::motion::elapsed_ms(self.epoch, Instant::now());
        let guard = self.insert_card(artifact, editor_open, now_ms);
        Ok(RestoreStart::Decode(guard, artifact.preview_path.clone()))
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
        self.exits.sync(self.stack.ids());
        self.release_empty_stack();
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
        self.exits.sync(self.stack.ids());
        self.release_empty_stack();
        removed
    }

    /// An empty stack forgets its display and dragged origin once its last
    /// exit has played.
    fn release_empty_stack(&mut self) {
        if self.stack.ids().is_empty() && self.exiting.is_empty() && self.trashing.is_empty() {
            self.stack_target = None;
            self.visibility.clear_stack_origin();
        }
    }

    fn now_ms(&self) -> f64 {
        crate::motion::elapsed_ms(self.epoch, Instant::now())
    }

    /// Start `artifact_id`'s shipping exit before it leaves the stack, when
    /// the expanded stack is on screen and motion is allowed. The card keeps
    /// its slot and paints its exit until [`Self::settle_exits`] drops it.
    fn begin_exit(
        &mut self,
        artifact_id: &str,
        kind: captures_app::preview_motion::ExitKind,
        delay_ms: f64,
        settles: bool,
        reduced_motion: bool,
    ) {
        use captures_app::preview::{THUMBNAIL_CARD_HEIGHT, THUMBNAIL_PADDING, THUMBNAIL_WIDTH};
        use captures_app::preview_motion::{self, ExitKind};
        if reduced_motion || self.stack.is_collapsed() || self.fly.is_some() || !self.is_visible() {
            return;
        }
        let Some(card) = self.cards.get(artifact_id) else {
            return;
        };
        let Some(texture) = card.texture.clone() else {
            return;
        };
        let dust = if kind == ExitKind::Dust {
            let width = THUMBNAIL_WIDTH - THUMBNAIL_PADDING * 2.;
            preview_motion::dust_particles(
                width,
                THUMBNAIL_CARD_HEIGHT,
                (f64::from(card.width), f64::from(card.height)),
                (
                    preview_motion::delete_origin_x(
                        width,
                        card.saved_path.is_some(),
                        self.placement.is_right(),
                    ),
                    preview_motion::DELETE_ORIGIN_Y,
                ),
                card.generation as u32,
            )
        } else {
            Vec::new()
        };
        let blurred = card.blurred.clone();
        let media = card.media.clone();
        let live = self.stack.ids().to_vec();
        let now = self.now_ms();
        if self.exits.begin(
            &live,
            artifact_id,
            kind,
            now,
            delay_ms,
            settles,
            &preview_motion::settle_tween(),
        ) {
            crate::diagnostics::event("preview-exit", || {
                serde_json::json!({"id":artifact_id, "kind":format!("{kind:?}"),
                    "started_ms":now, "delay_ms":delay_ms, "settles":settles})
            });
            self.exiting.insert(
                artifact_id.to_owned(),
                ExitingCard {
                    texture,
                    blurred,
                    dust: std::sync::Arc::new(dust),
                    media,
                    streak: Vec::new(),
                    dust_atlas: None,
                },
            );
        }
    }

    /// Close or Delete from a card: play the exit, then leave the stack. A
    /// Close or Delete that leaves fewer than two live cards plays the
    /// toolbar's exit.
    fn exit_card(
        &mut self,
        artifact_id: &str,
        kind: captures_app::preview_motion::ExitKind,
        reduced_motion: bool,
    ) -> bool {
        self.begin_exit(artifact_id, kind, 0., true, reduced_motion);
        self.remove_after_exit(artifact_id)
    }

    fn remove_after_exit(&mut self, artifact_id: &str) -> bool {
        let removed = self.remove(artifact_id);
        if removed && self.stack.ids().len() < 2 {
            let now = self.now_ms();
            self.toolbar
                .set(false, captures_app::preview_motion::ToolbarCause::Exit, now);
        }
        removed
    }

    /// A saved card's Delete: shipping dissolves it first and moves its
    /// export to the Trash once the dust has played and the stack settled.
    /// Returns whether the request can go out now (no exit plays).
    fn begin_trash(&mut self, artifact_id: &str, reduced_motion: bool) -> bool {
        let Some(index) = self.stack.ids().iter().position(|id| id == artifact_id) else {
            return false;
        };
        self.begin_exit(
            artifact_id,
            captures_app::preview_motion::ExitKind::Dust,
            0.,
            true,
            reduced_motion,
        );
        let Some(card) = self.cards.remove(artifact_id) else {
            return false;
        };
        let now = !self.exiting.contains_key(artifact_id);
        self.trashing.insert(
            artifact_id.to_owned(),
            TrashingCard {
                card,
                index,
                sent: now,
            },
        );
        self.remove_after_exit(artifact_id);
        now
    }

    /// Trash requests whose dust has finished: `(id, generation, saved path)`.
    /// The dissolved card keeps its (empty) slot until the reply, as
    /// shipping keeps the card until `trash_artifact` resolves.
    fn due_trash(&mut self, reduced_motion: bool) -> Vec<(String, u64, Option<PathBuf>)> {
        let now = self.now_ms();
        let exits = &self.exits;
        self.trashing
            .iter_mut()
            .filter(|(id, pending)| {
                !pending.sent
                    && exits
                        .exiting(id)
                        .is_none_or(|exit| exit.finished(now, reduced_motion))
            })
            .map(|(id, pending)| {
                pending.sent = true;
                (
                    id.clone(),
                    pending.card.generation,
                    pending.card.saved_path.clone(),
                )
            })
            .collect()
    }

    /// The export is in the Trash: forget the dissolved card.
    fn finish_trash(&mut self, artifact_id: &str, generation: u64) -> bool {
        if self
            .trashing
            .get(artifact_id)
            .is_none_or(|pending| pending.card.generation != generation)
        {
            return false;
        }
        self.trashing.remove(artifact_id);
        self.exits.release(artifact_id, false);
        self.exiting.remove(artifact_id);
        self.release_empty_stack();
        true
    }

    /// Trash failed: like shipping's unlocked card, it returns to its slot
    /// with the error, available to retry.
    fn restore_trashed(&mut self, artifact_id: &str, generation: u64, message: String) -> bool {
        if self
            .trashing
            .get(artifact_id)
            .is_none_or(|pending| pending.card.generation != generation)
        {
            return false;
        }
        let Some(TrashingCard {
            mut card, index, ..
        }) = self.trashing.remove(artifact_id)
        else {
            return false;
        };
        // The held slot becomes the card's again.
        self.exits.release(artifact_id, true);
        self.exiting.remove(artifact_id);
        if !self.stack.restore(artifact_id.to_owned(), index) {
            self.exits.sync(self.stack.ids());
            return false;
        }
        card.busy = None;
        card.message = Some(message);
        self.cards.insert(artifact_id.to_owned(), card);
        self.exits.sync(self.stack.ids());
        true
    }

    /// Clear all: every card streaks out, bottom first, without settling.
    fn clear_with_exit(&mut self, artifact_ids: &[String], reduced_motion: bool) -> usize {
        let live = self.stack.ids().to_vec();
        let top_anchor = self.placement.is_top();
        for (index, artifact_id) in live.iter().enumerate() {
            if artifact_ids.contains(artifact_id) {
                let delay =
                    captures_app::preview_motion::clear_delay_ms(live.len(), index, top_anchor);
                self.begin_exit(
                    artifact_id,
                    captures_app::preview_motion::ExitKind::Dismiss,
                    delay,
                    false,
                    reduced_motion,
                );
            }
        }
        let removed = self.clear(artifact_ids);
        if removed > 0 {
            let now = self.now_ms();
            self.toolbar.set(
                false,
                captures_app::preview_motion::ToolbarCause::Clear,
                now,
            );
        }
        removed
    }

    /// Drop exits and stack flights that finished. Returns whether any did.
    fn settle_exits(&mut self, reduced_motion: bool) -> bool {
        let now = self.now_ms();
        let trashing = &self.trashing;
        let mut changed = self
            .exits
            .prune_holding(now, reduced_motion, &|id| trashing.contains_key(id));
        let exits = &self.exits;
        let before = self.exiting.len();
        self.exiting.retain(|id, _| exits.exiting(id).is_some());
        changed |= self.exiting.len() != before;
        if let Some(fly) = self.fly
            && (reduced_motion || fly.started.elapsed() >= STACK_FLY)
        {
            self.fly = None;
            changed = true;
        }
        if changed {
            self.release_empty_stack();
        }
        changed
    }

    /// Finish every exit at once (the stack collapsed or closed).
    fn finish_exits(&mut self) {
        self.exits.clear();
        self.exiting.clear();
        self.release_empty_stack();
    }

    /// Show less / expand, with the shipping fly and toolbar motion.
    fn toggle_collapsed(&mut self, reduced_motion: bool) {
        use captures_app::preview_motion::ToolbarCause;
        let collapse = !self.stack.is_collapsed();
        self.finish_exits();
        self.stack.set_collapsed(collapse);
        let now = self.now_ms();
        if collapse {
            self.toolbar.set(false, ToolbarCause::Collapse, now);
        } else {
            self.toolbar
                .set(self.stack.ids().len() >= 2, ToolbarCause::Expand, now);
        }
        self.fly = (!reduced_motion && self.is_visible()).then(|| StackFly {
            collapsing: collapse,
            started: Instant::now(),
        });
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
                .count()
                + self.exiting.len(),
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
/// `THUMBNAIL_STACK_EXPAND_COLLAPSE_MS` ([`Transition::PreviewStackFly`]).
///
/// [`Transition::PreviewStackFly`]: captures_app::motion::Transition::PreviewStackFly
const STACK_FLY: Duration = Duration::from_millis(520);

/// Shipping `recording-countdown-fade-in` / `-content-in`, timed from the
/// first paint of this countdown `key`. Requests frames only while moving.
fn countdown_entrance(
    ctx: &egui::Context,
    t: &Tokens,
    key: (&'static str, u64),
    reduced_motion: bool,
) -> crate::countdown::Poses {
    let id = egui::Id::unique(("countdown-entrance", key));
    let shown = ctx.data_mut(|data| *data.get_temp_mut_or_insert_with(id, Instant::now));
    let elapsed = crate::motion::elapsed_ms(shown, Instant::now());
    let (poses, moving) = crate::countdown::Poses::entrance(t, elapsed, reduced_motion);
    if moving {
        ctx.request_repaint();
    }
    poses
}

/// Shipping keeps the controls-hidden notice window for 6.2 s.
const RECORDING_HIDDEN_NOTICE_MS: f64 = 6_200.;

/// Where the mini preview viewport keeps its collapsed pile's
/// [`captures_app::preview_motion::StackFan`].
fn fan_id() -> egui::Id {
    egui::Id::unique("mini-preview-hover-fan")
}

fn carry_id() -> egui::Id {
    egui::Id::unique("mini-preview-pile-carry")
}

/// The collapsed pile being carried: shipping holds the fanned pose until the
/// fan has gathered (`thumbnailStackFanCollapseMs`), then leans the rear cards
/// with [`captures_app::preview_motion::DragSway`]; dropping lets the lean
/// ease back over the fan's staggered transition.
#[derive(Clone, Copy, Debug, Default)]
struct PileCarry {
    /// When the press became a drag (`input.time`-based ms), until release.
    since: Option<f64>,
    /// The lean is live.
    ready: bool,
    sway: captures_app::preview_motion::DragSway,
    /// The last desktop pointer sample and when.
    last: Option<(egui::Pos2, f64)>,
    /// The lean at the drop, and when it began easing back.
    released: Option<((f64, f64), f64)>,
}

impl PileCarry {
    fn start(&mut self, now_ms: f64) {
        if self.since.is_none() {
            *self = Self {
                since: Some(now_ms),
                ..Self::default()
            };
        }
    }

    /// Advance one frame and return each depth's lean now.
    fn frame(
        &mut self,
        now_ms: f64,
        pressed: bool,
        pointer: Option<egui::Pos2>,
        gather_ms: f64,
        reduced: bool,
    ) -> impl Fn(usize, &captures_app::motion::Tween) -> (f64, f64) + use<> {
        if let Some(since) = self.since {
            if !pressed {
                let lean = self.sway.offset();
                *self = Self {
                    released: (lean != (0., 0.)).then_some((lean, now_ms)),
                    ..Self::default()
                };
            } else {
                if !self.ready && (reduced || now_ms - since >= gather_ms) {
                    self.ready = true;
                    self.sway = Default::default();
                    self.last = pointer.map(|point| (point, now_ms));
                }
                if self.ready
                    && let Some(point) = pointer
                {
                    let (step, dt) = self.last.map_or(
                        (
                            egui::Vec2::ZERO,
                            captures_app::preview_motion::DRAG_SWAY_DEFAULT_DT_MS,
                        ),
                        |(last, at)| (point - last, now_ms - at),
                    );
                    self.sway
                        .tick(f64::from(step.x), f64::from(step.y), dt, reduced);
                    self.last = Some((point, now_ms));
                }
            }
        }
        let (ready, sway, released) = (self.ready, self.sway.offset(), self.released);
        move |depth, tween| {
            if ready {
                return sway;
            }
            let Some(((x, y), at)) = released else {
                return (0., 0.);
            };
            let delay = captures_app::preview_motion::fan_delay_ms(depth);
            let left = 1. - tween.progress(now_ms - at - delay, reduced);
            (x * left, y * left)
        }
    }

    /// Whether the lean still moves (the root keeps painting until it rests).
    fn moving(
        &mut self,
        now_ms: f64,
        deepest: usize,
        tween: &captures_app::motion::Tween,
        reduced: bool,
    ) -> bool {
        if let Some((_, at)) = self.released {
            let delay = captures_app::preview_motion::fan_delay_ms(deepest);
            if tween.running(now_ms - at - delay, reduced) {
                return true;
            }
            self.released = None;
        }
        // Before the lean starts, the carry only waits for its gather.
        (self.since.is_some() && !self.ready) || (self.ready && !self.sway.settled())
    }
}

pub(crate) fn request_hidden_viewport_paint(ctx: &egui::Context, viewport: egui::ViewportId) {
    if ctx.data(|data| data.get_temp::<bool>(egui::Id::unique("wayland-surface"))) == Some(true) {
        // Presenting a buffer would remap an excluded Wayland surface. Eframe
        // runs hidden-root logic (and UI for visible descendants) without painting.
        ctx.request_repaint_of(viewport);
    } else {
        ctx.send_viewport_cmd_to(viewport, egui::ViewportCommand::RequestPaintWhileHidden);
    }
}

pub(crate) fn request_hidden_root_paint(ctx: &egui::Context) {
    request_hidden_viewport_paint(ctx, egui::ViewportId::ROOT);
}

/// Declare a new child in a root UI pass. The private renderer runs this pass
/// without presenting an unmapped Wayland root; worker wakes remain logic-only.
pub(crate) fn request_hidden_root_ui(ctx: &egui::Context) {
    ctx.send_viewport_cmd_to(
        egui::ViewportId::ROOT,
        egui::ViewportCommand::RequestPaintWhileHidden,
    );
    ctx.request_repaint_of(egui::ViewportId::ROOT);
}

#[cfg(target_os = "linux")]
struct PortalScreenshot {
    flow: captures_app::capture_flow::PortalCapture,
    hidden: Vec<egui::ViewportId>,
    restore_root: bool,
    recording: bool,
    started: Instant,
    submitted: bool,
    countdown: Option<captures_app::capture_flow::Countdown>,
    countdown_hidden_frame: Option<u64>,
}

#[cfg(target_os = "linux")]
impl PortalScreenshot {
    fn countdown_hidden(&mut self, now: Instant, frame: u64) -> bool {
        let Some(clock) = self.countdown else {
            return true;
        };
        if clock.remaining(now) > 0 {
            return false;
        }
        let Some(hidden) = self.countdown_hidden_frame else {
            self.countdown_hidden_frame = Some(frame);
            // The unmap timeout starts after the configured delay, not launch.
            self.started = now;
            return false;
        };
        frame > hidden
    }
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
    /// A hidden editor must finish its draft before this capture opens again.
    pending_editor_opens: HashMap<String, (PathBuf, captures_capture::CaptureMode)>,
    recording_editors: HashMap<String, crate::recording_editor::Editor>,
    /// Latest Preferences snapshot, used only when opening a new editor.
    recording_editor_preferences: Result<captures_settings::RecordingSettings, String>,
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
    /// History Restore waiting for its preview card to decode ("Restoring…").
    card_restoring: Option<PreviewGuard>,
    /// Shipping "✓ Restored" feedback, shown for `ACTION_FEEDBACK_MS`.
    card_restored: Option<(String, Instant)>,
    /// Shipping `HistoryCard` errors (`.history-card-error`) from a failed
    /// Restore or Edit restore, by artifact, until that card acts again.
    card_errors: HashMap<String, String>,
    /// The latest progress message, for tests and diagnostics. Shipping
    /// History has no status line; failures show in `error` (`.history-error`).
    status: String,
    error: Option<String>,
    pending: usize,
    open_media: VecDeque<(PathBuf, PathBuf)>,
    media_open_failed: bool,
    opening_media: bool,
    media_open_errors: Vec<String>,
    capture_waiting_for_hide: bool,
    /// A capture has hidden the workspace windows (History and Preferences)
    /// and has not restored them yet.
    workspace_hidden: bool,
    /// The host's Preferences window is still shown.
    companion_visible: bool,
    /// Whether the host means History to be on screen (not closed to the
    /// tray or launched hidden).
    root_shown: bool,
    hide_started: Option<Instant>,
    hidden_since: Option<Instant>,
    capture_in_flight: bool,
    auto_copy_on_capture: bool,
    include_cursor: bool,
    flow: Option<CaptureFlow>,
    capture_phase: Option<CapturePhase>,
    countdown_target: Option<CaptureTarget>,
    /// Windows unmapped before portal recording; restore only after the take.
    recording_portal_windows: Vec<egui::ViewportId>,
    /// Keeps a cancelled countdown up briefly with the shipping "Cancelling…" copy.
    countdown_exit: Option<CountdownExit>,
    region_session: Option<Box<RegionSession>>,
    region_texture: Option<egui::TextureHandle>,
    region_selector: Arc<Mutex<Selector>>,
    selector_tx: Sender<SelectorMessage>,
    selector_rx: Receiver<SelectorMessage>,
    preview_tx: Sender<PreviewMessage>,
    preview_rx: Receiver<PreviewMessage>,
    history_drag_tx: Sender<HistoryDragFinished>,
    history_drag_rx: Receiver<HistoryDragFinished>,
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
    sharing: crate::sharing::Window,
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
    /// The menu is preparing another display the user chose. Only that
    /// auto-starts Full screen; opening on Full screen waits for a choice.
    controls_switching_display: bool,
    /// The menu being replaced by a display switch, which stays up showing
    /// "Switching…" until the new display is ready (shipping
    /// `select_capture_display`).
    controls_switch_from: Option<(
        CaptureTarget,
        Arc<WindowSession>,
        Option<egui::TextureHandle>,
    )>,
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
    recording_screenshot_window_session: Option<Arc<WindowSession>>,
    recording_screenshot_texture: Option<egui::TextureHandle>,
    recording_screenshot_settings: Option<AppSettings>,
    recording_selector_shot: Option<RecordingSelectorShot>,
    recording_display_shot: Option<RecordingDisplayShot>,
    recording_controls_hidden: Option<u64>,
    recording_hidden_notice_until: Option<Instant>,
    /// The saved New Capture shortcut, for the hidden-controls notice's chips.
    new_capture_shortcut: String,
    recording_restore_available: bool,
    history_refresh_status: Option<String>,
    can_hide: Option<bool>,
    /// Shipping two-step deletion: the armed card and when it reverts.
    confirm_delete: Option<(String, Instant)>,
    confirm_clear_history: Option<Instant>,
    clearing_history: bool,
    requested_capture: Option<CaptureRequest>,
    /// A capture action asked to recapture the open UI; launched with the
    /// next frame's display list.
    requested_recapture: Option<Recapture>,
    /// The capture menu's Screenshot target as its shortcuts and tray items
    /// last set it, or `None` in Record mode. Like shipping's selection
    /// summary (`open_menu_screenshot_target`), toolbar clicks and window
    /// picks inside the menu leave it unchanged.
    menu_screenshot_target: Option<captures_app::capture_error::Target>,
    recapture: Option<PendingRecapture>,
    /// New Capture brought the recording controls back during a screenshot
    /// beside the take (shipping `restore_hidden_recording_controls`).
    recording_screenshot_controls_restored: bool,
    restore_root_visible: bool,
    /// A failed capture awaiting the host's shipping error dialog (or
    /// permission recovery). Never shown in the History error card.
    capture_failure: Option<CaptureFailure>,
    #[cfg(target_os = "linux")]
    portal_screenshot: Option<PortalScreenshot>,
    history_requested: bool,
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
        let (history_drag_tx, history_drag_rx) = mpsc::channel();
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
                    #[cfg(target_os = "linux")]
                    Job::CapturePortal { root, generation } => Reply::PortalCaptured {
                        generation,
                        result: captures_app::capture_portal_screenshot(&root, generation)
                            .map(|artifact| artifact.map(Box::new))
                            .map_err(|error| error.to_string()),
                    },
                    Job::Execute {
                        request,
                        preview,
                        notice,
                    } => {
                        let clearing = matches!(request, Request::ClearHistory { .. });
                        let listing = matches!(request, Request::Displays);
                        let result = captures_app::execute(request)
                            .map(Box::new)
                            .map_err(|error| error.to_string());
                        if clearing {
                            Reply::HistoryCleared(result)
                        } else if listing {
                            Reply::DisplaysListed(result)
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
                    Job::CaptureDisplay {
                        root,
                        display_id,
                        generation,
                        include_cursor,
                    } => Reply::DisplayCaptured {
                        generation,
                        result: captures_app::execute(Request::CaptureDisplay {
                            root,
                            display_id,
                            generation,
                            include_cursor,
                        })
                        .map_err(|error| error.to_string())
                        .and_then(|response| match response {
                            Response::Captured { artifact } => Ok(Box::new(artifact)),
                            _ => Err("The display capture returned no screenshot.".into()),
                        }),
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
                    } => {
                        let result = decode(&path);
                        let media = result.as_ref().ok().and_then(|decoded| {
                            crate::mini_preview::card_media_image(&decoded.image)
                        });
                        let blurred = media.as_ref().map(crate::mini_preview::hover_blur_image);
                        let arrive_blurred =
                            media.as_ref().map(crate::mini_preview::arrive_blur_image);
                        Reply::PreviewDecoded {
                            generation,
                            artifact_id,
                            result,
                            blurred,
                            arrive_blurred,
                            media,
                        }
                    }
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
            pending_editor_opens: HashMap::new(),
            recording_editors: HashMap::new(),
            recording_editor_preferences: Err("Recording preferences are still loading.".into()),
            history_filter: HistoryFilter::All,
            selection: Selection::default(),
            history_cards: HashMap::new(),
            history_thumbnails: HashMap::new(),
            history_loaded: false,
            history_scroll_to: None,
            card_busy: None,
            card_restoring: None,
            card_restored: None,
            card_errors: HashMap::new(),
            status: "Loading capture workspace…".into(),
            error: None,
            pending: 0,
            open_media: VecDeque::new(),
            media_open_failed: false,
            opening_media: false,
            media_open_errors: Vec::new(),
            capture_waiting_for_hide: false,
            workspace_hidden: false,
            companion_visible: false,
            root_shown: true,
            hide_started: None,
            hidden_since: None,
            capture_in_flight: false,
            auto_copy_on_capture: false,
            include_cursor: false,
            flow: None,
            capture_phase: None,
            countdown_target: None,
            recording_portal_windows: Vec::new(),
            countdown_exit: None,
            region_session: None,
            region_texture: None,
            region_selector: Arc::new(Mutex::new(Selector::default())),
            selector_tx,
            selector_rx,
            preview_tx,
            preview_rx,
            history_drag_tx,
            history_drag_rx,
            notice_tx,
            notice_rx,
            recording_notice: None,
            recording_notice_generation: 0,
            recording_notice_target: None,
            open_editor_after_recording: None,
            recording_editor_notice_targets: HashMap::new(),
            last_capture_target: None,
            previews: MiniPreviews::default(),
            sharing: crate::sharing::Window::default(),
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
            controls_switching_display: false,
            controls_switch_from: None,
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
            recording_screenshot_window_session: None,
            recording_screenshot_texture: None,
            recording_screenshot_settings: None,
            recording_selector_shot: None,
            recording_display_shot: None,
            recording_controls_hidden: None,
            recording_hidden_notice_until: None,
            new_capture_shortcut: String::new(),
            recording_restore_available: false,
            history_refresh_status: None,
            can_hide: None,
            confirm_delete: None,
            confirm_clear_history: None,
            clearing_history: false,
            requested_capture: None,
            requested_recapture: None,
            menu_screenshot_target: None,
            recapture: None,
            recording_screenshot_controls_restored: false,
            restore_root_visible: true,
            capture_failure: None,
            #[cfg(target_os = "linux")]
            portal_screenshot: None,
            history_requested: false,
            preference_target_requested: None,
            controls_error: None,
            permission_recovery_visible: false,
        };
        live.load_history();
        if ctx.data(|data| data.get_temp::<bool>(egui::Id::unique("wayland-surface"))) != Some(true)
        {
            live.send(Request::Displays);
        }
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
            self.media_open_failed = true;
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
        if self.editors.get(&id).is_some_and(|editor| editor.closing()) {
            self.pending_editor_opens
                .insert(id, (output_directory, mode));
            return;
        }
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

    pub fn set_recording_editor_preferences(
        &mut self,
        preferences: Result<captures_settings::RecordingSettings, String>,
    ) {
        self.recording_editor_preferences = preferences;
    }

    fn open_recording_editor(
        &mut self,
        ctx: &egui::Context,
        id: String,
        output_directory: PathBuf,
    ) {
        // Refocusing an existing editor never replaces its staged edits with
        // newly saved application defaults.
        if let Some(editor) = self.recording_editors.get(&id) {
            editor.focus(ctx);
            return;
        }
        let preferences = match &self.recording_editor_preferences {
            Ok(preferences) => preferences.clone(),
            Err(error) => {
                self.error = Some(format!("Could not load recording preferences: {error}"));
                self.media_open_failed = true;
                return;
            }
        };
        self.recording_editors
            .entry(id.clone())
            .or_insert_with(|| {
                crate::recording_editor::Editor::open(
                    ctx,
                    self.root.clone(),
                    id,
                    output_directory,
                    preferences,
                )
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
                self.media_open_failed = true;
            }
        }
    }

    pub fn is_capturing(&self) -> bool {
        self.flow.is_some() || self.capture_in_flight
    }

    pub fn take_history_requested(&mut self) -> bool {
        std::mem::take(&mut self.history_requested)
    }

    pub fn can_launch_capture(&self) -> bool {
        self.pending == 0
            && !self.recovery.blocking()
            && !self.permission_recovery_visible
            && !self.is_capturing()
            && self.requested_capture.is_none()
    }

    /// The failed capture the host reports next, once. Shipping reports
    /// every failed tray, shortcut or menu capture in a modal dialog
    /// (`report_capture_error`); denied permissions open recovery instead.
    pub fn take_capture_failure(&mut self) -> Option<CaptureFailure> {
        self.capture_failure.take()
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

    /// While true the host hides its other workspace windows too.
    pub fn workspace_hidden(&self) -> bool {
        self.workspace_hidden
    }

    /// Whether the host's Preferences window is still on screen. A capture
    /// waits for it to hide as well as the root.
    pub fn set_companion_visible(&mut self, visible: bool) {
        self.companion_visible = visible;
    }

    /// Whether History should be on screen when not hidden for a capture.
    pub fn set_root_shown(&mut self, shown: bool) {
        self.root_shown = shown;
    }

    /// Whether a screenshot or recording editor window is open. Shipping
    /// reopen focuses one before History and Preferences.
    pub fn has_open_editor(&self) -> bool {
        self.editors.values().any(|editor| !editor.closed())
            || self
                .recording_editors
                .values()
                .any(|editor| !editor.closed())
    }

    /// Show, restore and focus one open editor window (the first by artifact
    /// id). Returns false when none is open.
    pub fn focus_open_editor(&self, ctx: &egui::Context) -> bool {
        let screenshot = self
            .editors
            .iter()
            .filter(|(_, editor)| !editor.closed())
            .min_by_key(|(id, _)| id.as_str());
        let recording = self
            .recording_editors
            .iter()
            .filter(|(_, editor)| !editor.closed())
            .min_by_key(|(id, _)| id.as_str());
        match (screenshot, recording) {
            (Some((screenshot_id, screenshot)), Some((recording_id, recording))) => {
                if screenshot_id <= recording_id {
                    screenshot.focus(ctx);
                } else {
                    recording.focus(ctx);
                }
            }
            (Some((_, screenshot)), None) => screenshot.focus(ctx),
            (None, Some((_, recording))) => recording.focus(ctx),
            (None, None) => return false,
        }
        true
    }

    /// Whether opening external media failed since the last call. A media
    /// launch leaves History hidden, so the host shows it for the error.
    pub fn take_media_open_failed(&mut self) -> bool {
        std::mem::take(&mut self.media_open_failed)
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
        #[cfg(target_os = "linux")]
        if self.portal_screenshot.is_some() {
            return false; // Do not remap excluded controls during the portal round trip.
        }
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

    fn can_start_capture(&self) -> bool {
        self.can_launch_capture() && self.can_hide == Some(true)
    }

    /// The request for the display shortcut and tray "Screenshot Display",
    /// or `None` when shipping refuses it silently (`CaptureInProgress`).
    pub fn display_request(&self) -> Option<CaptureRequest> {
        use captures_app::capture_error::{DisplayRoute, display_route};
        match display_route(recording_route_state(
            self.capture_phase,
            self.recording_has_started,
        )) {
            DisplayRoute::CaptureMenu => Some(CaptureRequest::DisplayMenu),
            DisplayRoute::CaptureDisplay => Some(CaptureRequest::Display),
            DisplayRoute::Ignore => None,
        }
    }

    /// A running or paused recording can take a region, window or display
    /// screenshot now. Routes those shortcuts past the recording's capture flow.
    pub fn recording_screenshot_available(&self) -> bool {
        #[cfg(target_os = "linux")]
        if self.portal_screenshot.is_some() {
            return false;
        }
        matches!(
            self.capture_phase,
            Some(CapturePhase::Recording | CapturePhase::RecordingPaused)
        ) && self.recording_screenshot_flow.is_none()
            && self.requested_capture.is_none()
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
        if is_recording_phase(self.capture_phase) {
            match request {
                CaptureRequest::Display | CaptureRequest::Region | CaptureRequest::Window => {
                    // Beside the recording, not instead of it. A take that is
                    // not running or paused, or already taking one, refuses
                    // silently (shipping `screenshot_capture_is_blocked`).
                    if self.recording_screenshot_available() {
                        self.requested_capture = Some(request);
                    }
                    return;
                }
                CaptureRequest::NewCapture => {
                    use captures_app::capture_error::{
                        CAPTURE_IN_PROGRESS, NewCaptureRoute, new_capture_route,
                    };
                    // Hosts bring hidden controls back first; with them
                    // showing, shipping reports the busy session in the
                    // capture error dialog (`open_capture_controls`).
                    let state =
                        recording_route_state(self.capture_phase, self.recording_has_started);
                    if new_capture_route(state, false) == NewCaptureRoute::InProgress {
                        self.capture_failed(CAPTURE_IN_PROGRESS.into());
                        return;
                    }
                }
                CaptureRequest::DisplayMenu | CaptureRequest::Recording(_) => {}
            }
        }
        if self.is_capturing() || self.requested_capture.is_some() {
            // Shipping ignores a capture that arrives while another one owns
            // the flow (`CaptureInProgress`); no dialog, no History error.
        } else if !self.can_launch_capture() {
            self.capture_failed("Another capture or history action is still in progress.".into());
        } else {
            self.requested_capture = Some(request);
        }
    }

    /// Opens the capture menu's state for the current flow, in Record mode
    /// on `target` when `record`, and otherwise in Screenshot mode on it.
    fn configure_capture_menu(
        &mut self,
        settings: &AppSettings,
        record: bool,
        target: capture_controls::TargetMode,
    ) {
        self.selector_scope_generation.store(0, Ordering::Release);
        self.controls_error = None;
        self.controls_freeze = settings.freeze_screen;
        self.controls_auto_start = settings.auto_start_on_selection;
        self.controls_switching_display = false;
        self.controls_switch_from = None;
        self.controls_countdown_seconds = settings.screenshot_countdown_seconds;
        self.include_recording_controls = settings.include_recording_controls_in_captures;
        let mut controls = self.controls.lock().unwrap();
        controls.reset();
        controls.configure_recording(
            &settings.recording,
            RecordingCapabilities::current(settings.include_recording_controls_in_captures),
        );
        match (record, target) {
            (true, target) => controls.select_recording_target(target),
            (false, capture_controls::TargetMode::Region) => {}
            (false, capture_controls::TargetMode::Window) => {
                controls.apply_target_shortcut(CaptureShortcut::Window)
            }
            (false, capture_controls::TargetMode::Display) => {
                controls.apply_target_shortcut(CaptureShortcut::Display)
            }
        }
        drop(controls);
        self.menu_screenshot_target = (!record).then_some(target.target());
        self.recording_toolchain_ready = false;
        self.recording_toolchain_error = None;
        self.recording_has_started = false;
        let generation = self
            .flow
            .as_ref()
            .expect("the capture menu owns the flow")
            .generation();
        self.recording_worker
            .send(recording::Command::VerifyToolchain { generation });
    }

    /// What is open or in flight for `capture_error::busy_route`.
    fn capture_activity(&self) -> captures_app::capture_error::Activity {
        capture_activity(
            self.recording_screenshot_flow
                .is_some()
                .then_some(self.recording_screenshot_phase),
            self.capture_phase,
            || self.menu_screenshot_target,
            self.flow.is_some() || self.capture_in_flight,
        )
    }

    /// The take's controls are showing now: neither hidden by the user nor
    /// concealed for a screenshot beside it.
    fn recording_controls_on_screen(&self) -> bool {
        !self.recording_controls_hidden()
            && (self.recording_screenshot_flow.is_none()
                || self.recording_screenshot_keeps_controls())
    }

    /// A capture shortcut or tray item, routed like shipping: with nothing
    /// open it starts, and while a capture is open or in flight it follows
    /// `capture_error::busy_route` (recapture, switch the menu, restore the
    /// controls, report the busy capture or refuse silently).
    pub fn capture_action(
        &mut self,
        action: captures_app::capture_error::Action,
        ctx: &egui::Context,
    ) {
        use captures_app::capture_error::{
            Action, BusyRoute, CAPTURE_IN_PROGRESS, Target, busy_route,
        };
        #[cfg(target_os = "linux")]
        if self.portal_screenshot.is_some() {
            // A busy error dialog or New Capture restore would enter the still.
            return;
        }
        if self.recapture.is_some() || self.requested_recapture.is_some() {
            // The open UI is already being recaptured.
            return;
        }
        let route = busy_route(
            action,
            self.capture_activity(),
            recording_route_state(self.capture_phase, self.recording_has_started),
            self.recording_controls_on_screen(),
        );
        let selector_kind = |target| match target {
            Target::Window => RecordingSelectorKind::Window,
            Target::Region | Target::Display => RecordingSelectorKind::Region,
        };
        match route {
            BusyRoute::Idle => match action {
                Action::NewCapture => {
                    if !self.show_recording_controls(ctx) {
                        self.request_capture(CaptureRequest::NewCapture);
                    }
                }
                Action::Screenshot(Target::Display) => {
                    if let Some(request) = self.display_request() {
                        self.request_capture(request);
                    }
                }
                Action::Screenshot(Target::Region) => self.request_capture(CaptureRequest::Region),
                Action::Screenshot(Target::Window) => self.request_capture(CaptureRequest::Window),
                Action::Record(target) => self.request_capture(CaptureRequest::Recording(
                    capture_controls::TargetMode::of(target),
                )),
            },
            BusyRoute::RestoreControls => {
                self.show_recording_controls(ctx);
                if self.recording_screenshot_flow.is_some() {
                    self.recording_screenshot_controls_restored = true;
                }
                ctx.request_repaint_of(egui::ViewportId::from_hash_of("recording-controls"));
                request_hidden_root_paint(ctx);
            }
            BusyRoute::InProgress => self.capture_failed(CAPTURE_IN_PROGRESS.into()),
            BusyRoute::Ignore => {}
            BusyRoute::SwitchMenu { record, target } => {
                let shortcut = match (record, target) {
                    (false, Target::Region) => CaptureShortcut::Region,
                    (false, Target::Window) => CaptureShortcut::Window,
                    (false, Target::Display) => CaptureShortcut::Display,
                    (true, Target::Region) => CaptureShortcut::RecordRegion,
                    (true, Target::Window) => CaptureShortcut::RecordWindow,
                    (true, Target::Display) => CaptureShortcut::RecordDisplay,
                };
                self.controls
                    .lock()
                    .unwrap()
                    .apply_target_shortcut(shortcut);
                self.menu_screenshot_target = (!record).then_some(target);
                if let Some(target) = self.countdown_target {
                    ctx.request_repaint_of(capture_controls_viewport(target.monitor));
                }
            }
            BusyRoute::RecaptureSelector(target) => {
                self.requested_recapture = Some(Recapture::Selector(selector_kind(target)));
            }
            BusyRoute::RecaptureDisplay => self.requested_recapture = Some(Recapture::Display),
            BusyRoute::RecaptureMenu { record, target } => {
                self.requested_recapture = Some(Recapture::Menu {
                    record,
                    target: capture_controls::TargetMode::of(target),
                });
            }
        }
        if self.requested_recapture.is_some() {
            request_hidden_root_paint(ctx);
            ctx.request_repaint();
        }
    }

    /// Freezes the display under the pointer with the open capture UI still
    /// on it, then puts the requested UI in its place (or, for a display
    /// screenshot beside a take, saves that frame). Shipping
    /// `include_capture_ui_in_snapshot` before `prepare_capture`.
    fn launch_recapture(
        &mut self,
        frame: &eframe::Frame,
        settings: Result<AppSettings, String>,
        kind: Recapture,
    ) {
        let settings = match settings {
            Ok(settings) => settings,
            Err(error) => {
                self.capture_failed(error);
                return;
            }
        };
        let child = self.recording_screenshot_flow.is_some();
        let (generation, showing) = if let Some(flow) = &self.recording_screenshot_flow {
            (
                flow.generation(),
                flow.is_current()
                    && self.recording_screenshot_phase == Some(RecordingScreenshotPhase::Selecting),
            )
        } else if let Some(flow) = &self.flow {
            (
                flow.generation(),
                flow.is_current()
                    && matches!(
                        self.capture_phase,
                        Some(
                            CapturePhase::RegionSelecting
                                | CapturePhase::WindowSelecting
                                | CapturePhase::ControlsSelecting
                        )
                    ),
            )
        } else {
            return;
        };
        if !showing || (kind == Recapture::Display && !child) {
            return;
        }
        let pointer = captures_capture::pointer_position()
            .and_then(|point| captures_capture::XcapBackend.display_id_at_point(point));
        let display_id = match resolve_capture_display(
            &mut self.displays,
            self.display_id.as_deref(),
            pointer.as_deref(),
            || {
                captures_capture::XcapBackend
                    .displays()
                    .map_err(|error| error.to_string())
            },
        ) {
            Ok(id) => id,
            Err(error) => {
                self.capture_failed(error);
                return;
            }
        };
        let Some(target) = capture_target(frame, &self.displays, Some(&display_id)) else {
            self.capture_failed("The selected display is no longer available for capture.".into());
            return;
        };
        let include_cursor = settings.show_cursor_in_screenshots;
        let job = match kind {
            Recapture::Display => {
                // The selector stays declared while this frame is taken; the
                // reply saves it and closes the selector.
                self.recording_display_shot = Some(RecordingDisplayShot {
                    display_id: display_id.clone(),
                    target: Some(target),
                    countdown_seconds: 0,
                    include_cursor,
                    keeps_controls: self.recording_screenshot_keeps_controls(),
                });
                self.recording_screenshot_phase = Some(RecordingScreenshotPhase::DisplayCapturing);
                self.status = "Capturing screenshot while recording…".into();
                self.pending += 1;
                let _ = self.tx.send(Job::CaptureDisplay {
                    root: self.root.clone(),
                    display_id,
                    generation,
                    include_cursor,
                });
                return;
            }
            Recapture::Selector(RecordingSelectorKind::Region) => Job::PrepareRegion {
                display_id: display_id.clone(),
                generation,
                freeze: true,
                include_cursor,
            },
            Recapture::Selector(RecordingSelectorKind::Window) | Recapture::Menu { .. } => {
                Job::PrepareWindow {
                    display_id: display_id.clone(),
                    generation,
                    freeze: true,
                    include_cursor,
                }
            }
        };
        self.pending += 1;
        let _ = self.tx.send(job);
        self.recapture = Some(PendingRecapture {
            kind,
            generation,
            child,
            from_phase: (!child).then_some(self.capture_phase).flatten(),
            display_id,
            target,
            settings,
        });
        self.status = "Capturing the open capture UI… Press Escape to cancel.".into();
    }

    /// Takes the pending recapture a prepare reply belongs to.
    fn take_recapture(&mut self, generation: u64) -> Option<PendingRecapture> {
        if self
            .recapture
            .as_ref()
            .is_some_and(|pending| pending.generation == generation)
        {
            self.recapture.take()
        } else {
            None
        }
    }

    /// Retires the UI a recapture kept up and puts the flow in the phase
    /// that accepts its prepared snapshot. Returns false when that UI is
    /// no longer the open one (it was confirmed, cancelled or replaced).
    fn begin_recaptured(&mut self, pending: PendingRecapture) -> bool {
        if pending.child {
            let showing =
                self.recording_screenshot_flow.as_ref().is_some_and(|flow| {
                    flow.generation() == pending.generation && flow.is_current()
                }) && self.recording_screenshot_phase == Some(RecordingScreenshotPhase::Selecting);
            let Recapture::Selector(kind) = pending.kind else {
                return false;
            };
            if !showing {
                return false;
            }
            self.recording_screenshot_session = None;
            self.recording_screenshot_window_session = None;
            self.recording_screenshot_texture = None;
            self.recording_selector_shot = Some(RecordingSelectorShot {
                kind,
                display_id: pending.display_id,
                target: pending.target,
                includes_capture_ui: true,
            });
            self.recording_screenshot_phase = Some(RecordingScreenshotPhase::Preparing);
            return true;
        }
        let showing = self
            .flow
            .as_ref()
            .is_some_and(|flow| flow.generation() == pending.generation && flow.is_current())
            && self.capture_phase == pending.from_phase;
        if !showing {
            return false;
        }
        self.display_id = Some(pending.display_id);
        self.countdown_target = Some(pending.target);
        self.last_capture_target = Some(pending.target);
        self.previews.capture_target = Some(pending.target);
        self.selector_scope_generation.store(0, Ordering::Release);
        self.region_session = None;
        self.region_texture = None;
        self.region_selector.lock().unwrap().reset();
        self.window_session = None;
        self.window_texture = None;
        self.window_selector.lock().unwrap().reset();
        self.controls.lock().unwrap().reset();
        // The new selection has the old UI in its snapshot: no countdown
        // (shipping `screenshot_countdown_seconds_for_capture_ui`).
        match pending.kind {
            Recapture::Selector(RecordingSelectorKind::Region) => {
                self.region_freeze = true;
                self.region_countdown_seconds = 0;
                self.capture_phase = Some(CapturePhase::RegionPreparing);
            }
            Recapture::Selector(RecordingSelectorKind::Window) => {
                self.window_freeze = true;
                self.window_countdown_seconds = 0;
                self.capture_phase = Some(CapturePhase::WindowPreparing);
            }
            Recapture::Menu { record, target } => {
                self.configure_capture_menu(&pending.settings, record, target);
                self.controls_freeze = true;
                self.controls_countdown_seconds = 0;
                self.recording_screenshot_settings = Some(pending.settings);
                self.capture_phase = Some(CapturePhase::ControlsPreparing);
            }
            Recapture::Display => return false,
        }
        true
    }

    pub fn launch_requested_capture(
        &mut self,
        ctx: &egui::Context,
        frame: &eframe::Frame,
        settings: Result<AppSettings, String>,
    ) {
        if let Some(kind) = self.requested_recapture.take() {
            self.launch_recapture(frame, settings, kind);
            return;
        }
        let Some(request) = self.requested_capture.take() else {
            return;
        };
        #[cfg(target_os = "linux")]
        if ctx.data(|data| data.get_temp::<bool>(egui::Id::unique("wayland-surface"))) == Some(true)
        {
            if let CaptureRequest::Recording(mode) = request {
                self.launch_portal_recording(ctx, frame, settings, mode);
            } else {
                self.launch_portal_screenshot(ctx, frame, request, settings);
            }
            return;
        }
        if is_recording_phase(self.capture_phase)
            && let Some(kind) = match request {
                CaptureRequest::Display => Some(None),
                CaptureRequest::Region => Some(Some(RecordingSelectorKind::Region)),
                CaptureRequest::Window => Some(Some(RecordingSelectorKind::Window)),
                _ => None,
            }
        {
            match (settings, kind) {
                (Ok(settings), None) => {
                    self.start_recording_display_screenshot(ctx, frame, &settings);
                }
                (Ok(settings), Some(kind)) => {
                    self.start_recording_selector_screenshot(ctx, frame, kind, settings);
                }
                (Err(error), _) => self.capture_failed(error),
            }
            return;
        }
        if !self.can_start_capture() {
            self.capture_failed("Capture is unavailable until the current action finishes.".into());
            return;
        }
        let settings = match settings {
            Ok(settings) => settings,
            Err(error) => {
                self.capture_failed(error);
                return;
            }
        };
        // Shipping shortcut, tray and New Capture flows start on the display
        // under the pointer, looked up fresh for every capture. Keep the
        // current display when the pointer is unknown (for example Wayland).
        let pointer = captures_capture::pointer_position()
            .and_then(|point| captures_capture::XcapBackend.display_id_at_point(point));
        match resolve_capture_display(
            &mut self.displays,
            self.display_id.as_deref(),
            pointer.as_deref(),
            || {
                captures_capture::XcapBackend
                    .displays()
                    .map_err(|error| error.to_string())
            },
        ) {
            Ok(id) => self.display_id = Some(id),
            Err(error) => {
                self.display_id = None;
                self.capture_failed(error);
                return;
            }
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
            self.capture_failed(match request {
                CaptureRequest::Display => {
                    "The selected display is no longer available for countdown.".into()
                }
                CaptureRequest::NewCapture
                | CaptureRequest::DisplayMenu
                | CaptureRequest::Recording(_) => {
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
                self.capture_failed(format!("Could not arm capture Escape: {error}"));
                return;
            }
        };
        if let Err(error) =
            self.previews
                .begin_capture(&settings, target, ctx.cumulative_frame_nr())
        {
            flow.cancel();
            self.capture_failed(error);
            return;
        }
        // winit on X11 reports a mapped window as not visible until its first
        // VisibilityNotify, which can lag (for example while Preferences is
        // mapped over History). Trust the host's intent when History is meant
        // to be shown, and winit otherwise.
        self.restore_root_visible = self.root_shown
            || frame
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
        self.new_capture_shortcut = settings.new_capture_shortcut.clone();
        self.recording_screenshot_settings = matches!(
            request,
            CaptureRequest::NewCapture | CaptureRequest::DisplayMenu | CaptureRequest::Recording(_)
        )
        .then(|| settings.clone());
        match request {
            CaptureRequest::NewCapture
            | CaptureRequest::DisplayMenu
            | CaptureRequest::Recording(_) => {
                let (record, target) = match request {
                    CaptureRequest::Recording(target) => (true, target),
                    // Shipping `open_capture_controls_with_target(Screenshot, Display)`.
                    CaptureRequest::DisplayMenu => (false, capture_controls::TargetMode::Display),
                    _ => (false, capture_controls::TargetMode::Region),
                };
                self.configure_capture_menu(&settings, record, target);
                self.capture_phase = Some(CapturePhase::ControlsPreparing);
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

    #[cfg(target_os = "linux")]
    fn launch_portal_recording(
        &mut self,
        ctx: &egui::Context,
        frame: &eframe::Frame,
        settings: Result<AppSettings, String>,
        mode: capture_controls::TargetMode,
    ) {
        if !self.can_start_capture() {
            self.capture_failed("Capture is unavailable until the current action finishes.".into());
            return;
        }
        let (settings, options) = match settings.and_then(|settings| {
            portal_recording_options(&settings.recording, mode).map(|options| (settings, options))
        }) {
            Ok(prepared) => prepared,
            Err(error) => {
                self.capture_failed(error);
                return;
            }
        };
        let flow = match CaptureFlow::begin_portal(0) {
            Ok(flow) => flow,
            Err(error) => {
                self.capture_failed(error);
                return;
            }
        };
        let generation = flow.generation();
        self.restore_root_visible = self.root_shown
            || frame.winit_window().and_then(|window| window.is_visible()) == Some(true);
        self.flow = Some(flow);
        self.countdown_target = None;
        self.auto_copy_on_capture = false;
        self.open_editor_after_recording = settings
            .recording
            .open_editor_after_recording
            .then(|| PathBuf::from(&settings.output_directory));
        self.new_capture_shortcut = settings.new_capture_shortcut;
        // Linux cannot exclude its controls from the granted video. The HUD
        // states this explicitly and uses the existing manual Hide action.
        self.include_recording_controls = true;
        self.workspace_hidden = true;
        self.recording_portal_windows = ctx.input(|input| {
            input
                .raw
                .viewports
                .keys()
                .copied()
                .filter(|id| *id != egui::ViewportId::ROOT)
                .collect()
        });
        self.recording_portal_windows
            .push(main_countdown_viewport());
        for id in self
            .recording_portal_windows
            .iter()
            .copied()
            .filter(|id| *id != main_countdown_viewport())
            .chain([egui::ViewportId::ROOT])
        {
            ctx.send_viewport_cmd_to(id, egui::ViewportCommand::Visible(false));
            ctx.request_repaint_of(id);
        }
        self.capture_phase = Some(CapturePhase::RecordingPreparing { target: None });
        self.recording_worker.send(recording::Command::Prepare {
            generation,
            recovery_root: recording_recovery_root(&self.root),
            options,
            display: None,
        });
        self.status = "Preparing portal recording…".into();
        request_hidden_root_paint(ctx);
    }

    #[cfg(target_os = "linux")]
    fn launch_portal_screenshot(
        &mut self,
        ctx: &egui::Context,
        frame: &eframe::Frame,
        request: CaptureRequest,
        settings: Result<AppSettings, String>,
    ) {
        let recording = is_recording_phase(self.capture_phase);
        if !(if recording {
            self.recording_screenshot_available()
        } else {
            self.can_start_capture()
        }) {
            self.capture_failed("Capture is unavailable until the current action finishes.".into());
            return;
        }
        if !matches!(
            request,
            CaptureRequest::NewCapture | CaptureRequest::Display | CaptureRequest::DisplayMenu
        ) {
            self.capture_failed("Wayland supports desktop-portal screenshots and display recording here; native region/window selection is not available yet.".into());
            self.history_requested = true;
            return;
        }
        let settings = match settings {
            Ok(settings) => settings,
            Err(error) => {
                self.capture_failed(error);
                return;
            }
        };
        let flow = match if recording {
            self.flow
                .as_ref()
                .ok_or_else(|| "The recording is no longer active".to_owned())
                .and_then(CaptureFlow::begin_recording_portal_screenshot)
        } else {
            captures_app::capture_flow::PortalCapture::begin()
        } {
            Ok(flow) => flow,
            Err(error) => {
                self.capture_failed(error);
                return;
            }
        };
        // The portal supplies no named-monitor coordinates. Return to History
        // instead of placing a preview against guessed desktop bounds.
        self.auto_copy_on_capture = settings.auto_copy_to_clipboard;
        let mut hidden = ctx.input(|input| {
            input
                .raw
                .viewports
                .iter()
                .filter(|(id, _)| **id != egui::ViewportId::ROOT)
                .map(|(id, _)| *id)
                .collect::<Vec<_>>()
        });
        // Immediate Preferences must not paint while the compositor is still
        // acknowledging the other windows' unmaps.
        self.workspace_hidden = true;
        for id in hidden.iter().copied().chain([egui::ViewportId::ROOT]) {
            ctx.send_viewport_cmd_to(id, egui::ViewportCommand::Visible(false));
            ctx.request_repaint_of(id);
        }
        let generation = flow.generation();
        let countdown = (settings.screenshot_countdown_seconds > 0).then(|| {
            // No monitor geometry is supplied by the screenshot portal; use
            // the same compact, compositor-placed countdown as portal recording.
            hidden.push(recording_screenshot_countdown_viewport(generation));
            captures_app::capture_flow::Countdown::new(
                Instant::now(),
                settings.screenshot_countdown_seconds,
            )
        });
        let wake = ctx.clone();
        thread::spawn(move || {
            // Keep the logic-only root alive across compositor acknowledgements.
            // This poll exists only while this capture owns the process gate.
            while captures_app::capture_flow::is_current(generation) {
                wake.request_repaint_of(egui::ViewportId::ROOT);
                thread::sleep(Duration::from_millis(100));
            }
            wake.request_repaint_of(egui::ViewportId::ROOT);
        });
        self.portal_screenshot = Some(PortalScreenshot {
            flow,
            hidden,
            restore_root: self.root_shown
                || frame.winit_window().and_then(|window| window.is_visible()) == Some(true),
            recording,
            started: Instant::now(),
            submitted: false,
            countdown,
            countdown_hidden_frame: None,
        });
        self.capture_in_flight = true;
        self.status = "Preparing desktop-portal screenshot…".into();
        if recording || countdown.is_some() {
            // Replace the HUD callback before any child can repaint with its
            // old Visible(true), and declare the countdown on the hidden root.
            request_hidden_root_ui(ctx);
        }
        ctx.request_repaint_after(Duration::from_millis(100));
    }

    #[cfg(target_os = "linux")]
    fn advance_portal_screenshot(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        let Some(portal) = self.portal_screenshot.as_mut() else {
            return;
        };
        crate::diagnostics::event("portal-capture", || {
            serde_json::json!({
                "generation":portal.flow.generation(), "submitted":portal.submitted,
                "current":portal.flow.is_current(),
                "rootVisible":frame.winit_window().and_then(|window| window.is_visible()),
                "childrenVisible":ctx.input(|input| portal.hidden.iter().map(|id|
                    input.raw.viewports.get(id).and_then(|info| info.visible())).collect::<Vec<_>>())
            })
        });
        if portal.submitted {
            return;
        }
        if !portal.flow.is_current() {
            if let Some(clock) = portal.countdown {
                let remaining = clock.remaining(Instant::now());
                if remaining > 0 {
                    self.countdown_exit = Some(CountdownExit::new(
                        recording_screenshot_countdown_viewport(portal.flow.generation()),
                        "Captures Screenshot Countdown",
                        None,
                        crate::countdown::Kind::Screenshot,
                        remaining,
                    ));
                    request_hidden_root_ui(ctx);
                }
            }
            self.finish_portal_screenshot(ctx, false);
            self.status = "Screenshot cancelled (Escape or desktop session unavailable).".into();
            return;
        }
        let countdown_hidden = portal.countdown_hidden(Instant::now(), ctx.cumulative_frame_nr());
        if !countdown_hidden {
            if portal.countdown_hidden_frame.is_some() {
                ctx.send_viewport_cmd_to(
                    recording_screenshot_countdown_viewport(portal.flow.generation()),
                    egui::ViewportCommand::Visible(false),
                );
                // Replace the old deferred callback in a completed UI pass
                // before trusting the countdown window's unmap acknowledgement.
                request_hidden_root_ui(ctx);
            }
            if portal.countdown_hidden_frame.is_none()
                || portal.started.elapsed() <= Duration::from_secs(2)
            {
                return;
            }
        }
        for id in &portal.hidden {
            ctx.request_repaint_of(*id);
        }
        let hidden = countdown_hidden
            && frame.winit_window().and_then(|window| window.is_visible()) == Some(false)
            && (portal.countdown.is_none()
                || ctx.input(|input| {
                    input
                        .raw
                        .viewports
                        .get(&recording_screenshot_countdown_viewport(
                            portal.flow.generation(),
                        ))
                        .is_some_and(|info| info.visible() == Some(false))
                }))
            && ctx.input(|input| {
                portal.hidden.iter().all(|id| {
                    input
                        .raw
                        .viewports
                        .get(id)
                        .is_none_or(|info| info.visible() == Some(false))
                })
            });
        if hidden {
            portal.submitted = true;
            self.pending += 1;
            let _ = self.tx.send(Job::CapturePortal {
                root: self.root.clone(),
                generation: portal.flow.generation(),
            });
            self.status = "Waiting for the desktop screenshot portal…".into();
        } else if portal.started.elapsed() > Duration::from_secs(2) {
            let recording = portal.recording;
            self.finish_portal_screenshot(ctx, false);
            let error = "Could not unmap all Captures windows. No screenshot was taken.".into();
            if recording {
                self.hud_action_failed(ctx, error);
            } else {
                self.capture_failed(error);
                self.history_requested = true;
            }
        }
    }

    #[cfg(target_os = "linux")]
    fn finish_portal_screenshot(&mut self, ctx: &egui::Context, captured: bool) {
        let Some(portal) = self.portal_screenshot.take() else {
            return;
        };
        if portal.recording {
            // Child completion/cancellation must not retire the accepted take,
            // reset its clock, or remap the workspace into its video.
            self.capture_in_flight = false;
            if !captured {
                self.auto_copy_on_capture = false;
            }
            // With every child unmapped, a logic-only root cannot redeclare
            // the HUD callback that restores it (and respects manual Hide).
            request_hidden_root_ui(ctx);
            if !self.recording_controls_hidden() {
                // Updating a deferred callback does not run an unmapped child;
                // remap only the HUD, never the hidden History/Preferences.
                let hud = egui::ViewportId::from_hash_of("recording-controls");
                ctx.send_viewport_cmd_to(hud, egui::ViewportCommand::Visible(true));
                ctx.request_repaint_of(hud);
            }
            return;
        }
        let countdown = recording_screenshot_countdown_viewport(portal.flow.generation());
        for id in portal.hidden.into_iter().filter(|id| *id != countdown) {
            ctx.send_viewport_cmd_to(id, egui::ViewportCommand::Visible(true));
            ctx.request_repaint_of(id);
        }
        self.restore_root_visible = portal.restore_root;
        self.finish_capture(ctx, captured);
    }

    fn open_share(&self, ctx: &egui::Context, artifact_id: &str) {
        let Some(artifact) = self.artifacts.iter().find(|a| a.entry.id == artifact_id) else {
            return;
        };
        let path = if artifact.entry.kind.is_recording() {
            artifact.entry.recording_media_path(&self.root)
        } else {
            Some(artifact.image_path.clone())
        };
        let Some(path) = path else {
            return;
        };
        let content_type = match path.extension().and_then(|e| e.to_str()) {
            Some("gif") => "image/gif",
            Some("mp4") => "video/mp4",
            Some("webm") => "video/webm",
            _ => "image/png",
        };
        let name = format!(
            "Capture-{}.{}",
            artifact.entry.id,
            path.extension().and_then(|e| e.to_str()).unwrap_or("png")
        );
        let texture = self
            .previews
            .cards
            .get(artifact_id)
            .and_then(|c| c.texture.clone())
            .or_else(|| {
                self.history_thumbnails
                    .get(artifact_id)
                    .and_then(|h| match &h.thumbnail {
                        crate::history::Thumbnail::Ready(t) => Some(t.clone()),
                        _ => None,
                    })
            });
        self.sharing.open(
            ctx,
            self.root.clone(),
            captures_account::native::Selection {
                artifact_id: artifact_id.into(),
                path,
                name,
                content_type: content_type.into(),
            },
            texture,
        );
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
        self.sharing.shutdown();
        self.open_media.clear();
        self.pending_editor_opens.clear();
        self.editors.clear();
        self.recording_editors.clear();
        self.recording_notice = None;
        self.recording_notice_target = None;
        // Cancel preparation/countdown before draining work. CaptureFlow::cancel
        // leaves a capture that already crossed its persistence commit point alone.
        self.selector_scope_generation.store(0, Ordering::Release);
        #[cfg(target_os = "linux")]
        if let Some(portal) = &self.portal_screenshot {
            portal.flow.cancel();
        }
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
        self.recording_screenshot_window_session = None;
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
        self.workspace_hidden = true;
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
        #[cfg(target_os = "linux")]
        self.advance_portal_screenshot(ctx, frame);
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
        self.editors.retain(|_, editor| !editor.retired());
        // Failed inline composition cancels close and restores that editor.
        self.pending_editor_opens
            .retain(|id, _| self.editors.get(id).is_none_or(|editor| editor.closing()));
        let ready = self
            .pending_editor_opens
            .keys()
            .filter(|id| !self.editors.contains_key(*id))
            .cloned()
            .collect::<Vec<_>>();
        for id in ready {
            let (directory, mode) = self.pending_editor_opens.remove(&id).unwrap();
            self.open_screenshot_editor(ctx, id, directory, mode);
        }
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
                    kind: SelectorKind::Region | SelectorKind::Window,
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
                    let seconds = self.recording_selector_countdown_seconds();
                    let Some(flow) = &mut self.recording_screenshot_flow else {
                        continue;
                    };
                    match flow.start_countdown(seconds) {
                        Ok(()) => {
                            self.recording_screenshot_phase =
                                Some(RecordingScreenshotPhase::Countdown {
                                    choice: RecordingShotChoice::Region(rect),
                                    after_countdown: seconds > 0,
                                });
                            self.status =
                                "Screenshot region confirmed. Press Escape to cancel.".into();
                            request_hidden_root_paint(ctx);
                        }
                        Err(error) => {
                            self.fail_recording_screenshot(ctx, error);
                        }
                    }
                }
                SelectorMessage::ConfirmWindow { generation, target }
                    if self.recording_screenshot_flow.as_ref().is_some_and(|flow| {
                        flow.generation() == generation && flow.is_current()
                    }) && self.recording_screenshot_phase
                        == Some(RecordingScreenshotPhase::Selecting) =>
                {
                    let seconds = self.recording_selector_countdown_seconds();
                    let Some(flow) = &mut self.recording_screenshot_flow else {
                        continue;
                    };
                    match flow.start_countdown(seconds) {
                        Ok(()) => {
                            self.recording_screenshot_phase =
                                Some(RecordingScreenshotPhase::Countdown {
                                    choice: RecordingShotChoice::Window(target),
                                    after_countdown: seconds > 0,
                                });
                            self.status =
                                "Screenshot window confirmed. Press Escape to cancel.".into();
                            request_hidden_root_paint(ctx);
                        }
                        Err(error) => {
                            self.fail_recording_screenshot(ctx, error);
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
                            self.fail_capture(ctx, error);
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
                            self.fail_capture(ctx, error);
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
                    self.controls_error = None;
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
                            self.fail_capture(ctx, error);
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
                    // Shipping clears the menu's error as a start begins.
                    self.controls_error = None;
                    if !self.recording_toolchain_ready {
                        // The menu stays open and shows this inline, like shipping.
                        let error = self.recording_toolchain_error.clone().unwrap_or_else(|| {
                            "FFmpeg and ffprobe verification is still in progress.".into()
                        });
                        self.keep_controls_open_with_error(ctx, error);
                        continue;
                    }
                    // Shipping `start_recording` restores the selection and
                    // returns these to the menu (`validate_target`, the draft).
                    let Some(display) = self
                        .displays
                        .iter()
                        .find(|display| Some(&display.id) == self.display_id.as_ref())
                        .cloned()
                    else {
                        self.keep_controls_open_with_error(
                            ctx,
                            "The recording display is no longer available.".into(),
                        );
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
                            self.keep_controls_open_with_error(ctx, error);
                            continue;
                        }
                    };
                    // The menu stays up showing "Starting…" while the take prepares,
                    // as shipping's selector does until `start_recording` hides it.
                    self.selector_scope_generation.store(0, Ordering::Release);
                    self.capture_phase = Some(CapturePhase::RecordingPreparing {
                        target: Some(target),
                    });
                    self.recording_worker.send(recording::Command::Prepare {
                        generation,
                        recovery_root: recording_recovery_root(&self.root),
                        options,
                        display: Some(display),
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
                        && self.recording_screenshot_flow.is_none()
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
                    if std::env::var_os("CAPTURES_NATIVE_LAYOUT_PROBE").is_some() {
                        println!(
                            "{}",
                            serde_json::json!({"event":"recording-stop-submit",
                            "detail":{"generation":generation}})
                        );
                    }
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
                    // Shipping `switchDisplay` clears the error first and
                    // shows any failure inline on the current display.
                    self.controls_error = None;
                    if !self.displays.iter().any(|display| display.id == display_id) {
                        self.keep_controls_open_with_error(
                            ctx,
                            "The selected display is no longer available.".into(),
                        );
                        continue;
                    }
                    let Some(target) = capture_target(frame, &self.displays, Some(&display_id))
                    else {
                        self.keep_controls_open_with_error(
                            ctx,
                            "The selected display is unavailable for capture controls.".into(),
                        );
                        continue;
                    };
                    self.display_id = Some(display_id);
                    // The old menu stays up showing "Switching…" until the new
                    // display's session is ready (shipping `switchDisplay`).
                    self.controls_switch_from = self
                        .countdown_target
                        .zip(self.window_session.take())
                        .map(|(from, session)| (from, session, self.window_texture.take()));
                    self.countdown_target = Some(target);
                    self.previews.capture_target = Some(target);
                    self.selector_scope_generation.store(0, Ordering::Release);
                    self.controls_switching_display = true;
                    self.window_session = None;
                    self.window_texture = None;
                    self.capture_phase = Some(CapturePhase::ControlsPreparing);
                    self.status = "Switching capture controls to the selected display…".into();
                    request_hidden_root_paint(ctx);
                    self.begin_root_hide(ctx);
                }
                SelectorMessage::ListMicrophones { generation }
                    if self.flow.as_ref().map(CaptureFlow::generation) == Some(generation) =>
                {
                    self.recording_worker
                        .send(recording::Command::ListMicrophones { generation });
                }
                SelectorMessage::ConfirmRegion { .. }
                | SelectorMessage::ConfirmWindow { .. }
                | SelectorMessage::ConfirmControls { .. }
                | SelectorMessage::ListMicrophones { .. }
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
                    // Shipping dissolves the card first; Trash follows the dust.
                    if self
                        .previews
                        .begin_trash(&artifact_id, crate::motion::reduced(ctx))
                    {
                        self.previews
                            .trashing
                            .get_mut(&artifact_id)
                            .expect("trashing card")
                            .sent = true;
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
                    request_hidden_root_paint(ctx);
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
                PreviewMessage::Share {
                    artifact_id,
                    generation,
                } if self.previews.accepts(&artifact_id, generation)
                    && !self.workspace_hidden
                    && self.previews.cards[&artifact_id].busy.is_none()
                    && self.pending == 0
                    && !self.permission_recovery_visible =>
                {
                    self.open_share(ctx, &artifact_id);
                }
                PreviewMessage::Dismiss {
                    artifact_id,
                    generation,
                    delete,
                } => {
                    let kind = if delete {
                        captures_app::preview_motion::ExitKind::Dust
                    } else {
                        captures_app::preview_motion::ExitKind::Dismiss
                    };
                    if self.previews.accepts(&artifact_id, generation)
                        && self
                            .previews
                            .exit_card(&artifact_id, kind, crate::motion::reduced(ctx))
                    {
                        request_hidden_root_paint(ctx);
                        ctx.request_repaint();
                    }
                }
                PreviewMessage::ToggleCollapsed => {
                    self.previews.toggle_collapsed(crate::motion::reduced(ctx));
                    let collapse = self.previews.stack.is_collapsed();
                    if !collapse {
                        // Expanding leaves the pointer over a card that was
                        // never hovered; hold its chrome until it moves.
                        self.previews.hover_lock_generation =
                            self.previews.hover_lock_generation.wrapping_add(1);
                    }
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
                    if self
                        .previews
                        .clear_with_exit(&artifact_ids, crate::motion::reduced(ctx))
                        > 0
                    {
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
                | PreviewMessage::Share { .. }
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
                            if snapshot.state == RecordingState::Failed
                                && matches!(
                                    self.capture_phase,
                                    Some(CapturePhase::Recording | CapturePhase::RecordingPaused)
                                )
                            {
                                self.error = snapshot.error;
                                self.status =
                                    "Recording source ended; partial media retained for recovery."
                                        .into();
                                self.finish_capture(ctx, false);
                                continue;
                            }
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
                                    request_hidden_root_ui(ctx);
                                }
                                Err(error) => {
                                    self.capture_failure = Some(CaptureFailure {
                                        error,
                                        recording: true,
                                    });
                                    self.recording_worker
                                        .send(recording::Command::Discard { generation });
                                    self.capture_phase = Some(CapturePhase::RecordingDiscarding);
                                }
                            }
                        }
                        Err(error) => {
                            // Shipping `initialize_recording_session` failures
                            // keep the menu open with the inline error.
                            if self.capture_phase
                                == Some(CapturePhase::RecordingPreparing { target: None })
                            {
                                self.fail_recording(ctx, error);
                            } else {
                                self.keep_controls_open_with_error(ctx, error);
                            }
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
                            request_hidden_root_ui(ctx);
                        }
                        Err(failure) => {
                            let current = self.flow.as_ref().is_some_and(CaptureFlow::is_current);
                            let snapshot = failure.snapshot.map(|snapshot| *snapshot);
                            match snapshot {
                                Some(snapshot)
                                    if snapshot.state == RecordingState::Discarded
                                        && !self.recording_has_started =>
                                {
                                    self.status = "Recording cancelled.".into();
                                    self.finish_capture(ctx, false);
                                    continue;
                                }
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
                            self.fail_recording(ctx, error);
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
                            self.fail_recording(ctx, error);
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
                {
                    let (title, kind) = main_countdown_presentation(self.capture_phase);
                    self.countdown_exit = Some(CountdownExit::new(
                        main_countdown_viewport(),
                        title,
                        self.countdown_target,
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
                    && let Some(
                        RecordingScreenshotPhase::Countdown { .. }
                        | RecordingScreenshotPhase::DisplayCountdown,
                    ) = self.recording_screenshot_phase
                    && let Some(target) = self.recording_screenshot_target()
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
                    Some(RecordingScreenshotPhase::DisplayHidingControls) => {
                        let Some(shot) = self.recording_display_shot.as_ref() else {
                            self.fail_recording_screenshot(
                                ctx,
                                "Display preparation was lost before capture.".into(),
                            );
                            return;
                        };
                        let settle = shot.controls_settle();
                        let seconds = shot.countdown_seconds;
                        if self
                            .recording_screenshot_hide_started
                            .is_some_and(|started| started.elapsed() >= settle)
                        {
                            let started = self
                                .recording_screenshot_flow
                                .as_mut()
                                .map(|flow| flow.start_countdown(seconds));
                            match started {
                                Some(Ok(())) if seconds > 0 => {
                                    self.recording_screenshot_phase =
                                        Some(RecordingScreenshotPhase::DisplayCountdown);
                                    self.status =
                                        "Screenshot countdown. Press Escape to cancel.".into();
                                }
                                Some(Ok(())) => {
                                    // No countdown window to retire; the
                                    // controls already had time to leave.
                                    self.recording_screenshot_phase =
                                        Some(RecordingScreenshotPhase::DisplaySettling {
                                            until: Instant::now(),
                                        });
                                }
                                Some(Err(error)) => {
                                    self.fail_recording_screenshot(ctx, error);
                                    return;
                                }
                                None => return,
                            }
                            request_hidden_root_paint(ctx);
                            ctx.request_repaint();
                        } else {
                            ctx.request_repaint_after(Duration::from_millis(16));
                        }
                    }
                    Some(RecordingScreenshotPhase::DisplayCountdown)
                        if flow.countdown().remaining(Instant::now()) == 0 =>
                    {
                        self.recording_screenshot_phase =
                            Some(RecordingScreenshotPhase::DisplayRetiring {
                                omitted_frame: ctx.cumulative_frame_nr(),
                            });
                        request_hidden_root_paint(ctx);
                        ctx.request_repaint();
                    }
                    Some(RecordingScreenshotPhase::DisplayCountdown) => {
                        ctx.request_repaint_after(Duration::from_millis(100));
                    }
                    Some(
                        RecordingScreenshotPhase::DisplayRetiring { .. }
                        | RecordingScreenshotPhase::DisplaySettling { .. },
                    ) => {
                        let ready = self
                            .recording_screenshot_phase
                            .as_mut()
                            .is_some_and(|phase| {
                                phase.display_capture_after_hide(
                                    ctx.cumulative_frame_nr(),
                                    Instant::now(),
                                )
                            });
                        if !ready {
                            request_hidden_root_paint(ctx);
                            ctx.request_repaint_after(Duration::from_millis(16));
                        } else if let Some(shot) = self.recording_display_shot.as_ref() {
                            self.status = "Capturing screenshot while recording…".into();
                            self.pending += 1;
                            let _ = self.tx.send(Job::CaptureDisplay {
                                root: self.root.clone(),
                                display_id: shot.display_id.clone(),
                                generation,
                                include_cursor: shot.include_cursor,
                            });
                        }
                    }
                    Some(RecordingScreenshotPhase::WaitingForHud)
                        if self
                            .recording_screenshot_hide_started
                            .is_some_and(|started| {
                                started.elapsed() >= Duration::from_millis(300)
                            }) =>
                    {
                        let Some(shot) = self.recording_selector_shot.as_ref() else {
                            self.fail_recording_screenshot(
                                ctx,
                                "The screenshot display is no longer available.".into(),
                            );
                            return;
                        };
                        let display_id = shot.display_id.clone();
                        let kind = shot.kind;
                        let settings = self
                            .recording_screenshot_settings
                            .as_ref()
                            .expect("recording screenshot retains settings");
                        let (freeze, include_cursor) =
                            (settings.freeze_screen, settings.show_cursor_in_screenshots);
                        self.recording_screenshot_phase = Some(RecordingScreenshotPhase::Preparing);
                        self.pending += 1;
                        let _ = self.tx.send(match kind {
                            RecordingSelectorKind::Region => Job::PrepareRegion {
                                display_id,
                                generation,
                                freeze,
                                include_cursor,
                            },
                            RecordingSelectorKind::Window => Job::PrepareWindow {
                                display_id,
                                generation,
                                freeze,
                                include_cursor,
                            },
                        });
                    }
                    Some(RecordingScreenshotPhase::WaitingForHud) => {
                        ctx.request_repaint_after(Duration::from_millis(16));
                    }
                    Some(RecordingScreenshotPhase::Countdown {
                        choice,
                        after_countdown,
                    }) if flow.countdown().remaining(Instant::now()) == 0 => {
                        self.recording_screenshot_texture = None;
                        self.recording_screenshot_phase =
                            Some(RecordingScreenshotPhase::RetiringCaptureUi {
                                choice,
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
                        if let Some((choice, after_countdown)) = ready {
                            let job = match choice {
                                RecordingShotChoice::Region(rect) => self
                                    .recording_screenshot_session
                                    .take()
                                    .map(|session| Job::CaptureRegion {
                                        root: self.root.clone(),
                                        generation,
                                        session,
                                        rect,
                                        after_countdown,
                                    })
                                    .ok_or("Region preparation was lost before capture."),
                                RecordingShotChoice::Window(target) => self
                                    .recording_screenshot_window_session
                                    .take()
                                    .ok_or("Window preparation was lost before capture.")
                                    .and_then(|session| {
                                        let target = window_capture_target(&session, target)
                                            .ok_or("The selected window changed before capture.")?;
                                        Ok(Job::CaptureWindow {
                                            root: self.root.clone(),
                                            generation,
                                            session,
                                            target,
                                            after_countdown,
                                        })
                                    }),
                            };
                            match job {
                                Ok(job) => {
                                    self.pending += 1;
                                    let _ = self.tx.send(job);
                                }
                                Err(error) => {
                                    self.fail_recording_screenshot(ctx, error.into());
                                    return;
                                }
                            }
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
            for id in &self.recording_portal_windows {
                ctx.send_viewport_cmd_to(*id, egui::ViewportCommand::Visible(false));
                ctx.request_repaint_of(*id);
            }
            let portal_windows_hidden = ctx.input(|input| {
                self.recording_portal_windows.iter().all(|id| {
                    input
                        .raw
                        .viewports
                        .get(id)
                        .is_none_or(|info| info.visible() == Some(false))
                })
            });
            let visible = frame.winit_window().and_then(|window| window.is_visible());
            if visible == Some(false) && !self.companion_visible && portal_windows_hidden {
                self.hidden_since.get_or_insert_with(Instant::now);
            } else {
                self.hidden_since = None;
            }
            if self
                .hidden_since
                .is_some_and(|since| since.elapsed() >= Duration::from_millis(150))
            {
                self.capture_waiting_for_hide = false;
                if self.capture_phase == Some(CapturePhase::RecordingCountdown)
                    && self
                        .recording_snapshot
                        .as_ref()
                        .is_some_and(|snapshot| snapshot.options.target.is_portal())
                {
                    let Some(flow) = &self.flow else { return };
                    if std::env::var_os("CAPTURES_NATIVE_LAYOUT_PROBE").is_some() {
                        println!(
                            "{}",
                            serde_json::json!({"event":"portal-recording-submit",
                            "detail":{"generation":flow.generation(), "root_visible":visible,
                                "children_hidden":portal_windows_hidden}})
                        );
                    }
                    self.capture_phase = Some(CapturePhase::RecordingStarting);
                    self.recording_worker.send(recording::Command::Start {
                        generation: flow.generation(),
                        exclude_captures_app: false,
                    });
                    self.status = "Waiting for desktop-portal recording consent…".into();
                    request_hidden_root_paint(ctx);
                    return;
                }
                let Some(display_id) = self.display_id.clone() else {
                    self.flow = None;
                    self.workspace_hidden = false;
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
                            self.fail_capture(
                                ctx,
                                "Region preparation was lost before capture.".into(),
                            );
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
                            self.fail_capture(
                                ctx,
                                "Window preparation was lost before capture.".into(),
                            );
                            return;
                        };
                        let Some(target) = window_capture_target(&session, target) else {
                            self.fail_capture(
                                ctx,
                                "The selected window changed before capture.".into(),
                            );
                            return;
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
                            self.fail_capture(
                                ctx,
                                "Capture-control preparation was lost before capture.".into(),
                            );
                            return;
                        };
                        let target = match target {
                            capture_controls::Target::Region(rect) => {
                                WindowCaptureTarget::Region { rect }
                            }
                            capture_controls::Target::Display => WindowCaptureTarget::Display,
                            capture_controls::Target::Window(index) => {
                                let Some(window) = session.windows().get(index) else {
                                    self.fail_capture(
                                        ctx,
                                        "The selected window changed before capture.".into(),
                                    );
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
                self.fail_capture(
                    ctx,
                    "Could not hide the capture window. No screenshot was taken.".into(),
                );
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
                Reply::DisplaysListed(result) => {
                    self.pending = self.pending.saturating_sub(1);
                    match result {
                        Ok(response) => self.apply(*response, true),
                        // Shipping never lists displays ahead of a capture;
                        // the next capture lists them again and reports it.
                        Err(error) => eprintln!("Could not list displays: {error}"),
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
                    // Shipping "Clipboard unavailable": the card's last copy failed.
                    let copied_id = preview
                        .as_ref()
                        .map(|preview| preview.artifact_id.clone())
                        .or_else(|| owner.clone());
                    if let Some(card) = copied_id.and_then(|id| self.previews.cards.get_mut(&id)) {
                        card.copy_failed = result.is_err();
                    }
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
                    let display_capture =
                        self.capture_phase == Some(CapturePhase::DisplayCapturing);
                    if display_capture {
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
                                if self.previews.restore_trashed(
                                    &preview.artifact_id,
                                    preview.generation,
                                    format!("Trash failed: {error}"),
                                ) {
                                    request_hidden_root_paint(ctx);
                                } else if let Some(card) = self
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
                            } else if display_capture {
                                self.capture_failed(error);
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
                                // The card already dissolved before the request.
                                if *id == guard.artifact_id
                                    && self.previews.finish_trash(id, guard.generation)
                                {
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
                    if let Some(pending) = self.take_recapture(generation) {
                        if !self.begin_recaptured(pending) {
                            continue;
                        }
                        request_recaptured_viewports(ctx, generation, self.displays.len());
                    }
                    let recording_screenshot =
                        self.recording_screenshot_flow.as_ref().is_some_and(|flow| {
                            flow.generation() == generation && flow.is_current()
                        }) && self.recording_screenshot_phase
                            == Some(RecordingScreenshotPhase::Preparing);
                    if recording_screenshot {
                        match result {
                            Ok(session) => {
                                if !self.recording_selector_display_matches(session.display()) {
                                    self.fail_recording_screenshot(
                                        ctx,
                                        "The display changed while preparing the screenshot."
                                            .into(),
                                    );
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
                                self.fail_recording_screenshot(ctx, error);
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
                                self.fail_capture(
                                    ctx,
                                    "The selected display changed while preparing the region."
                                        .into(),
                                );
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
                            self.fail_capture(ctx, error);
                        }
                    }
                }
                Reply::DisplayCaptured { generation, result } => {
                    self.pending = self.pending.saturating_sub(1);
                    let current = self
                        .recording_screenshot_flow
                        .as_ref()
                        .is_some_and(|flow| flow.generation() == generation)
                        && self.recording_screenshot_phase
                            == Some(RecordingScreenshotPhase::DisplayCapturing);
                    if !current {
                        continue;
                    }
                    let captured = result.is_ok();
                    self.finish_recording_screenshot(ctx, captured);
                    match result {
                        Ok(artifact) => {
                            self.accept_artifact(
                                *artifact,
                                "Screenshot captured while recording",
                                true,
                            );
                        }
                        Err(error) => self.capture_failed(error),
                    }
                }
                #[cfg(target_os = "linux")]
                Reply::PortalCaptured { generation, result } => {
                    self.pending = self.pending.saturating_sub(1);
                    if self.portal_screenshot.as_ref().is_none_or(|portal| {
                        portal.flow.generation() != generation || !portal.submitted
                    }) {
                        continue;
                    }
                    let recording = self.portal_screenshot.as_ref().unwrap().recording;
                    let captured = matches!(&result, Ok(Some(_)));
                    if !recording {
                        self.history_requested = !matches!(&result, Ok(None));
                    }
                    self.finish_portal_screenshot(ctx, captured);
                    match result {
                        Ok(Some(artifact)) => {
                            self.accept_artifact(
                                *artifact,
                                "Screenshot captured through the desktop portal",
                                false,
                            );
                        }
                        Ok(None) => self.status = "Screenshot cancelled.".into(),
                        Err(error) if recording => self.hud_action_failed(ctx, error),
                        Err(error) => self.capture_failed(error),
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
                            Ok(artifact) => self.accept_artifact(
                                *artifact,
                                "Screenshot captured while recording",
                                true,
                            ),
                            Err(error) => self.capture_failed(error),
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
                        Ok(artifact) => {
                            self.accept_artifact(*artifact, "Region captured as PNG", true)
                        }
                        Err(error) => self.capture_failed(error),
                    }
                }
                Reply::WindowPrepared { generation, result } => {
                    self.pending = self.pending.saturating_sub(1);
                    if let Some(pending) = self.take_recapture(generation) {
                        if !self.begin_recaptured(pending) {
                            continue;
                        }
                        request_recaptured_viewports(ctx, generation, self.displays.len());
                    }
                    let recording_screenshot =
                        self.recording_screenshot_flow.as_ref().is_some_and(|flow| {
                            flow.generation() == generation && flow.is_current()
                        }) && self.recording_screenshot_phase
                            == Some(RecordingScreenshotPhase::Preparing);
                    if recording_screenshot {
                        match result {
                            Ok(session) => {
                                if !self.recording_selector_display_matches(session.display()) {
                                    self.fail_recording_screenshot(
                                        ctx,
                                        "The display changed while preparing the screenshot."
                                            .into(),
                                    );
                                    continue;
                                }
                                self.recording_screenshot_texture =
                                    session.frozen_image().map(|image| {
                                        ctx.load_texture(
                                            format!("recording-screenshot-windows-{generation}"),
                                            egui::ColorImage::from_rgba_unmultiplied(
                                                [image.width() as usize, image.height() as usize],
                                                image.as_raw(),
                                            ),
                                            egui::TextureOptions::LINEAR,
                                        )
                                    });
                                self.recording_screenshot_window_session = Some(session);
                                self.window_selector.lock().unwrap().reset();
                                self.recording_screenshot_phase =
                                    Some(RecordingScreenshotPhase::Selecting);
                                self.status =
                                    "Choose a window or the display. Press Escape to cancel."
                                        .into();
                                request_hidden_root_paint(ctx);
                            }
                            Err(error) => {
                                self.fail_recording_screenshot(ctx, error);
                            }
                        }
                        continue;
                    }
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
                                let error = "The selected display changed while preparing windows.";
                                if !(controls && self.fail_controls_switch(ctx, error.into())) {
                                    self.fail_capture(ctx, error.into());
                                }
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
                                self.controls_switch_from = None;
                                if self.controls_switching_display {
                                    let mut menu = self.controls.lock().unwrap();
                                    menu.reset_for_display_change();
                                    menu.end_in_flight();
                                }
                                // Shipping auto-starts after choosing another
                                // Full screen display, not when the menu opens on it.
                                let auto_capture_display = self.controls_auto_start
                                    && std::mem::take(&mut self.controls_switching_display)
                                    && self.controls.lock().unwrap().mode()
                                        == capture_controls::TargetMode::Display;
                                if auto_capture_display {
                                    let Some(flow) = &mut self.flow else {
                                        continue;
                                    };
                                    if let Err(error) =
                                        flow.start_countdown(self.controls_countdown_seconds)
                                    {
                                        self.fail_capture(ctx, error);
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
                            // Shipping `select_capture_display` restores the
                            // previous display and reports inline.
                            if !(controls && self.fail_controls_switch(ctx, error.clone())) {
                                self.fail_capture(ctx, error);
                            }
                        }
                    }
                }
                Reply::WindowCaptured { generation, result } => {
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
                            Ok(artifact) => self.accept_artifact(
                                *artifact,
                                "Screenshot captured while recording",
                                true,
                            ),
                            Err(error) => self.capture_failed(error),
                        }
                        continue;
                    }
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
                            true,
                        ),
                        Err(error) => self.capture_failed(error),
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
                    blurred,
                    arrive_blurred,
                    media,
                } if self.previews.accepts(&artifact_id, generation) => match result {
                    Ok(decoded) => {
                        self.finish_restore(&artifact_id, generation, true);
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
                        card.blurred = blurred.map(|image| {
                            ctx.load_texture(
                                format!("mini-preview-blur:{generation}:{artifact_id}"),
                                image,
                                egui::TextureOptions::LINEAR,
                            )
                        });
                        card.arrive_blurred = arrive_blurred.map(|image| {
                            ctx.load_texture(
                                format!("mini-preview-arrive:{generation}:{artifact_id}"),
                                image,
                                egui::TextureOptions::LINEAR,
                            )
                        });
                        card.media = media.map(std::sync::Arc::new);
                        card.depth_blurred.clear();
                        card.arrived_at = Some(Instant::now());
                        // A card appearing under a resting pointer must not
                        // open its hover chrome until the pointer moves.
                        self.previews.hover_lock_generation =
                            self.previews.hover_lock_generation.wrapping_add(1);
                        request_hidden_root_paint(ctx);
                        ctx.request_repaint();
                    }
                    Err(error) => {
                        let message = format!("Could not load mini preview: {error}");
                        if self.finish_restore(&artifact_id, generation, false) {
                            self.card_errors.insert(artifact_id.clone(), message);
                        } else {
                            self.error = Some(message);
                        }
                        self.previews.dismiss(&artifact_id, generation);
                    }
                },
                // A card dismissed before it decoded ends its Restore quietly.
                Reply::PreviewDecoded {
                    generation,
                    artifact_id,
                    ..
                } => {
                    self.finish_restore(&artifact_id, generation, false);
                }
            }
        }
        while let Ok(finished) = self.history_drag_rx.try_recv() {
            if let Err(error) = finished.result
                && self
                    .artifacts
                    .iter()
                    .any(|artifact| artifact.entry.id == finished.artifact_id)
            {
                self.card_errors.insert(finished.artifact_id, error);
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
        request_hidden_root_ui(ctx);
    }

    /// The recording controls' Screenshot button: shipping
    /// `start_capture_inner(Region)`, on the recording's display.
    fn start_recording_screenshot(&mut self, ctx: &egui::Context) {
        #[cfg(target_os = "linux")]
        if ctx.data(|data| data.get_temp::<bool>(egui::Id::unique("wayland-surface"))) == Some(true)
        {
            // The desktop chooses the screenshot extent; no native region or
            // monitor geometry is promised for this portal action.
            self.request_capture(CaptureRequest::Display);
            request_hidden_root_ui(ctx);
            return;
        }
        let Some(settings) = self.recording_screenshot_settings.as_ref().cloned() else {
            self.hud_action_failed(
                ctx,
                "Screenshot preferences are unavailable for this recording.".into(),
            );
            return;
        };
        let (Some(display_id), Some(target)) = (self.display_id.clone(), self.countdown_target)
        else {
            self.hud_action_failed(ctx, "The recording display is no longer available.".into());
            return;
        };
        let shot = RecordingSelectorShot {
            kind: RecordingSelectorKind::Region,
            display_id,
            target,
            includes_capture_ui: false,
        };
        if let Err(error) = self.begin_recording_selector_screenshot(ctx, shot, settings) {
            self.hud_action_failed(ctx, error);
        }
    }

    /// Shipping region and window shortcuts and tray items during a recording
    /// (`start_capture_inner(Region | Window)`): the selector opens on the
    /// display under the pointer, beside the take that keeps running, and
    /// uses the current screenshot preferences.
    fn start_recording_selector_screenshot(
        &mut self,
        ctx: &egui::Context,
        frame: &eframe::Frame,
        kind: RecordingSelectorKind,
        settings: AppSettings,
    ) {
        if !self.recording_screenshot_available() {
            return;
        }
        let pointer = captures_capture::pointer_position()
            .and_then(|point| captures_capture::XcapBackend.display_id_at_point(point));
        // Resolve without replacing the recording's display, which its
        // controls, region guide and HUD screenshot still use.
        let display_id = match resolve_capture_display(
            &mut self.displays,
            self.display_id.as_deref(),
            pointer.as_deref(),
            || {
                captures_capture::XcapBackend
                    .displays()
                    .map_err(|error| error.to_string())
            },
        ) {
            Ok(id) => id,
            Err(error) => {
                self.capture_failed(error);
                return;
            }
        };
        let Some(target) = capture_target(frame, &self.displays, Some(&display_id)) else {
            self.capture_failed(match kind {
                RecordingSelectorKind::Region => {
                    "The selected display is no longer available for region selection.".into()
                }
                RecordingSelectorKind::Window => {
                    "The selected display is no longer available for window selection.".into()
                }
            });
            return;
        };
        let shot = RecordingSelectorShot {
            kind,
            display_id,
            target,
            includes_capture_ui: false,
        };
        if let Err(error) = self.begin_recording_selector_screenshot(ctx, shot, settings) {
            self.capture_failed(error);
        }
    }

    /// Starts a region or window screenshot as a child of the recording's
    /// capture flow. The controls leave first unless they are opted into
    /// captures (shipping `hide_capture_huds_before_snapshot`).
    fn begin_recording_selector_screenshot(
        &mut self,
        ctx: &egui::Context,
        shot: RecordingSelectorShot,
        settings: AppSettings,
    ) -> Result<(), String> {
        let Some(parent) = self.flow.as_ref() else {
            return Ok(());
        };
        let flow = parent
            .begin_recording_screenshot(0)
            .map_err(|error| format!("Could not start recording screenshot: {error}"))?;
        if let Err(error) =
            self.previews
                .begin_capture(&settings, Some(shot.target), ctx.cumulative_frame_nr())
        {
            flow.cancel();
            return Err(error);
        }
        self.auto_copy_on_capture = settings.auto_copy_to_clipboard;
        self.include_cursor = settings.show_cursor_in_screenshots;
        self.status = match shot.kind {
            RecordingSelectorKind::Region => "Preparing region screenshot… Press Escape to cancel.",
            RecordingSelectorKind::Window => "Preparing window screenshot… Press Escape to cancel.",
        }
        .into();
        self.recording_screenshot_settings = Some(settings);
        self.recording_screenshot_flow = Some(flow);
        self.recording_selector_shot = Some(shot);
        self.recording_screenshot_phase = Some(RecordingScreenshotPhase::WaitingForHud);
        self.recording_screenshot_hide_started = Some(Instant::now());
        self.region_selector.lock().unwrap().reset();
        self.window_selector.lock().unwrap().reset();
        request_hidden_root_paint(ctx);
        ctx.request_repaint_after(Duration::from_millis(300));
        Ok(())
    }

    /// The display the current screenshot beside a recording counts down on.
    fn recording_screenshot_target(&self) -> Option<CaptureTarget> {
        match (&self.recording_display_shot, &self.recording_selector_shot) {
            (Some(shot), _) => shot.target,
            (None, Some(shot)) => Some(shot.target),
            (None, None) => None,
        }
    }

    /// The prepared selector still covers the display it was opened on.
    fn recording_selector_display_matches(&self, prepared: &DisplayDescriptor) -> bool {
        let Some(shot) = &self.recording_selector_shot else {
            return false;
        };
        self.displays
            .iter()
            .find(|display| display.id == shot.display_id)
            .is_some_and(|display| same_display_geometry(display, prepared))
    }

    /// Whether the recording controls stay in view during the current
    /// screenshot: only when they are opted into captures (shipping
    /// `conceal_capture_chrome_for_snapshot`).
    fn recording_screenshot_keeps_controls(&self) -> bool {
        if self.recording_screenshot_controls_restored {
            return true;
        }
        if let Some(shot) = &self.recording_display_shot {
            return shot.keeps_controls;
        }
        self.recording_selector_shot.is_some()
            && self
                .recording_screenshot_settings
                .as_ref()
                .is_some_and(|settings| settings.include_recording_controls_in_captures)
    }

    /// The countdown after a selection beside the take: none when the
    /// selector froze the previous one into its snapshot.
    fn recording_selector_countdown_seconds(&self) -> u8 {
        if self
            .recording_selector_shot
            .as_ref()
            .is_some_and(|shot| shot.includes_capture_ui)
        {
            return 0;
        }
        self.recording_screenshot_settings
            .as_ref()
            .map(|settings| settings.screenshot_countdown_seconds)
            .unwrap_or(0)
    }

    /// Shipping display shortcut and tray "Screenshot Display" during a
    /// recording: capture the display under the pointer with the screenshot
    /// countdown, without the recording controls unless they are opted into
    /// captures, into History, the clipboard and the mini previews. The
    /// recording keeps its flow, display and output; only a child flow runs.
    fn start_recording_display_screenshot(
        &mut self,
        ctx: &egui::Context,
        frame: &eframe::Frame,
        settings: &AppSettings,
    ) {
        if !self.recording_screenshot_available() {
            return;
        }
        let pointer = captures_capture::pointer_position()
            .and_then(|point| captures_capture::XcapBackend.display_id_at_point(point));
        // Resolve without replacing the recording's display, which its
        // controls, region guide and HUD screenshot still use.
        let display_id = match resolve_capture_display(
            &mut self.displays,
            self.display_id.as_deref(),
            pointer.as_deref(),
            || {
                captures_capture::XcapBackend
                    .displays()
                    .map_err(|error| error.to_string())
            },
        ) {
            Ok(id) => id,
            Err(error) => {
                self.capture_failed(error);
                return;
            }
        };
        let target = capture_target(frame, &self.displays, Some(&display_id));
        let countdown_seconds = settings.screenshot_countdown_seconds;
        if target.is_none() && countdown_seconds > 0 {
            self.capture_failed(
                "The selected display is no longer available for countdown.".into(),
            );
            return;
        }
        let Some(parent) = self.flow.as_ref() else {
            return;
        };
        let flow = match parent.begin_recording_screenshot(0) {
            Ok(flow) => flow,
            Err(error) => {
                self.capture_failed(error);
                return;
            }
        };
        if let Err(error) = self
            .previews
            .begin_capture(settings, target, ctx.cumulative_frame_nr())
        {
            flow.cancel();
            self.capture_failed(error);
            return;
        }
        self.auto_copy_on_capture = settings.auto_copy_to_clipboard;
        self.recording_screenshot_flow = Some(flow);
        self.recording_display_shot = Some(RecordingDisplayShot {
            display_id,
            target,
            countdown_seconds,
            include_cursor: settings.show_cursor_in_screenshots,
            keeps_controls: settings.include_recording_controls_in_captures,
        });
        self.recording_screenshot_phase = Some(RecordingScreenshotPhase::DisplayHidingControls);
        self.recording_screenshot_hide_started = Some(Instant::now());
        self.status = "Preparing screenshot… Press Escape to cancel.".into();
        request_hidden_root_paint(ctx);
        ctx.request_repaint();
    }

    /// Reports a failed screenshot in the shipping error dialog.
    fn capture_failed(&mut self, error: String) {
        self.capture_failure = Some(CaptureFailure {
            error,
            recording: false,
        });
    }

    /// Ends the current capture flow and reports why.
    fn fail_capture(&mut self, ctx: &egui::Context, error: String) {
        self.finish_capture(ctx, false);
        self.capture_failed(error);
    }

    /// Shipping `RecordingSelector`'s failed `start_recording` or
    /// `select_capture_display`: the menu stays open on its current display
    /// with its selections, the in-flight label ends and the error shows
    /// inline until the next start or switch, so the user can retry or pick
    /// something else. No dialog.
    fn keep_controls_open_with_error(&mut self, ctx: &egui::Context, error: String) {
        self.status = format!("{error} Retry or choose again. Press Escape to cancel.");
        self.controls_error = Some(error);
        self.controls.lock().unwrap().end_in_flight();
        // The next UI pass that declares the menu republishes its shortcut
        // scope for this generation.
        self.capture_phase = Some(CapturePhase::ControlsSelecting);
        request_hidden_root_paint(ctx);
        ctx.request_repaint();
    }

    /// A display switch failed: return the menu to the display it was on
    /// (shipping restores the previous selection and keeps `session.display`)
    /// with the inline error. Returns false when no switch was in flight, so
    /// the caller ends the capture as before.
    fn fail_controls_switch(&mut self, ctx: &egui::Context, error: String) -> bool {
        if !self.controls_switching_display {
            return false;
        }
        let Some((from, session, texture)) = self.controls_switch_from.take() else {
            return false;
        };
        self.controls_switching_display = false;
        self.display_id = Some(session.display().id.clone());
        self.countdown_target = Some(from);
        self.previews.capture_target = Some(from);
        self.window_session = Some(session);
        self.window_texture = texture;
        self.keep_controls_open_with_error(ctx, error);
        true
    }

    /// Ends a recording flow that could not continue and reports why.
    fn fail_recording(&mut self, ctx: &egui::Context, error: String) {
        self.finish_capture(ctx, false);
        self.capture_failure = Some(CaptureFailure {
            error,
            recording: true,
        });
    }

    fn fail_recording_screenshot(&mut self, ctx: &egui::Context, error: String) {
        self.finish_recording_screenshot(ctx, false);
        self.capture_failed(error);
    }

    fn finish_recording_screenshot(&mut self, ctx: &egui::Context, preserve_auto_copy: bool) {
        self.recording_screenshot_flow = None;
        self.recording_screenshot_controls_restored = false;
        self.requested_recapture = None;
        self.recapture = None;
        self.recording_display_shot = None;
        self.recording_selector_shot = None;
        self.recording_screenshot_phase = None;
        self.recording_screenshot_hide_started = None;
        self.recording_screenshot_session = None;
        self.recording_screenshot_window_session = None;
        self.recording_screenshot_texture = None;
        self.region_selector.lock().unwrap().reset();
        self.window_selector.lock().unwrap().reset();
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
        #[cfg(target_os = "linux")]
        if let Some(portal) = self.portal_screenshot.take() {
            // Source/session loss or parent finalization invalidates only an
            // uncommitted child. A worker already persisting owns its result.
            portal.flow.cancel();
        }
        if is_recording_phase(self.capture_phase) {
            // A failed session deliberately holds its recovery-root lease until
            // the owner is dropped. List only after that worker acknowledges it.
            self.pending += 1;
            self.recording_worker.send(recording::Command::Retire);
        }
        self.selector_scope_generation.store(0, Ordering::Release);
        self.recording_screenshot_flow = None;
        self.recording_screenshot_controls_restored = false;
        self.requested_recapture = None;
        self.recapture = None;
        self.recording_screenshot_phase = None;
        self.recording_screenshot_hide_started = None;
        self.recording_screenshot_session = None;
        self.recording_screenshot_window_session = None;
        self.recording_screenshot_texture = None;
        self.recording_screenshot_settings = None;
        self.recording_selector_shot = None;
        self.recording_display_shot = None;
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
        self.controls_switch_from = None;
        self.window_selector.lock().unwrap().reset();
        self.controls.lock().unwrap().reset();
        self.workspace_hidden = false;
        for id in self.recording_portal_windows.drain(..) {
            if id != main_countdown_viewport() {
                ctx.send_viewport_cmd_to(id, egui::ViewportCommand::Visible(true));
                ctx.request_repaint_of(id);
            }
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(self.restore_root_visible));
        request_hidden_root_paint(ctx);
        ctx.request_repaint();
    }

    fn accept_artifact(&mut self, artifact: Artifact, status: &str, preview: bool) {
        let id = artifact.entry.id.clone();
        let path = artifact.image_path.clone();
        let preview = if preview {
            self.previews.start_artifact(&artifact)
        } else {
            Ok(None)
        };
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
                self.accept_artifact(artifact, "Full display captured as PNG", true);
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
        self.sharing.show(ctx, tokens, self.workspace_hidden);
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
        // Finished exits and flights leave before the stack is measured; the
        // root wakes once for the next one to end.
        self.previews.exits.sync(self.previews.stack.ids());
        if self.previews.settle_exits(reduced_motion) {
            request_hidden_root_paint(ctx);
            ctx.request_repaint();
        }
        // A saved card's Trash goes out once its dust has played.
        for (artifact_id, generation, saved_path) in self.previews.due_trash(reduced_motion) {
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
        if let Some(wait) = self
            .previews
            .exits
            .next_finish_in_ms(self.previews.now_ms())
        {
            request_hidden_root_paint(ctx);
            ctx.request_repaint_after(Duration::from_secs_f64(wait / 1000.));
        }
        if let Some(fly) = self.previews.fly {
            request_hidden_root_paint(ctx);
            ctx.request_repaint_after(STACK_FLY.saturating_sub(fly.started.elapsed()));
        }
        {
            let shown = crate::mini_preview::stack_controls_visible(
                self.previews.stack.ids().len(),
                self.previews.stack.is_collapsed(),
            );
            let now_ms = self.previews.now_ms();
            self.previews.toolbar.set(
                shown,
                captures_app::preview_motion::ToolbarCause::Other,
                now_ms,
            );
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
        let fly = self.previews.fly;
        // A collapsing stack keeps its list window until the fly lands.
        let collapsed =
            self.previews.stack.is_collapsed() && !fly.is_some_and(|fly| fly.collapsing);
        let placement = self.previews.placement;
        let top_anchor = placement.is_top();
        let display = if collapsed {
            self.previews.stack.ids().to_vec()
        } else {
            self.previews.exits.display_ids().to_vec()
        };
        let display_count = display.len().max(count);
        let settle = captures_app::preview_motion::settle_tween();
        let exits_now = self.previews.now_ms();
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
        // Editor presence follows this host's open screenshot editors; the
        // shared rules time the leave and linger, waking once per change.
        let presence_now = crate::motion::elapsed_ms(self.previews.epoch, now);
        for (artifact_id, card) in &mut self.previews.cards {
            let active = self
                .editors
                .get(artifact_id)
                .is_some_and(|editor| !editor.closed());
            card.editor.set_active(active, presence_now, reduced_motion);
            if let Some(wait) = card.editor.next_change_in_ms(presence_now, reduced_motion) {
                request_hidden_root_paint(ctx);
                ctx.request_repaint_after(Duration::from_secs_f64(wait / 1000.));
            }
        }
        let epoch = self.previews.epoch;
        let hover_lock_generation = self.previews.hover_lock_generation;
        let live_ids = self.previews.stack.ids().to_vec();
        // Corner piles sit at full gravity; a dragged pile derives it from
        // its position, fading the paper spin in toward the screen middle.
        let pile_gravity = captures_app::preview::collapsed_stack_gravity(
            preview_bounds,
            count.max(1),
            self.previews.visibility.stack_origin(),
            placement,
        );
        // Rear pile media blurs (`pose × 1.15px`, `× 0.75px` fanned): real
        // Gaussians of each card's media, built once per radius on the first
        // compact frame at that depth; settled frames reuse them.
        if collapsed || fly.is_some() {
            for (live_index, artifact_id) in live_ids.iter().enumerate() {
                let Some(depth) = count.checked_sub(live_index + 1) else {
                    continue;
                };
                if let Some(card) = self.previews.cards.get_mut(artifact_id) {
                    card.prepare_depth_blurs(ctx, depth);
                }
            }
        }
        // Delete's blurred dust chips, once per exit.
        let radius = tokens.number("thumbnail-card-radius");
        for exiting in self.previews.exiting.values_mut() {
            if exiting.dust_atlas.is_none()
                && !exiting.dust.is_empty()
                && let Some(media) = &exiting.media
                && let Some((atlas, cells)) =
                    crate::mini_preview::dust_atlas(media, &exiting.dust, radius)
            {
                exiting.dust_atlas = Some((
                    ctx.load_texture("mini-preview-dust", atlas, egui::TextureOptions::LINEAR),
                    std::sync::Arc::new(cells),
                ));
            }
        }
        // The Close streak's stepped horizontal blurs, once per exit.
        for (id, exiting) in &mut self.previews.exiting {
            if self
                .previews
                .exits
                .exiting(id)
                .is_some_and(|exit| exit.kind == captures_app::preview_motion::ExitKind::Dismiss)
                && exiting.streak.is_empty()
                && let Some(media) = &exiting.media
            {
                exiting.streak = crate::mini_preview::streak_blur_images(media)
                    .into_iter()
                    .map(|image| {
                        ctx.load_texture("mini-preview-streak", image, egui::TextureOptions::LINEAR)
                    })
                    .collect();
            }
        }
        let cards = display
            .iter()
            .enumerate()
            .filter_map(|(index, artifact_id)| {
                let card = self.previews.cards.get(artifact_id)?;
                let live_index = live_ids.iter().position(|id| id == artifact_id)?;
                let pile_depth = count.checked_sub(live_index + 1)?;
                let pile_pose = |hovered: bool| {
                    captures_app::preview::collapsed_card_pose(
                        artifact_id,
                        pile_depth,
                        hovered,
                        pile_gravity,
                        top_anchor,
                    )
                };
                let layout = |collapsed: bool, hovered: bool| {
                    captures_app::preview::card_layout_in(
                        display_count,
                        index,
                        collapsed,
                        top_anchor,
                        hovered,
                    )
                };
                Some(PreviewRenderCard {
                    artifact_id: artifact_id.clone(),
                    generation: card.generation,
                    width: card.width,
                    height: card.height,
                    texture: card.texture.clone()?,
                    blurred: card.blurred.clone(),
                    arrive_blurred: card.arrive_blurred.clone(),
                    depth_blurred: [true, false].map(|hovered| {
                        let radius =
                            captures_app::preview::collapsed_media_blur(pile_depth, hovered) as f32;
                        (radius, card.depth_blur(radius))
                    }),
                    size_bytes: card.size_bytes,
                    busy: card.busy,
                    message: card.message.clone(),
                    saved_path: card.saved_path.clone(),
                    saved_at: card
                        .saved_at
                        .filter(|at| now.saturating_duration_since(*at) < SAVED_FEEDBACK),
                    clipboard_current: clipboard_owner.as_deref() == Some(artifact_id.as_str()),
                    editor_phase: card.editor.phase(),
                    editor_since: epoch
                        + Duration::from_secs_f64(card.editor.since_ms().max(0.) / 1000.),
                    rejected_at: card.rejected_at,
                    arrived_at: card.arrived_at,
                    pile_depth,
                    layout: layout(collapsed, false)?,
                    hover_y: layout(collapsed, true)?.y,
                    pile_y: captures_app::preview::card_layout_in(
                        count, live_index, true, top_anchor, false,
                    )?
                    .y,
                    pile_rest: pile_pose(false),
                    pile_hover: pile_pose(true),
                    shift_y: self.previews.exits.shift_px(
                        artifact_id,
                        exits_now,
                        reduced_motion,
                        &settle,
                        top_anchor,
                    ) as f32,
                    copy_failed: card.copy_failed,
                })
            })
            .collect::<Vec<_>>();
        let exiting = display
            .iter()
            .enumerate()
            .filter_map(|(index, artifact_id)| {
                let card = self.previews.exiting.get(artifact_id)?;
                let exit = self.previews.exits.exiting(artifact_id)?;
                Some(PreviewExitRender {
                    texture: card.texture.clone(),
                    blurred: card.blurred.clone(),
                    streak: card.streak.clone(),
                    dust_atlas: card.dust_atlas.clone(),
                    kind: exit.kind,
                    elapsed_ms: exit.elapsed_ms(exits_now),
                    dust: card.dust.clone(),
                    layout: captures_app::preview::card_layout_in(
                        display_count,
                        index,
                        false,
                        top_anchor,
                        false,
                    )?,
                    shift_y: self.previews.exits.shift_px(
                        artifact_id,
                        exits_now,
                        reduced_motion,
                        &settle,
                        top_anchor,
                    ) as f32,
                })
            })
            .collect::<Vec<_>>();
        if cards.is_empty() && exiting.is_empty() {
            return;
        }
        let toolbar = self.previews.toolbar;
        let pile_geometry = captures_app::preview::thumbnail_geometry(
            preview_bounds,
            count.max(1),
            true,
            self.previews.visibility.stack_origin(),
            placement,
        );
        let tokens = tokens.clone();
        let drag_root = self.root.clone();
        let geometry = captures_app::preview::thumbnail_geometry(
            preview_bounds,
            display_count,
            collapsed,
            self.previews.visibility.stack_origin(),
            placement,
        );
        let sender = self.preview_tx.clone();
        let clear_ids = self.previews.stack.ids().to_vec();
        let content_height = if collapsed {
            self.previews.stack.content_height()
        } else {
            captures_app::preview::stack_height(display_count)
        };
        let scroll_content_height =
            (content_height - captures_app::preview::THUMBNAIL_CONTROL_GUTTER).max(0.) as f32;
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
                // Shipping stale-pointer suppression: after an expand or a new
                // card, card hover waits until the pointer really moves.
                let lock_id = egui::Id::unique("mini-preview-hover-lock");
                let geometry_key = [
                    geometry.x.round() as i64,
                    geometry.y.round() as i64,
                    geometry.height.round() as i64,
                ];
                let (mut hover_lock, seen_generation, seen_geometry) = ui
                    .data(|data| {
                        data.get_temp::<(captures_app::preview_chrome::CardHoverLock, u64, [i64; 3])>(
                            lock_id,
                        )
                    })
                    .map_or((Default::default(), None, geometry_key), |(lock, generation, key)| {
                        (lock, Some(generation), key)
                    });
                if seen_generation != Some(hover_lock_generation) {
                    hover_lock.lock();
                } else if seen_geometry != geometry_key {
                    hover_lock.resample_origin();
                }
                let hover_locked = hover_lock.pointer(
                    ui.input(|input| input.pointer.hover_pos())
                        .map(|point| (f64::from(point.x), f64::from(point.y))),
                );
                ui.data_mut(|data| {
                    data.insert_temp(lock_id, (hover_lock, hover_lock_generation, geometry_key))
                });
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
                let highlight = tokens.motion(captures_app::motion::Motion::PreviewCaptureHighlight);
                let frame_ms = crate::motion::elapsed_ms(epoch, Instant::now());
                // The list ↔ pile flight, eased on the shipping curve.
                let fly_tween = tokens.transition(captures_app::motion::Transition::PreviewStackFly);
                let flight = fly.map(|fly| {
                    let elapsed = crate::motion::elapsed_ms(fly.started, Instant::now());
                    if fly_tween.running(elapsed, reduced_motion) {
                        ui.ctx().request_repaint();
                    }
                    (fly.collapsing, fly_tween.progress(elapsed, reduced_motion) as f32)
                });
                let mut show_card = |ui: &mut egui::Ui,
                                     card: &PreviewRenderCard,
                                     compact: bool,
                                     interactive: bool,
                                     depth_shade: f32,
                                     pile: crate::mini_preview::PileTransform| {
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
                    // Shipping `thumbnail-capture-highlight` from the card's arrival.
                    let outline = card.arrived_at.map_or(0., |at| {
                        let elapsed = crate::motion::elapsed_ms(at, Instant::now());
                        if highlight.running(elapsed, reduced_motion) {
                            ui.ctx().request_repaint();
                        }
                        highlight.pose_at(elapsed, reduced_motion).opacity as f32
                    });
                    // Shipping writes every native capture to History first,
                    // so only a failed copy can warn here.
                    let warning = captures_app::preview_chrome::card_warning(
                        card.clipboard_current,
                        true,
                        card.copy_failed,
                    );
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
                                interactive: interactive && card.layout.interactive,
                                collapsed: compact,
                                stack_count: count,
                                depth: card.layout.depth,
                                desktop_pointer,
                                reject_offset,
                                right_anchor: placement.is_right(),
                                top_anchor,
                                blurred: card.blurred.as_ref(),
                                arrive_blur: (arrival.blur as f32, card.arrive_blurred.as_ref()),
                                depth_blurred: [
                                    (card.depth_blurred[0].0, card.depth_blurred[0].1.as_ref()),
                                    (card.depth_blurred[1].0, card.depth_blurred[1].1.as_ref()),
                                ],
                                editor: card.editor_phase,
                                editor_elapsed_ms: crate::motion::elapsed_ms(
                                    card.editor_since,
                                    Instant::now(),
                                ),
                                hover_locked,
                                reduced_motion,
                                highlight: outline,
                                warning,
                                depth_shade,
                                pile,
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
                        Some(crate::mini_preview::Action::Share) => Some(PreviewMessage::Share {
                            artifact_id: card.artifact_id.clone(),
                            generation: card.generation,
                        }),
                        Some(crate::mini_preview::Action::Dismiss) => {
                            Some(PreviewMessage::Dismiss {
                                artifact_id: card.artifact_id.clone(),
                                generation: card.generation,
                                delete: false,
                            })
                        }
                        Some(crate::mini_preview::Action::Discard) => {
                            Some(PreviewMessage::Dismiss {
                                artifact_id: card.artifact_id.clone(),
                                generation: card.generation,
                                delete: true,
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
                    // Shipping's hover fan: each layer eases on its own
                    // transition, 16 ms later per depth.
                    let fan_tween =
                        tokens.transition(captures_app::motion::Transition::PreviewStackFan);
                    let mut fan: captures_app::preview_motion::StackFan =
                        ui.data(|data| data.get_temp(fan_id())).unwrap_or_default();
                    let pile_cards: Vec<(&str, usize)> = cards
                        .iter()
                        .map(|card| (card.artifact_id.as_str(), card.pile_depth))
                        .collect();
                    if fan.set(fan_open, frame_ms, &pile_cards, &fan_tween, reduced_motion) {
                        ui.data_mut(|data| data.insert_temp(fan_id(), fan.clone()));
                    }
                    let deepest = pile_cards.iter().map(|card| card.1).max().unwrap_or(0);
                    if fan.running(deepest, frame_ms, &fan_tween, reduced_motion) {
                        ui.ctx().request_repaint();
                    }
                    // Shipping drag sway: once a carry has held the fan open
                    // for its gather, the rear cards lean with the pointer's
                    // velocity; dropping eases the lean back with the fan.
                    let mut carry: PileCarry =
                        ui.data(|data| data.get_temp(carry_id())).unwrap_or_default();
                    let lean = carry.frame(
                        frame_ms,
                        ui.input(|input| input.pointer.primary_down()),
                        desktop_pointer,
                        captures_app::preview_motion::StackFan::settle_ms(deepest, &fan_tween),
                        reduced_motion,
                    );
                    if carry.moving(frame_ms, deepest, &fan_tween, reduced_motion) {
                        ui.ctx().request_repaint();
                    }
                    for card in &cards {
                        let (pose, media) = fan.progress(
                            &card.artifact_id,
                            card.pile_depth,
                            frame_ms,
                            &fan_tween,
                            reduced_motion,
                        );
                        let sway = lean(card.pile_depth, &fan_tween);
                        let swayed = (sway != (0., 0.)).then(|| {
                            captures_app::preview::collapsed_card_sway_pose(
                                &card.artifact_id,
                                card.pile_depth,
                                pile_gravity,
                                top_anchor,
                                sway,
                            )
                        });
                        let (offset, pile) = crate::mini_preview::pile_pose_between_staggered(
                            &card.pile_rest,
                            swayed.as_ref().unwrap_or(&card.pile_hover),
                            pose as f32,
                            media as f32,
                        );
                        let y = egui::lerp(
                            card.layout.y as f32..=card.hover_y as f32,
                            pose as f32,
                        );
                        let rect = egui::Rect::from_min_size(
                            egui::pos2(captures_app::preview::THUMBNAIL_PADDING as f32, y) + offset,
                            egui::vec2(
                                (captures_app::preview::THUMBNAIL_WIDTH
                                    - captures_app::preview::THUMBNAIL_PADDING * 2.)
                                    as f32,
                                captures_app::preview::THUMBNAIL_CARD_HEIGHT as f32,
                            ),
                        );
                        ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                            show_card(ui, card, true, true, 1., pile);
                        });
                    }
                    if matches!(message, Some(PreviewMessage::MoveStack { .. })) {
                        carry.start(frame_ms);
                    }
                    ui.data_mut(|data| data.insert_temp(carry_id(), carry));
                    // Shipping sparkles drift over the hovered pile until the
                    // pointer leaves.
                    let sparkle_id = egui::Id::unique("mini-preview-sparkle");
                    let now = ui.input(|input| input.time);
                    let since = fan_open.then(|| {
                        ui.data(|data| data.get_temp::<f64>(sparkle_id)).unwrap_or(now)
                    });
                    ui.data_mut(|data| match since {
                        Some(since) => {
                            data.insert_temp(sparkle_id, since);
                        }
                        None => data.remove::<f64>(sparkle_id),
                    });
                    if let (Some(since), Some(rect)) = (since, front_rect)
                        && crate::mini_preview::paint_sparkles(
                            ui,
                            &tokens,
                            rect,
                            top_anchor,
                            (now - since) * 1000.,
                            reduced_motion,
                        )
                    {
                        ui.ctx().request_repaint();
                    }
                } else if let Some((collapsing, progress)) = flight {
                    // Cards fly between their list slots and the pile with the
                    // compact look; the window keeps the list's size.
                    let gutter = captures_app::preview::THUMBNAIL_CONTROL_GUTTER as f32;
                    let padding = captures_app::preview::THUMBNAIL_PADDING as f32;
                    let viewport = geometry.height as f32 - gutter;
                    // The expanded list rests at its newest end.
                    let scroll = if top_anchor {
                        0.
                    } else {
                        (scroll_content_height - viewport).max(0.)
                    };
                    let pile_offset = egui::vec2(
                        (pile_geometry.x - geometry.x) as f32,
                        (pile_geometry.y - geometry.y) as f32,
                    );
                    let size = egui::vec2(
                        (captures_app::preview::THUMBNAIL_WIDTH
                            - captures_app::preview::THUMBNAIL_PADDING * 2.)
                            as f32,
                        captures_app::preview::THUMBNAIL_CARD_HEIGHT as f32,
                    );
                    let toward_pile = if collapsing { progress } else { 1. - progress };
                    // Show less lands on the resting pile. Expand starts from
                    // the pose each card had when it began (shipping's
                    // `--thumbnail-stack-expand-from` and `-expand-blur-from`),
                    // usually the fanned pile under the pointer.
                    let fan = if collapsing {
                        ui.data_mut(|data| {
                            data.remove::<captures_app::preview_motion::StackFan>(fan_id())
                        });
                        None
                    } else {
                        ui.data(|data| {
                            data.get_temp::<captures_app::preview_motion::StackFan>(fan_id())
                        })
                    };
                    let fan_tween =
                        tokens.transition(captures_app::motion::Transition::PreviewStackFan);
                    let expand_ms = fly.map_or(frame_ms, |fly| {
                        crate::motion::elapsed_ms(epoch, fly.started)
                    });
                    for card in &cards {
                        // The pile end carries its depth pose; the list end is flat.
                        let (pose_t, media_t) = fan.as_ref().map_or((0., 0.), |fan| {
                            fan.progress(
                                &card.artifact_id,
                                card.pile_depth,
                                expand_ms,
                                &fan_tween,
                                reduced_motion,
                            )
                        });
                        let (pose_offset, pose) =
                            crate::mini_preview::pile_pose_between_staggered(
                                &card.pile_rest,
                                &card.pile_hover,
                                pose_t as f32,
                                media_t as f32,
                            );
                        let list = egui::pos2(padding, card.layout.y as f32 - scroll);
                        let pile = egui::pos2(padding, card.pile_y as f32) + pile_offset + pose_offset;
                        let rect = egui::Rect::from_min_size(list.lerp(pile, toward_pile), size);
                        let flight = crate::mini_preview::PileTransform::IDENTITY.lerp(pose, toward_pile);
                        ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                            show_card(ui, card, true, false, toward_pile, flight);
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
                    ui.data_mut(|data| {
                        data.remove::<captures_app::preview_motion::StackFan>(fan_id())
                    });
                    // Overflow cues request whole-slot scrolls; apply them
                    // inside the scroll area on the next pass so egui
                    // animates the move and releases stick-to-bottom.
                    let cue_scroll_id = egui::Id::unique("preview-stack-cue-scroll");
                    let cue_scroll = ui.data_mut(|data| data.remove_temp::<f32>(cue_scroll_id));
                    let scroll =
                        ui.scope_builder(egui::UiBuilder::new().max_rect(card_area), |ui| {
                            egui::ScrollArea::vertical()
                                .id_salt("preview-stack-scroll")
                                // Shipping `.thumbnail-stack` hides its scroll bar.
                                .scroll_bar_visibility(
                                    egui::scroll_area::ScrollBarVisibility::AlwaysHidden,
                                )
                                .auto_shrink([false, false])
                                .stick_to_bottom(!top_anchor)
                                .show(ui, |ui| {
                                    if let Some(delta) = cue_scroll {
                                        ui.scroll_with_delta(egui::vec2(0., -delta));
                                        // ScrollArea applies the offset after this
                                        // pass paints. Present the new card positions
                                        // without waiting for another pointer event.
                                        ui.ctx().request_repaint();
                                    }
                                    let (content, _) = ui.allocate_exact_size(
                                        egui::vec2(ui.available_width(), scroll_content_height),
                                        egui::Sense::hover(),
                                    );
                                    for card in &cards {
                                        let y = card.layout.y as f32
                                            - if top_anchor { gutter } else { 0. }
                                            + card.shift_y;
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
                                            |ui| {
                                                show_card(
                                                    ui,
                                                    card,
                                                    false,
                                                    true,
                                                    1.,
                                                    crate::mini_preview::PileTransform::IDENTITY,
                                                )
                                            },
                                        );
                                    }
                                    // Exiting cards hold their slots and paint
                                    // above the survivors sliding into them.
                                    for exit in &exiting {
                                        let y = exit.layout.y as f32
                                            - if top_anchor { gutter } else { 0. }
                                            + exit.shift_y;
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
                                        crate::mini_preview::show_exit(
                                            ui,
                                            &tokens,
                                            rect,
                                            crate::mini_preview::ExitView {
                                                texture: &exit.texture,
                                                blurred: exit.blurred.as_ref(),
                                                streak: &exit.streak,
                                                dust_atlas: exit.dust_atlas.as_ref().map(
                                                    |(atlas, cells)| (atlas, cells.as_slice()),
                                                ),
                                                kind: exit.kind,
                                                elapsed_ms: exit.elapsed_ms,
                                                dust: &exit.dust,
                                                right_anchor: placement.is_right(),
                                                reduced_motion,
                                            },
                                        );
                                    }
                                    if exiting
                                        .iter()
                                        .any(|exit| exit.elapsed_ms < exit.kind.hold_ms())
                                    {
                                        // Exits and the survivor settle move
                                        // every frame until they end; a
                                        // dissolved card waiting on its Trash
                                        // holds an empty slot without frames.
                                        ui.ctx().request_repaint();
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
                // The stack toolbar enters with an expand and leaves with a
                // collapse, the last Close or Delete, or Clear all.
                let toolbar_pose = toolbar.pose(&tokens, frame_ms, reduced_motion);
                if toolbar.running(&tokens, frame_ms, reduced_motion) {
                    ui.ctx().request_repaint();
                }
                let toolbar_interactive = toolbar.interactive(&tokens, frame_ms, reduced_motion);
                if let Some(pose) = toolbar_pose {
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
                    let action = ui
                        .scope_builder(egui::UiBuilder::new().max_rect(controls), |ui| {
                            crate::motion::with_pose(ui, pose, controls, |ui| {
                                crate::mini_preview::show_stack_controls(
                                    ui,
                                    &tokens,
                                    placement.is_right(),
                                    top_anchor,
                                    reduced_motion,
                                )
                            })
                        })
                        .inner;
                    // Shipping ignores the toolbar while it enters or leaves.
                    if toolbar_interactive {
                        match action {
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
                    }
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
            && let (Some(flow), Some(snapshot)) = (&self.flow, self.recording_snapshot.clone())
        {
            let generation = flow.generation();
            let running = matches!(
                hud_state,
                RecordingState::Recording | RecordingState::Finalizing
            );
            let restart_confirmation = self.recording_restart_confirmation;
            let delete_confirmation = self.recording_delete_confirmation;
            // A screenshot keeps the controls only when they are opted into
            // captures (shipping `conceal_capture_chrome_for_snapshot`).
            let controls_hidden = self.recording_controls_hidden == Some(generation)
                || (self.recording_screenshot_flow.is_some()
                    && !self.recording_screenshot_keeps_controls());
            #[cfg(target_os = "linux")]
            let controls_hidden = controls_hidden
                || self
                    .portal_screenshot
                    .as_ref()
                    .is_some_and(|portal| portal.recording);
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
            let position = self.countdown_target.map(|target| {
                target.position
                    + egui::vec2(
                        (target.size.x - crate::recording_hud::SIZE.x).max(0.) / 2.,
                        (target.size.y - crate::recording_hud::SIZE.y).max(0.) - 20.,
                    )
            });
            ctx.show_viewport_deferred(
                egui::ViewportId::from_hash_of("recording-controls"),
                egui::ViewportBuilder {
                    position,
                    ..egui::ViewportBuilder::default()
                    .with_title("Captures Recording Controls")
                    .with_inner_size(crate::recording_hud::SIZE)
                    .with_transparent(true)
                    .with_decorations(false)
                    .with_always_on_top()
                },
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
                    let notice = if snapshot.options.target == RecordingTarget::PortalWindow {
                        "Capture is limited to the portal-selected window"
                    } else if cfg!(target_os = "linux") || include_controls {
                        if hide_available {
                            "These controls will show in recordings · Use Hide controls to keep them out"
                        } else {
                            "These controls will show in recordings · Hide unavailable without a tray"
                        }
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
                && self.recording_screenshot_flow.is_none()
                && let Some(deadline) = self
                    .recording_hidden_notice_until
                    .filter(|deadline| *deadline > Instant::now())
            {
                let notice_tokens = t.clone();
                let copy = captures_app::recording_hud::hidden_notice_copy(
                    &self.new_capture_shortcut,
                    crate::preferences::shortcut_platform(),
                );
                let notice_size = egui::vec2(
                    captures_app::recording_hud::HIDDEN_NOTICE_WIDTH as f32,
                    captures_app::recording_hud::HIDDEN_NOTICE_HEIGHT as f32,
                );
                // Centred where the HUD was, like shipping `hide_recording_controls`.
                let offset = (crate::recording_hud::SIZE - notice_size) / 2.;
                ctx.show_viewport_deferred(
                    egui::ViewportId::from_hash_of("recording-controls-hidden"),
                    egui::ViewportBuilder {
                        position: position.map(|position| position + offset),
                        ..egui::ViewportBuilder::default()
                            .with_title(captures_app::recording_hud::HIDDEN_NOTICE_TITLE)
                            .with_inner_size(notice_size)
                            .with_transparent(true)
                            .with_decorations(false)
                            .with_always_on_top()
                            .with_mouse_passthrough(true)
                    },
                    move |ui, _| {
                        notice_tokens.glass_controls(ui);
                        // Shipping `recording-controls-hidden-lifecycle` (6 s) ends
                        // 200 ms before the 6.2 s window closes.
                        let lifecycle = notice_tokens
                            .motion(captures_app::motion::Motion::RecordingControlsHiddenLifecycle);
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
                            crate::recording_hud::show_hidden_notice(ui, &notice_tokens, &copy);
                        });
                    },
                );
            }
            if restart_confirmation {
                let tokens = t.clone();
                ctx.show_viewport_deferred(
                    egui::ViewportId::from_hash_of("recording-restart-confirmation"),
                    egui::ViewportBuilder {
                        position: position.map(|position| position + egui::vec2(35., -170.)),
                        ..egui::ViewportBuilder::default()
                        .with_title("Restart recording?")
                        .with_inner_size([360., 150.])
                        .with_always_on_top()
                        .with_resizable(false)
                    },
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
                    egui::ViewportBuilder {
                        position: position.map(|position| position + egui::vec2(35., -170.)),
                        ..egui::ViewportBuilder::default()
                            .with_title("Delete recording?")
                            .with_inner_size([360., 150.])
                            .with_always_on_top()
                            .with_resizable(false)
                    },
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
        // The menu stays up while a start or display switch it sent is in
        // flight ("Starting…", "Switching…"), as shipping's selector does
        // until `start_recording` or `select_capture_display` returns.
        let selecting = self.capture_phase == Some(CapturePhase::ControlsSelecting);
        let menu = match self.capture_phase {
            Some(CapturePhase::ControlsSelecting | CapturePhase::RecordingPreparing { .. }) => self
                .countdown_target
                .zip(self.window_session.clone())
                .map(|(target, session)| (target, session, self.window_texture.clone())),
            Some(CapturePhase::ControlsPreparing) => self.controls_switch_from.clone(),
            _ => None,
        };
        if let Some((target, session, texture)) = menu
            && let Some(generation) = self.flow.as_ref().map(CaptureFlow::generation)
        {
            let t = t.clone();
            // Publish selector shortcut scope only in the UI pass that declares
            // the child, never during an earlier hidden-root logic-only pass.
            if selecting {
                self.selector_scope_generation
                    .store(generation, Ordering::Release);
            }
            let controls = Arc::clone(&self.controls);
            let selector_scope_generation = Arc::clone(&self.selector_scope_generation);
            let sender = self.selector_tx.clone();
            let auto_start = self.controls_auto_start;
            let controls_error = self.controls_error.clone();
            let displays = self.displays.clone();
            let recording_unavailable_reason = (!self.recording_toolchain_ready).then(|| {
                self.recording_toolchain_error
                    .clone()
                    .unwrap_or_else(|| "Checking FFmpeg and ffprobe availability…".to_owned())
            });
            ctx.show_viewport_deferred(
                capture_controls_viewport(target.monitor),
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
                    let (action, in_flight, list_microphones) = {
                        let mut controls = controls.lock().unwrap();
                        let action = controls.show(
                            ui,
                            &t,
                            capture_controls::View {
                                panel_id: egui::Id::unique((
                                    "capture-controls-toolbar",
                                    generation,
                                )),
                                frozen: texture.as_ref(),
                                display: session.display(),
                                displays: &displays,
                                windows: session.windows(),
                                auto_start,
                                recording_available: recording_unavailable_reason.is_none(),
                                recording_unavailable_reason: recording_unavailable_reason
                                    .as_deref(),
                                error: controls_error.as_deref(),
                            },
                            |point| session.hit_test(point),
                        );
                        (
                            action,
                            controls.in_flight(),
                            controls.take_microphone_request(),
                        )
                    };
                    if list_microphones {
                        let _ = sender.send(SelectorMessage::ListMicrophones { generation });
                        ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
                    }
                    if action == Some(capture_controls::Action::Cancel) && in_flight.is_some() {
                        // The host no longer takes menu actions once a start or
                        // switch is in flight; cancel its flow like Close does.
                        let _ = selector_scope_generation.compare_exchange(
                            generation,
                            0,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        );
                        captures_app::capture_flow::cancel(generation);
                        ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
                    } else if let Some(action) = action {
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
        // A display recapture keeps the selector up until it is captured.
        let recording_selector = matches!(
            self.recording_screenshot_phase,
            Some(RecordingScreenshotPhase::Selecting | RecordingScreenshotPhase::DisplayCapturing)
        )
        .then(|| self.recording_selector_shot.clone())
        .flatten();
        let recording_region = recording_selector
            .as_ref()
            .filter(|shot| shot.kind == RecordingSelectorKind::Region);
        if self.capture_phase == Some(CapturePhase::RegionSelecting) || recording_region.is_some() {
            let t = t.clone();
            let recording_screenshot = recording_region.is_some();
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
            let target = recording_region
                .map(|shot| shot.target)
                .or(self.countdown_target)
                .expect("region target validated");
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
        let recording_window = recording_selector
            .as_ref()
            .filter(|shot| shot.kind == RecordingSelectorKind::Window);
        if self.capture_phase == Some(CapturePhase::WindowSelecting) || recording_window.is_some() {
            let t = t.clone();
            // A window screenshot beside a recording uses its own child flow,
            // session and viewport; the selector itself is the same.
            let (generation, target, texture, session, viewport_id) =
                if let Some(shot) = recording_window {
                    let generation = self
                        .recording_screenshot_flow
                        .as_ref()
                        .expect("recording screenshot selection owns flow")
                        .generation();
                    (
                        generation,
                        shot.target,
                        self.recording_screenshot_texture.clone(),
                        Arc::clone(
                            self.recording_screenshot_window_session
                                .as_ref()
                                .expect("recording screenshot selection owns window session"),
                        ),
                        egui::ViewportId::from_hash_of((
                            "recording-screenshot-window-selector",
                            generation,
                        )),
                    )
                } else {
                    (
                        self.flow
                            .as_ref()
                            .expect("selection owns flow")
                            .generation(),
                        self.countdown_target.expect("window target validated"),
                        self.window_texture.clone(),
                        Arc::clone(
                            self.window_session
                                .as_ref()
                                .expect("selection owns window session"),
                        ),
                        egui::ViewportId::from_hash_of("window-selector"),
                    )
                };
            let selector = Arc::clone(&self.window_selector);
            let sender = self.selector_tx.clone();
            // Shipping direct window overlay commits the clicked target.
            let auto_start = true;
            ctx.show_viewport_deferred(
                viewport_id,
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
        #[cfg(target_os = "linux")]
        if let Some(portal) = &self.portal_screenshot
            && let Some(clock) = portal.countdown
        {
            let t = t.clone();
            let generation = portal.flow.generation();
            // Omission would drop the viewport before its queued hide command
            // is processed. Keep it alive and hidden through portal completion.
            ctx.show_viewport_deferred(
                recording_screenshot_countdown_viewport(generation),
                countdown_viewport("Captures Screenshot Countdown", None, &t)
                    .with_visible(clock.remaining(Instant::now()) > 0),
                move |ui, _| {
                    if clock.remaining(Instant::now()) == 0 {
                        ui.ctx()
                            .send_viewport_cmd(egui::ViewportCommand::Visible(false));
                        ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
                        return;
                    }
                    if ui.input(|i| {
                        i.viewport().close_requested() || i.key_pressed(egui::Key::Escape)
                    }) {
                        captures_app::capture_flow::cancel(generation);
                        ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
                    }
                    let poses = countdown_entrance(
                        ui.ctx(),
                        &t,
                        ("portal-screenshot-countdown", generation),
                        reduced_motion,
                    );
                    crate::countdown::show(
                        ui,
                        &t,
                        clock.remaining(Instant::now()).max(1),
                        crate::countdown::Kind::Screenshot,
                        !captures_app::capture_flow::is_current(generation),
                        poses,
                    );
                    ui.ctx().request_repaint_after(Duration::from_millis(100));
                },
            );
            countdown_declared = true;
        }
        if let Some(flow) = &self.recording_screenshot_flow
            && let Some(
                RecordingScreenshotPhase::Countdown { .. }
                | RecordingScreenshotPhase::DisplayCountdown,
            ) = self.recording_screenshot_phase
            // The screenshot counts down on the display it captures.
            && let Some(target) = self.recording_screenshot_target()
        {
            let clock = flow.countdown();
            if clock.remaining(Instant::now()) > 0 {
                let t = t.clone();
                let generation = flow.generation();
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
                        let poses = countdown_entrance(
                            ui.ctx(),
                            &t,
                            ("recording-screenshot-countdown", generation),
                            reduced_motion,
                        );
                        crate::countdown::show(
                            ui,
                            &t,
                            clock.remaining(Instant::now()).max(1),
                            crate::countdown::Kind::Screenshot,
                            !captures_app::capture_flow::is_current(generation),
                            poses,
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
                let (title, kind) = main_countdown_presentation(self.capture_phase);
                ctx.show_viewport_deferred(
                    main_countdown_viewport(),
                    countdown_viewport(title, self.countdown_target, &t),
                    move |ui, _| {
                        if ui.input(|i| {
                            i.viewport().close_requested() || i.key_pressed(egui::Key::Escape)
                        }) {
                            captures_app::capture_flow::cancel(generation);
                            ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
                        }
                        let poses = countdown_entrance(
                            ui.ctx(),
                            &t,
                            ("main-countdown", generation),
                            reduced_motion,
                        );
                        crate::countdown::show(
                            ui,
                            &t,
                            clock.remaining(Instant::now()).max(1),
                            kind,
                            !captures_app::capture_flow::is_current(generation),
                            poses,
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
                    countdown_viewport(exit.title, exit.target, &t),
                    move |ui, _| {
                        // Shipping `.recording-countdown.exiting` fade over the linger.
                        let now = Instant::now();
                        let elapsed = crate::countdown::CANCEL_LINGER_MS as f64
                            - exit.until.saturating_duration_since(now).as_secs_f64() * 1000.;
                        let (poses, moving) =
                            crate::countdown::Poses::exit(&t, elapsed, reduced_motion);
                        if moving {
                            ui.ctx().request_repaint();
                        }
                        crate::countdown::show(ui, &t, exit.remaining, exit.kind, true, poses);
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
        let busy = self.pending > 0 || self.recovery.blocking() || self.card_restoring.is_some();
        let restored = self.restored_feedback(ui.ctx(), now);
        let margin = |name: &str| t.number(name) as i8;
        let mut header_event = None;
        let mut share_selected = false;
        egui::Panel::top("live-header")
            .show_separator_line(false)
            .frame(
                egui::Frame::new()
                    .fill(t.color("surface-canvas"))
                    .inner_margin(egui::Margin {
                        left: margin("s-8"),
                        right: margin("s-8"),
                        top: margin("s-8"),
                        // Shipping `.history-shell` gap before the filters.
                        bottom: margin("s-6"),
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
            if self.selection.id.is_some() {
                ui.add_space(t.number("s-5"));
                share_selected = ui.add_enabled(!busy, egui::Button::new("Share selected capture…")).clicked();
            }
            // Shipping History is only the header, filters and grid: captures
            // start from the tray, shortcuts and capture menu.
            #[cfg(target_os = "linux")]
            if ui.ctx().data(|data| data.get_temp::<bool>(egui::Id::unique("wayland-surface"))) == Some(true) {
                // Wayland may provide neither a tray host nor global shortcuts.
                // Keep portal capture reachable from the native window.
                ui.add_space(t.number("s-5"));
                let (button, record, record_window) = ui.horizontal(|ui| {
                    let button = ui.add_enabled_ui(self.can_launch_capture(), |ui| {
                        crate::preferences_widgets::button(ui, t, "Take screenshot…", true)
                    }).inner;
                    let record = ui.add_enabled_ui(self.can_launch_capture(), |ui| {
                        crate::preferences_widgets::button(ui, t, "Record display…", false)
                    }).inner;
                    let record_window = ui.add_enabled_ui(self.can_launch_capture(), |ui| {
                        crate::preferences_widgets::button(ui, t, "Record window…", false)
                    }).inner;
                    (button, record, record_window)
                }).inner;
                if std::env::var_os("CAPTURES_NATIVE_LAYOUT_PROBE").is_some() {
                    println!("{}", serde_json::json!({"event":"portal-screenshot-layout",
                        "detail":{"button":[button.rect.min.x, button.rect.min.y,
                            button.rect.max.x, button.rect.max.y], "enabled":button.enabled(),
                            "viewport_size":ui.input(|input| [input.content_rect().width(), input.content_rect().height()])}}));
                    println!("{}", serde_json::json!({"event":"portal-recording-layout",
                        "detail":{"button":[record.rect.min.x, record.rect.min.y,
                            record.rect.max.x, record.rect.max.y], "enabled":record.enabled(),
                            "viewport_size":ui.input(|input| [input.content_rect().width(), input.content_rect().height()])}}));
                    println!("{}", serde_json::json!({"event":"portal-window-recording-layout",
                        "detail":{"button":[record_window.rect.min.x, record_window.rect.min.y,
                            record_window.rect.max.x, record_window.rect.max.y], "enabled":record_window.enabled(),
                            "viewport_size":ui.input(|input| [input.content_rect().width(), input.content_rect().height()])}}));
                }
                if button.clicked() {
                    self.request_capture(CaptureRequest::Display);
                    ui.ctx().request_repaint();
                }
                if record.clicked() {
                    self.request_capture(CaptureRequest::Recording(capture_controls::TargetMode::Display));
                    ui.ctx().request_repaint();
                }
                if record_window.clicked() {
                    self.request_capture(CaptureRequest::Recording(capture_controls::TargetMode::Window));
                    ui.ctx().request_repaint();
                }
                let notice = ui.label(RichText::new("Desktop portal • Window recording needs portal support. Region capture, window screenshots and floating previews are unavailable.")
                    .size(t.number("text-sm")).color(t.color("text-subtle")));
                if std::env::var_os("CAPTURES_NATIVE_LAYOUT_PROBE").is_some() {
                    let clip = ui.clip_rect();
                    println!("{}", serde_json::json!({"event":"portal-limitation-layout",
                        "detail":{"rect":[notice.rect.min.x, notice.rect.min.y, notice.rect.max.x, notice.rect.max.y],
                            "clip":[clip.min.x, clip.min.y, clip.max.x, clip.max.y],
                            "viewport_size":ui.input(|input| [input.content_rect().width(), input.content_rect().height()])}}));
                }
            }
            if self.can_hide == Some(false) {
                ui.add_space(t.number("s-5"));
                ui.colored_label(t.color("theme-signal"), "Display, region and window capture unavailable: this Wayland backend cannot hide and verify the root window.");
            }
            // Panels paint inside their previous measured height. Wrapping on
            // resize needs a sizing pass before presenting the taller header.
            if ui.min_rect().bottom() > ui.clip_rect().bottom() {
                ui.ctx().request_discard("History header grew after wrapping");
            }
        });
        if share_selected && let Some(id) = self.selection.id.clone() {
            self.open_share(ui.ctx(), &id);
        }
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
                                    .map(|(_, action)| *action)
                                    .or_else(|| {
                                        self.card_restoring
                                            .as_ref()
                                            .filter(|guard| guard.artifact_id == id)
                                            .map(|_| {
                                                captures_app::history_view::CardAction::Restore
                                            })
                                    }),
                                done: (restored.as_deref() == Some(id))
                                    .then_some(captures_app::history_view::CardAction::Restore),
                                error: self.card_errors.get(id).map(String::as_str),
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
                        crate::history::Event::DragFile(slot) => {
                            let artifact_id = ids[slot].clone();
                            self.card_errors.remove(&artifact_id);
                            let completed = self.history_drag_tx.clone();
                            let wake = ui.ctx().clone();
                            crate::outbound_drag::request(
                                ui.ctx(),
                                self.root.clone(),
                                artifact_id.clone(),
                                Box::new(move |result| {
                                    let _ = completed.send(HistoryDragFinished {
                                        artifact_id,
                                        result,
                                    });
                                    wake.request_repaint();
                                }),
                            );
                        }
                        crate::history::Event::Open(slot) => {
                            self.select_card(&ids[slot]);
                            self.card_action(
                                ui.ctx(),
                                &ids[slot],
                                captures_app::history_view::CardAction::Edit,
                                settings(),
                                frame,
                            );
                        }
                        crate::history::Event::Action(slot, action) => {
                            self.card_action(ui.ctx(), &ids[slot], action, settings(), frame);
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
                        let layout = captures_app::history_view::grid_in_window(
                            output.width,
                            output.compact,
                        );
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
                            frame,
                        );
                    }
                }
            });
    }

    /// The card still showing "✓ Restored", waking once when it expires.
    fn restored_feedback(&mut self, ctx: &egui::Context, now: Instant) -> Option<String> {
        let duration = Duration::from_millis(captures_app::history_view::ACTION_FEEDBACK_MS);
        let (id, at) = self.card_restored.as_ref()?;
        let elapsed = now.saturating_duration_since(*at);
        if elapsed >= duration {
            self.card_restored = None;
            return None;
        }
        ctx.request_repaint_after(duration - elapsed);
        Some(id.clone())
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
        frame: &eframe::Frame,
    ) {
        use captures_app::history_view::CardAction;
        if self.pending > 0 || self.recovery.blocking() || self.card_restoring.is_some() {
            return;
        }
        let Some(entry) = self
            .artifact_index(id)
            .map(|index| self.artifacts[index].entry.clone())
        else {
            return;
        };
        let recording = entry.kind.is_recording();
        // Shipping clears a card's error when it starts another action.
        self.card_errors.remove(id);
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
                    // Shipping History Edit restores the floating preview
                    // (`restore_history_artifact`) before opening the editor;
                    // a failed restore opens nothing.
                    let target = capture_target(frame, &self.displays, self.display_id.as_deref());
                    if let Err(error) = self.restore_for_edit(ctx, id, &settings, target) {
                        self.card_errors.insert(id.to_owned(), error);
                        return;
                    }
                    let mode = entry.mode.unwrap_or(captures_capture::CaptureMode::Region);
                    self.open_screenshot_editor(
                        ctx,
                        id.to_owned(),
                        settings.output_directory.into(),
                        mode,
                    );
                }
                Err(error) if !recording => {
                    self.card_errors.insert(id.to_owned(), error);
                }
                Err(error) => self.error = Some(error),
            },
            CardAction::Restore => match settings {
                Ok(settings) => {
                    let target = capture_target(frame, &self.displays, self.display_id.as_deref());
                    self.restore(ctx, &entry.id, &settings, target);
                }
                Err(error) => {
                    self.card_errors.insert(id.to_owned(), error);
                }
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

    /// Shipping History Restore: reopen a screenshot through the mini-preview
    /// stack. An empty stack opens on the workspace's selected display.
    fn restore(
        &mut self,
        ctx: &egui::Context,
        id: &str,
        settings: &AppSettings,
        target: Option<CaptureTarget>,
    ) {
        let Some(index) = self.artifact_index(id) else {
            return;
        };
        let artifact = &self.artifacts[index];
        if artifact.entry.kind.is_recording() {
            return;
        }
        self.card_errors.remove(id);
        if ctx.data(|data| data.get_temp::<bool>(egui::Id::unique("wayland-surface"))) == Some(true)
        {
            self.card_errors.insert(
                id.to_owned(),
                "Floating previews are not available on Wayland. Use Edit instead.".into(),
            );
            return;
        }
        self.card_restored = None;
        let editor_open = self.editors.get(id).is_some_and(|editor| !editor.closed());
        match self
            .previews
            .restore_artifact(artifact, settings, target, editor_open)
        {
            Ok(RestoreStart::AlreadyShowing) => {
                self.card_restored = Some((id.to_owned(), Instant::now()));
            }
            Ok(RestoreStart::Decode(guard, path)) => {
                let _ = self.tx.send(Job::DecodePreview {
                    generation: guard.generation,
                    artifact_id: guard.artifact_id.clone(),
                    path,
                });
                self.card_restoring = Some(guard);
            }
            Err(error) => {
                self.card_errors.insert(id.to_owned(), error);
            }
        }
        request_hidden_root_paint(ctx);
        ctx.request_repaint();
    }

    /// History Edit's restore: the same stack insertion as Restore, without
    /// its busy state or "Restored" feedback. Succeeds when the editor may
    /// open (the card is showing, decoding, or already in the stack).
    fn restore_for_edit(
        &mut self,
        ctx: &egui::Context,
        id: &str,
        settings: &AppSettings,
        target: Option<CaptureTarget>,
    ) -> Result<(), String> {
        let Some(index) = self.artifact_index(id) else {
            return Err("The screenshot is no longer in Capture History.".into());
        };
        // Portal captures have no monitor geometry for a floating preview, but
        // opening their normal editor does not require that preview side effect.
        if ctx.data(|data| data.get_temp::<bool>(egui::Id::unique("wayland-surface"))) == Some(true)
        {
            return Ok(());
        }
        let artifact = &self.artifacts[index];
        let editor_open = self.editors.get(id).is_some_and(|editor| !editor.closed());
        match self
            .previews
            .restore_artifact(artifact, settings, target, editor_open)
        {
            Ok(RestoreStart::AlreadyShowing) => Ok(()),
            Ok(RestoreStart::Decode(guard, path)) => {
                let _ = self.tx.send(Job::DecodePreview {
                    generation: guard.generation,
                    artifact_id: guard.artifact_id,
                    path,
                });
                request_hidden_root_paint(ctx);
                ctx.request_repaint();
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    /// A preview decode finished; end the matching Restore. Returns whether
    /// it belonged to a card's Restore.
    fn finish_restore(&mut self, artifact_id: &str, generation: u64, shown: bool) -> bool {
        let restoring = self.card_restoring.as_ref().is_some_and(|guard| {
            guard.artifact_id == artifact_id && guard.generation == generation
        });
        if restoring {
            self.card_restoring = None;
            if shown {
                self.card_restored = Some((artifact_id.to_owned(), Instant::now()));
            }
        }
        restoring
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

/// Shipping looks the monitor up fresh for every capture
/// (`capture_display_at_point`). List the displays again when the list is
/// empty or lacks the display under the pointer, so a failed or stale first
/// list recovers without a relaunch.
fn resolve_capture_display(
    displays: &mut Vec<DisplayDescriptor>,
    current: Option<&str>,
    pointer: Option<&str>,
    list: impl FnOnce() -> Result<Vec<DisplayDescriptor>, String>,
) -> Result<String, String> {
    fn listed(displays: &[DisplayDescriptor], id: &str) -> bool {
        displays.iter().any(|display| display.id == id)
    }
    if displays.is_empty() || pointer.is_some_and(|id| !listed(displays, id)) {
        match list() {
            Ok(fresh) => *displays = fresh,
            Err(error) if displays.is_empty() => return Err(error),
            // A stale list still has usable displays.
            Err(_) => {}
        }
    }
    pointer
        .filter(|id| listed(displays, id))
        .or_else(|| current.filter(|id| listed(displays, id)))
        .map(str::to_owned)
        .or_else(|| {
            displays
                .iter()
                .find(|display| display.is_primary)
                .or(displays.first())
                .map(|display| display.id.clone())
        })
        .ok_or_else(|| "No display is available for capture.".to_owned())
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

fn countdown_viewport(
    title: &str,
    target: Option<CaptureTarget>,
    t: &Tokens,
) -> egui::ViewportBuilder {
    match target {
        Some(target) => capture_viewport(title, target.monitor, target.position, target.size),
        None => egui::ViewportBuilder::default()
            .with_title(title)
            .with_inner_size(egui::vec2(
                t.number("countdown-number-min") * 2. + t.number("s-9"),
                t.number("countdown-number-min") * 2.,
            ))
            .with_transparent(true)
            .with_decorations(false)
            .with_resizable(false)
            .with_has_shadow(false)
            .with_always_on_top()
            .with_taskbar(false),
    }
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

#[cfg(target_os = "linux")]
fn portal_recording_options(
    settings: &captures_settings::RecordingSettings,
    mode: capture_controls::TargetMode,
) -> Result<RecordingOptions, String> {
    let target = match mode {
        capture_controls::TargetMode::Display => RecordingTarget::PortalDisplay,
        capture_controls::TargetMode::Window => RecordingTarget::PortalWindow,
        capture_controls::TargetMode::Region => return Err("Region recording is unavailable through the desktop portal. Choose a display or window instead.".into()),
    };
    let options = RecordingOptions {
        kind: RecordingKind::Video,
        target,
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
    };
    options.validate().map_err(str::to_owned)?;
    Ok(options)
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

/// The shipping coordinator state a recording phase stands for, which decides
/// where Screenshot Display goes (`capture_error::display_route`).
/// A recaptured selector or menu reuses its viewport, which egui repaints
/// only on request: paint the new snapshot as soon as it is in place.
fn request_recaptured_viewports(ctx: &egui::Context, generation: u64, monitors: usize) {
    for id in [
        egui::ViewportId::from_hash_of("region-selector"),
        egui::ViewportId::from_hash_of("window-selector"),
        egui::ViewportId::from_hash_of(("recording-screenshot-selector", generation)),
        egui::ViewportId::from_hash_of(("recording-screenshot-window-selector", generation)),
    ]
    .into_iter()
    .chain((0..monitors.max(1)).map(capture_controls_viewport))
    {
        ctx.request_repaint_of(id);
    }
}

/// The capture menu's viewport on `monitor`. A display switch declares the
/// new display's menu as the old one closes, instead of moving one window.
fn capture_controls_viewport(monitor: usize) -> egui::ViewportId {
    egui::ViewportId::from_hash_of(("capture-controls", monitor))
}

/// What is open or in flight for `capture_error::busy_route`. `beside` is the
/// phase of a screenshot beside the take, when one is in progress; the menu
/// target is read only while the capture menu is open.
fn capture_activity(
    beside: Option<Option<RecordingScreenshotPhase>>,
    phase: Option<CapturePhase>,
    menu_target: impl FnOnce() -> Option<captures_app::capture_error::Target>,
    in_flight: bool,
) -> captures_app::capture_error::Activity {
    use captures_app::capture_error::Activity;
    if let Some(beside) = beside {
        return if beside == Some(RecordingScreenshotPhase::Selecting) {
            Activity::Selector
        } else {
            Activity::Busy
        };
    }
    if is_recording_phase(phase) {
        return Activity::Idle;
    }
    match phase {
        Some(CapturePhase::RegionSelecting | CapturePhase::WindowSelecting) => Activity::Selector,
        Some(CapturePhase::ControlsSelecting) => Activity::Menu {
            screenshot_target: menu_target(),
        },
        Some(_) => Activity::Busy,
        None if in_flight => Activity::Busy,
        None => Activity::Idle,
    }
}

fn recording_route_state(phase: Option<CapturePhase>, has_started: bool) -> Option<RecordingState> {
    Some(match phase? {
        CapturePhase::RecordingPreparing { .. } => RecordingState::Selecting,
        CapturePhase::RecordingCountdown | CapturePhase::RecordingRestarting => {
            RecordingState::Countdown
        }
        CapturePhase::RecordingStarting if !has_started => RecordingState::Countdown,
        CapturePhase::Recording
        | CapturePhase::RecordingPausing
        | CapturePhase::RecordingMuting { paused: false } => RecordingState::Recording,
        CapturePhase::RecordingStarting
        | CapturePhase::RecordingPaused
        | CapturePhase::RecordingMuting { paused: true } => RecordingState::Paused,
        CapturePhase::RecordingFinalizing | CapturePhase::RecordingDiscarding => {
            RecordingState::Finalizing
        }
        CapturePhase::RecordingFailed => RecordingState::Failed,
        _ => return None,
    })
}

/// The window selector's choice as a capture target, or `None` when the
/// chosen window left the prepared list.
fn window_capture_target(
    session: &WindowSession,
    target: SelectionTarget,
) -> Option<WindowCaptureTarget> {
    Some(match target {
        SelectionTarget::Display => WindowCaptureTarget::Display,
        SelectionTarget::Window(index) => WindowCaptureTarget::Window {
            id: session.windows().get(index)?.id.clone(),
        },
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
                // The History canvas renders the persisted poster and never
                // decodes media.mp4; the recording editor opens the media itself.
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn display(id: &str, is_primary: bool) -> DisplayDescriptor {
        DisplayDescriptor {
            id: id.into(),
            name: id.into(),
            x: 0,
            y: 0,
            width: 1280,
            height: 900,
            scale_factor: 1.,
            is_primary,
        }
    }

    #[test]
    fn an_empty_or_failed_display_list_is_listed_again_for_each_capture() {
        // A failed first list leaves nothing to capture on; shipping looks the
        // monitor up for every capture, so the next capture lists again.
        let mut displays = Vec::new();
        assert_eq!(
            resolve_capture_display(&mut displays, None, Some("a"), || Err(
                "monitor query failed".into()
            )),
            Err("monitor query failed".into())
        );
        assert!(displays.is_empty());
        let mut listed = 0;
        assert_eq!(
            resolve_capture_display(&mut displays, None, Some("b"), || {
                listed += 1;
                Ok(vec![display("a", true), display("b", false)])
            }),
            Ok("b".into()),
            "the recovered list resolves the display under the pointer"
        );
        assert_eq!(listed, 1);
        assert_eq!(displays.len(), 2);
        // Wayland reports no pointer: an empty list still recovers, and the
        // primary display is used.
        let mut displays = Vec::new();
        assert_eq!(
            resolve_capture_display(&mut displays, None, None, || Ok(vec![
                display("a", false),
                display("b", true)
            ])),
            Ok("b".into())
        );
        let mut displays = Vec::new();
        assert_eq!(
            resolve_capture_display(&mut displays, None, None, || Ok(vec![])),
            Err("No display is available for capture.".into())
        );
    }

    #[test]
    fn a_stale_display_list_is_refreshed_only_when_the_pointer_display_is_missing() {
        let mut displays = vec![display("a", true)];
        assert_eq!(
            resolve_capture_display(&mut displays, Some("a"), Some("a"), || {
                panic!("a current list is not listed again")
            }),
            Ok("a".into())
        );
        assert_eq!(
            resolve_capture_display(&mut displays, Some("a"), Some("c"), || Ok(vec![
                display("a", true),
                display("c", false)
            ])),
            Ok("c".into()),
            "a newly attached display under the pointer is listed"
        );
        // A failed refresh of a stale list keeps the displays it has.
        assert_eq!(
            resolve_capture_display(&mut displays, Some("c"), Some("d"), || Err("busy".into())),
            Ok("c".into())
        );
        assert_eq!(displays.len(), 2);
    }

    #[test]
    fn screenshot_display_opens_the_capture_menu_unless_a_take_is_running() {
        let root = tempfile::tempdir().unwrap();
        let mut live = Live::new(egui::Context::default(), Some(root.path().into()));
        assert_eq!(live.display_request(), Some(CaptureRequest::DisplayMenu));
        for (phase, expected) in [
            (CapturePhase::Recording, Some(CaptureRequest::Display)),
            (CapturePhase::RecordingPaused, Some(CaptureRequest::Display)),
            (
                CapturePhase::RecordingPausing,
                Some(CaptureRequest::Display),
            ),
            // Shipping `screenshot_capture_is_blocked`: refused silently.
            (
                CapturePhase::RecordingPreparing {
                    target: Some(capture_controls::Target::Display),
                },
                None,
            ),
            (CapturePhase::RecordingCountdown, None),
            (CapturePhase::RecordingRestarting, None),
            (CapturePhase::RecordingFinalizing, None),
            (
                CapturePhase::RecordingFailed,
                Some(CaptureRequest::DisplayMenu),
            ),
            (
                CapturePhase::ControlsSelecting,
                Some(CaptureRequest::DisplayMenu),
            ),
        ] {
            live.capture_phase = Some(phase);
            assert_eq!(live.display_request(), expected, "{phase:?}");
        }
        // A resume counts as paused; a first start is still the countdown.
        live.capture_phase = Some(CapturePhase::RecordingStarting);
        assert_eq!(live.display_request(), None);
        live.recording_has_started = true;
        assert_eq!(live.display_request(), Some(CaptureRequest::Display));
        live.recording_has_started = false;
        live.capture_phase = None;
        live.flush();
    }

    #[test]
    fn region_and_window_screenshots_go_beside_a_running_take() {
        let root = tempfile::tempdir().unwrap();
        let mut live = Live::new(egui::Context::default(), Some(root.path().into()));
        for request in [CaptureRequest::Region, CaptureRequest::Window] {
            for phase in [CapturePhase::Recording, CapturePhase::RecordingPaused] {
                live.capture_phase = Some(phase);
                live.requested_capture = None;
                live.request_capture(request);
                assert_eq!(
                    live.requested_capture,
                    Some(request),
                    "{request:?} {phase:?}"
                );
                // One screenshot at a time; a second recording or New Capture
                // cannot queue behind it.
                live.request_capture(CaptureRequest::Display);
                live.request_capture(CaptureRequest::Recording(
                    capture_controls::TargetMode::Region,
                ));
                assert_eq!(live.requested_capture, Some(request));
            }
            // Shipping `screenshot_capture_is_blocked`: refused silently.
            for phase in [
                CapturePhase::RecordingPreparing {
                    target: Some(capture_controls::Target::Display),
                },
                CapturePhase::RecordingCountdown,
                CapturePhase::RecordingPausing,
                CapturePhase::RecordingFinalizing,
            ] {
                live.requested_capture = None;
                live.capture_phase = Some(phase);
                live.request_capture(request);
                assert_eq!(live.requested_capture, None, "{request:?} {phase:?}");
                assert!(live.take_capture_failure().is_none(), "refused silently");
            }
        }
        live.requested_capture = None;
        live.capture_phase = None;
        live.flush();
    }

    #[test]
    fn new_capture_beside_visible_recording_controls_reports_the_busy_take() {
        let root = tempfile::tempdir().unwrap();
        let mut live = Live::new(egui::Context::default(), Some(root.path().into()));
        for phase in [
            CapturePhase::Recording,
            CapturePhase::RecordingPaused,
            CapturePhase::RecordingCountdown,
            CapturePhase::RecordingFinalizing,
        ] {
            live.capture_phase = Some(phase);
            live.request_capture(CaptureRequest::NewCapture);
            assert_eq!(live.requested_capture, None, "{phase:?}");
            let failure = live.take_capture_failure().expect("shipping error dialog");
            assert_eq!(
                failure.report(),
                FailureReport::Dialog {
                    title: captures_app::capture_error::TITLE,
                    message: "Captures could not start the capture: capture already in progress"
                        .into(),
                },
                "{phase:?}"
            );
        }
        // Record shortcuts and tray items refuse a second recording silently
        // while the take owns the capture flow.
        live.capture_phase = Some(CapturePhase::Recording);
        live.capture_in_flight = true;
        for target in [
            capture_controls::TargetMode::Region,
            capture_controls::TargetMode::Window,
            capture_controls::TargetMode::Display,
        ] {
            live.request_capture(CaptureRequest::Recording(target));
            assert_eq!(live.requested_capture, None);
            assert!(live.take_capture_failure().is_none());
        }
        live.capture_in_flight = false;
        live.capture_phase = None;
        live.flush();
    }

    #[test]
    fn capture_actions_while_a_capture_is_open_follow_the_shipping_busy_route() {
        use captures_app::capture_error::{Action, Target};
        let root = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut live = Live::new(ctx.clone(), Some(root.path().into()));
        let busy = || FailureReport::Dialog {
            title: captures_app::capture_error::TITLE,
            message: "Captures could not start the capture: capture already in progress".into(),
        };
        // A countdown or a screenshot without its selector: New Capture and
        // Screenshot Display report the busy capture, the rest do nothing.
        for phase in [
            CapturePhase::RegionCountdown {
                rect: SelectionRect {
                    x: 0.,
                    y: 0.,
                    width: 10.,
                    height: 10.,
                },
                after_countdown: true,
            },
            CapturePhase::DisplayCountdown,
            CapturePhase::RegionPreparing,
            CapturePhase::WindowCapturing,
        ] {
            live.capture_phase = Some(phase);
            for action in [Action::NewCapture, Action::Screenshot(Target::Display)] {
                live.capture_action(action, &ctx);
                assert_eq!(
                    live.take_capture_failure().map(|failure| failure.report()),
                    Some(busy()),
                    "{phase:?} {action:?}"
                );
            }
            for action in [
                Action::Screenshot(Target::Region),
                Action::Screenshot(Target::Window),
                Action::Record(Target::Region),
            ] {
                live.capture_action(action, &ctx);
                assert!(
                    live.take_capture_failure().is_none(),
                    "{phase:?} {action:?}"
                );
                assert_eq!(live.requested_capture, None);
                assert_eq!(live.requested_recapture, None);
            }
        }
        // An open selector is recaptured by every action.
        live.capture_phase = Some(CapturePhase::RegionSelecting);
        for (action, expected) in [
            (
                Action::Screenshot(Target::Region),
                Recapture::Selector(RecordingSelectorKind::Region),
            ),
            (
                Action::Screenshot(Target::Window),
                Recapture::Selector(RecordingSelectorKind::Window),
            ),
            (
                Action::NewCapture,
                Recapture::Menu {
                    record: false,
                    target: capture_controls::TargetMode::Region,
                },
            ),
            (
                Action::Screenshot(Target::Display),
                Recapture::Menu {
                    record: false,
                    target: capture_controls::TargetMode::Display,
                },
            ),
            (
                Action::Record(Target::Window),
                Recapture::Menu {
                    record: true,
                    target: capture_controls::TargetMode::Window,
                },
            ),
        ] {
            live.capture_action(action, &ctx);
            assert_eq!(live.requested_recapture, Some(expected), "{action:?}");
            // One recapture at a time: later actions wait for it.
            live.capture_action(Action::NewCapture, &ctx);
            assert_eq!(live.requested_recapture, Some(expected));
            assert!(live.take_capture_failure().is_none());
            live.requested_recapture = None;
        }
        // The open capture menu switches in place, or recaptures its own target.
        live.capture_phase = Some(CapturePhase::ControlsSelecting);
        live.capture_action(Action::Screenshot(Target::Window), &ctx);
        assert_eq!(live.requested_recapture, None);
        assert_eq!(
            live.controls.lock().unwrap().mode(),
            capture_controls::TargetMode::Window
        );
        live.capture_action(Action::Screenshot(Target::Window), &ctx);
        assert_eq!(
            live.requested_recapture,
            Some(Recapture::Selector(RecordingSelectorKind::Window))
        );
        live.requested_recapture = None;
        live.capture_action(Action::NewCapture, &ctx);
        assert_eq!(live.requested_recapture, None);
        assert_eq!(live.menu_screenshot_target, Some(Target::Region));
        // Like shipping's selection summary, a target picked inside the menu
        // (here Full screen) is not the one its shortcuts compare against.
        live.controls
            .lock()
            .unwrap()
            .apply_target_shortcut(CaptureShortcut::Display);
        live.capture_action(Action::Screenshot(Target::Display), &ctx);
        assert_eq!(live.requested_recapture, None);
        assert_eq!(live.menu_screenshot_target, Some(Target::Display));
        live.controls
            .lock()
            .unwrap()
            .apply_target_shortcut(CaptureShortcut::Region);
        live.capture_action(Action::NewCapture, &ctx);
        assert_eq!(live.requested_recapture, None);
        assert_eq!(live.menu_screenshot_target, Some(Target::Region));
        live.capture_action(Action::NewCapture, &ctx);
        assert_eq!(
            live.requested_recapture,
            Some(Recapture::Menu {
                record: false,
                target: capture_controls::TargetMode::Region
            })
        );
        live.requested_recapture = None;
        live.capture_action(Action::Record(Target::Display), &ctx);
        assert_eq!(live.menu_screenshot_target, None);
        assert_eq!(live.requested_recapture, None);
        live.capture_phase = None;
        live.flush();
    }

    #[test]
    fn the_busy_activity_follows_the_open_capture_ui() {
        use captures_app::capture_error::{Activity, Target};
        let menu = || Some(Target::Window);
        // A screenshot beside the take: its selector, or busy otherwise.
        for phase in [
            Some(CapturePhase::Recording),
            Some(CapturePhase::RecordingPaused),
        ] {
            assert_eq!(
                capture_activity(
                    Some(Some(RecordingScreenshotPhase::Selecting)),
                    phase,
                    menu,
                    true
                ),
                Activity::Selector
            );
            for beside in [
                None,
                Some(RecordingScreenshotPhase::WaitingForHud),
                Some(RecordingScreenshotPhase::Preparing),
                Some(RecordingScreenshotPhase::DisplayCapturing),
            ] {
                assert_eq!(
                    capture_activity(Some(beside), phase, menu, true),
                    Activity::Busy,
                    "{beside:?}"
                );
            }
            // The take alone leaves the recording routes to decide.
            assert_eq!(capture_activity(None, phase, menu, true), Activity::Idle);
        }
        for phase in [CapturePhase::RegionSelecting, CapturePhase::WindowSelecting] {
            assert_eq!(
                capture_activity(None, Some(phase), menu, true),
                Activity::Selector
            );
        }
        assert_eq!(
            capture_activity(None, Some(CapturePhase::ControlsSelecting), menu, true),
            Activity::Menu {
                screenshot_target: Some(Target::Window)
            }
        );
        for phase in [
            CapturePhase::ControlsPreparing,
            CapturePhase::RegionPreparing,
            CapturePhase::DisplayCountdown,
        ] {
            assert_eq!(
                capture_activity(None, Some(phase), menu, true),
                Activity::Busy
            );
        }
        assert_eq!(capture_activity(None, None, menu, true), Activity::Busy);
        assert_eq!(capture_activity(None, None, menu, false), Activity::Idle);
    }

    #[test]
    fn selector_screenshots_count_down_where_they_open_and_honor_controls() {
        let root = tempfile::tempdir().unwrap();
        let mut live = Live::new(egui::Context::default(), Some(root.path().into()));
        let target = |x| CaptureTarget {
            monitor: 0,
            position: egui::pos2(x, 0.),
            size: egui::vec2(1280., 900.),
            preview_bounds: None,
        };
        // The recording's display stays the region guide's and HUD's.
        live.countdown_target = Some(target(0.));
        live.recording_selector_shot = Some(RecordingSelectorShot {
            kind: RecordingSelectorKind::Window,
            display_id: "pointer".into(),
            target: target(1280.),
            includes_capture_ui: false,
        });
        assert_eq!(
            live.recording_screenshot_target()
                .map(|target| target.position.x),
            Some(1280.)
        );
        assert!(!live.recording_screenshot_keeps_controls());
        live.recording_screenshot_settings = Some(AppSettings {
            include_recording_controls_in_captures: true,
            ..AppSettings::default()
        });
        assert!(live.recording_screenshot_keeps_controls());
        live.recording_selector_shot = None;
        assert!(live.recording_screenshot_target().is_none());
        assert!(
            !live.recording_screenshot_keeps_controls(),
            "only a screenshot in progress keeps them"
        );
        live.flush();
    }

    #[test]
    fn a_screenshot_while_recording_never_becomes_a_second_capture() {
        let root = tempfile::tempdir().unwrap();
        let mut live = Live::new(egui::Context::default(), Some(root.path().into()));
        live.capture_phase = Some(CapturePhase::Recording);
        assert!(live.recording_screenshot_available());
        live.request_capture(CaptureRequest::Display);
        assert_eq!(live.requested_capture, Some(CaptureRequest::Display));
        assert!(
            !live.recording_screenshot_available(),
            "one display screenshot at a time"
        );
        live.request_capture(CaptureRequest::Display);
        live.request_capture(CaptureRequest::Recording(
            capture_controls::TargetMode::Display,
        ));
        live.request_capture(CaptureRequest::DisplayMenu);
        assert_eq!(
            live.requested_capture,
            Some(CaptureRequest::Display),
            "a second recording, menu or screenshot is ignored, not queued"
        );
        live.requested_capture = None;

        // Paused takes accept one too; a countdown or finalizing take does not.
        live.capture_phase = Some(CapturePhase::RecordingPaused);
        live.request_capture(CaptureRequest::Display);
        assert_eq!(live.requested_capture, Some(CaptureRequest::Display));
        for phase in [
            CapturePhase::RecordingCountdown,
            CapturePhase::RecordingFinalizing,
            CapturePhase::RecordingPausing,
        ] {
            live.requested_capture = None;
            live.capture_phase = Some(phase);
            live.request_capture(CaptureRequest::Display);
            assert_eq!(live.requested_capture, None, "{phase:?}");
            assert!(live.take_capture_failure().is_none(), "refused silently");
        }
        live.capture_phase = None;
        live.flush();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn wayland_hud_screenshot_is_a_portal_action_and_child_cleanup_keeps_the_take() {
        let root = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        ctx.set_embed_viewports(false);
        ctx.data_mut(|data| data.insert_temp(egui::Id::unique("wayland-surface"), true));
        let mut live = Live::new(ctx.clone(), Some(root.path().into()));
        live.capture_phase = Some(CapturePhase::RecordingPaused);
        live.workspace_hidden = true;
        live.restore_root_visible = true;
        live.start_recording_screenshot(&ctx);
        assert_eq!(live.requested_capture.take(), Some(CaptureRequest::Display));
        assert_eq!(live.capture_phase, Some(CapturePhase::RecordingPaused));
        for captured in [false, true] {
            live.portal_screenshot = Some(PortalScreenshot {
                flow: captures_app::capture_flow::PortalCapture::begin().unwrap(),
                hidden: vec![egui::ViewportId::from_hash_of("recording-controls")],
                restore_root: false,
                recording: true,
                started: Instant::now(),
                submitted: true,
                countdown: None,
                countdown_hidden_frame: None,
            });
            let now = Instant::now();
            let portal = live.portal_screenshot.as_mut().unwrap();
            assert!(portal.countdown_hidden(now, 7), "zero delay is unchanged");
            portal.countdown = Some(captures_app::capture_flow::Countdown::new(now, 3));
            assert!(!portal.countdown_hidden(now + Duration::from_millis(2999), 7));
            assert!(portal.countdown_hidden_frame.is_none());
            let deadline = now + Duration::from_secs(3);
            assert!(!portal.countdown_hidden(deadline, 7));
            assert_eq!(portal.started, deadline);
            assert!(!portal.countdown_hidden(deadline, 7), "same UI pass");
            assert!(
                portal.countdown_hidden(deadline, 8),
                "hidden callback applied"
            );
            portal.countdown = Some(captures_app::capture_flow::Countdown::new(now, 0));
            let countdown = recording_screenshot_countdown_viewport(portal.flow.generation());
            ctx.begin_pass(Default::default());
            live.capture_viewports(&ctx, &crate::tokens::load()["dark-mustard"], false);
            let mut output = ctx.end_pass();
            let visible = output
                .viewport_output
                .get(&countdown)
                .map(|viewport| viewport.builder.visible);
            output.textures_delta.clear();
            assert_eq!(
                visible,
                Some(Some(false)),
                "expired viewport retained hidden"
            );
            assert!(!live.recording_screenshot_available());
            live.capture_action(captures_app::capture_error::Action::NewCapture, &ctx);
            assert!(
                live.take_capture_failure().is_none(),
                "no dialog during portal capture"
            );
            assert!(!live.show_recording_controls(&ctx));
            live.request_capture(CaptureRequest::Display);
            assert!(live.requested_capture.is_none(), "no queued second child");
            live.auto_copy_on_capture = true;
            live.finish_portal_screenshot(&ctx, captured);
            assert!(live.portal_screenshot.is_none());
            assert_eq!(live.capture_phase, Some(CapturePhase::RecordingPaused));
            assert!(
                live.workspace_hidden,
                "completion cannot expose the workspace"
            );
            assert!(
                live.restore_root_visible,
                "parent restore policy is retained"
            );
            assert_eq!(live.auto_copy_on_capture, captured);
        }
        live.portal_screenshot = Some(PortalScreenshot {
            flow: captures_app::capture_flow::PortalCapture::begin().unwrap(),
            hidden: Vec::new(),
            restore_root: false,
            recording: true,
            started: Instant::now(),
            submitted: true,
            countdown: None,
            countdown_hidden_frame: None,
        });
        live.finish_capture(&ctx, false);
        assert!(
            live.portal_screenshot.is_none(),
            "source loss retires a pending child"
        );
        live.flush();
    }

    #[test]
    fn display_screenshot_countdown_waits_for_its_window_to_retire() {
        let now = Instant::now();
        let mut phase = RecordingScreenshotPhase::DisplayRetiring { omitted_frame: 7 };
        assert!(!phase.display_capture_after_hide(7, now));
        assert_eq!(
            phase,
            RecordingScreenshotPhase::DisplayRetiring { omitted_frame: 7 }
        );
        assert!(!phase.display_capture_after_hide(8, now));
        assert!(!phase.display_capture_after_hide(9, now + Duration::from_millis(149)));
        assert!(phase.display_capture_after_hide(9, now + Duration::from_millis(150)));
        assert_eq!(phase, RecordingScreenshotPhase::DisplayCapturing);
        assert!(!phase.display_capture_after_hide(10, now + Duration::from_secs(1)));
        let shot = |keeps_controls| RecordingDisplayShot {
            display_id: "a".into(),
            target: None,
            countdown_seconds: 0,
            include_cursor: false,
            keeps_controls,
        };
        assert_eq!(shot(false).controls_settle(), Duration::from_millis(300));
        assert_eq!(shot(true).controls_settle(), Duration::ZERO);
    }

    #[test]
    fn capture_failures_skip_the_history_error_card() {
        let root = tempfile::tempdir().unwrap();
        let mut live = Live::new(egui::Context::default(), Some(root.path().into()));
        live.capture_failed("The selected window changed before capture.".into());
        assert!(
            live.error.is_none(),
            "History shows only load and delete errors"
        );
        assert_eq!(
            live.take_capture_failure(),
            Some(CaptureFailure {
                error: "The selected window changed before capture.".into(),
                recording: false,
            })
        );
        assert_eq!(live.take_capture_failure(), None, "reported once");
        // A capture that arrives while another owns the flow is ignored, like
        // shipping's `CaptureInProgress`.
        live.capture_in_flight = true;
        live.request_capture(CaptureRequest::DisplayMenu);
        assert_eq!(live.requested_capture, None);
        assert_eq!(live.take_capture_failure(), None);
        assert!(live.error.is_none());
        live.capture_in_flight = false;
        live.flush();
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
        // Startup History also lists interrupted recordings on the recovery
        // worker, which wakes ROOT on its own schedule. A late wake between the
        // paints below and the callback leaves ROOT's repaint outstanding, and
        // egui then skips the callback for the History wake under test.
        let deadline = Instant::now() + Duration::from_secs(5);
        while live.recovery.blocking() {
            assert!(Instant::now() < deadline, "recovery listing never finished");
            live.recovery.receive();
            thread::sleep(Duration::from_millis(5));
        }
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
        let choices = [
            RecordingShotChoice::Region(rect),
            RecordingShotChoice::Window(SelectionTarget::Window(1)),
            RecordingShotChoice::Window(SelectionTarget::Display),
        ];
        for (after_countdown, choice) in [false, true]
            .into_iter()
            .flat_map(|after| choices.map(|choice| (after, choice)))
        {
            let mut phase = RecordingScreenshotPhase::RetiringCaptureUi {
                choice,
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
                Some((choice, after_countdown)),
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
    fn failed_menu_starts_and_switches_keep_the_menu_open_with_an_inline_error() {
        let root = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut live = Live::new(ctx.clone(), Some(root.path().into()));
        let display = |id: &str, x: i32| DisplayDescriptor {
            id: id.into(),
            name: id.into(),
            x,
            y: 0,
            width: 1280,
            height: 720,
            scale_factor: 1.,
            is_primary: x == 0,
        };
        let target = |monitor| CaptureTarget {
            monitor,
            position: egui::pos2(monitor as f32 * 1280., 0.),
            size: egui::vec2(1280., 720.),
            preview_bounds: None,
        };
        live.displays = vec![display("left", 0), display("right", 1280)];

        // Shipping `start_recording` restores the selection on a failed
        // draft and the menu shows the error: no dialog, no History card.
        live.display_id = Some("left".into());
        live.countdown_target = Some(target(0));
        live.capture_phase = Some(CapturePhase::RecordingPreparing {
            target: Some(capture_controls::Target::Display),
        });
        live.controls
            .lock()
            .unwrap()
            .begin_in_flight(capture_controls::InFlight::Starting);
        live.keep_controls_open_with_error(&ctx, "Could not create the draft.".into());
        assert_eq!(live.capture_phase, Some(CapturePhase::ControlsSelecting));
        assert_eq!(
            live.controls_error.as_deref(),
            Some("Could not create the draft.")
        );
        assert_eq!(live.controls.lock().unwrap().in_flight(), None);
        assert_eq!(live.take_capture_failure(), None);
        assert!(live.error.is_none());
        assert_eq!(live.display_id.as_deref(), Some("left"));

        // A failed switch returns the menu to the display it was on, with
        // its session and snapshot, like `select_capture_display`.
        live.controls_error = None;
        live.display_id = Some("right".into());
        live.countdown_target = Some(target(1));
        live.previews.capture_target = Some(target(1));
        live.window_session = None;
        live.controls_switching_display = true;
        live.controls_switch_from = Some((
            target(0),
            Arc::new(WindowSession::fixture(display("left", 0))),
            None,
        ));
        live.capture_phase = Some(CapturePhase::ControlsPreparing);
        live.controls
            .lock()
            .unwrap()
            .begin_in_flight(capture_controls::InFlight::Switching);
        assert!(live.fail_controls_switch(&ctx, "Could not capture the display.".into()));
        assert_eq!(live.capture_phase, Some(CapturePhase::ControlsSelecting));
        assert_eq!(live.display_id.as_deref(), Some("left"));
        assert_eq!(live.countdown_target.map(|target| target.monitor), Some(0));
        assert_eq!(
            live.previews.capture_target.map(|target| target.monitor),
            Some(0)
        );
        assert_eq!(
            live.window_session
                .as_ref()
                .map(|session| session.display().id.as_str()),
            Some("left")
        );
        assert!(!live.controls_switching_display);
        assert!(live.controls_switch_from.is_none());
        assert_eq!(live.controls.lock().unwrap().in_flight(), None);
        assert_eq!(
            live.controls_error.as_deref(),
            Some("Could not capture the display.")
        );
        assert_eq!(live.take_capture_failure(), None);

        // Without a switch in flight the caller ends the capture as before.
        assert!(!live.fail_controls_switch(&ctx, "Later failure".into()));
        assert_eq!(
            live.controls_error.as_deref(),
            Some("Could not capture the display.")
        );
        live.capture_phase = None;
        live.window_session = None;
        live.flush();
    }

    #[test]
    fn cancelling_screenshot_hide_wait_preserves_paused_recording() {
        let root = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut live = Live::new(ctx.clone(), Some(root.path().into()));
        live.capture_phase = Some(CapturePhase::RecordingPaused);
        live.recording_screenshot_phase = Some(RecordingScreenshotPhase::SettlingCaptureUi {
            choice: RecordingShotChoice::Region(SelectionRect {
                x: 140.,
                y: 180.,
                width: 310.,
                height: 170.,
            }),
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
            // Shipping dissolves the card before its Trash request.
            assert!(!live.previews.cards.contains_key(&id));
            assert!(live.previews.stack.ids().is_empty());
            if attempt == 1 {
                // Same ID, different presentation: a late reply cannot finish it.
                live.previews.trashing.get_mut(&id).unwrap().card.generation += 1;
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
                let pending = live.previews.trashing.remove(&guard.artifact_id).unwrap();
                assert!(
                    live.previews
                        .stack
                        .restore(guard.artifact_id.clone(), pending.index)
                );
                live.previews
                    .cards
                    .insert(guard.artifact_id.clone(), pending.card);
            } else {
                assert!(live.previews.cards.is_empty());
                assert!(live.previews.trashing.is_empty());
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
    fn preview_share_rejects_stale_or_blocked_actions_and_pins_its_original_identity() {
        struct SignedOut;
        impl captures_account::Vault for SignedOut {
            fn load(&self) -> Result<Option<String>, captures_account::VaultError> {
                Ok(None)
            }
            fn save(&self, _: &str) -> Result<(), captures_account::VaultError> {
                unreachable!()
            }
            fn delete(&self) -> Result<(), captures_account::VaultError> {
                Ok(())
            }
        }
        let root = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut frame = eframe::Frame::_new_kittest();
        let mut live = Live::new(ctx.clone(), Some(root.path().into()));
        live.flush();
        ctx.begin_pass(Default::default());
        live.logic(&ctx, &mut frame);
        let artifact = preview_artifact(root.path(), [73, 15, 201, 255]);
        let other = preview_artifact(root.path(), [14, 118, 52, 255]);
        let original = artifact.image_path.clone();
        let id = artifact.entry.id.clone();
        let other_id = other.entry.id.clone();
        live.previews
            .begin_capture(&AppSettings::default(), Some(preview_target()), 1)
            .unwrap();
        let (guard, _) = live.previews.start_artifact(&artifact).unwrap().unwrap();
        live.artifacts = vec![artifact, other];
        live.selection.begin(other_id.clone());
        live.sharing =
            crate::sharing::Window::with_worker(captures_account::native::Worker::with_client(
                root.path().into(),
                captures_account::AccountClient::new("http://127.0.0.1:9", SignedOut).unwrap(),
                Arc::new(|| {}),
            ));
        for blocked in ["stale", "hidden", "busy", "pending", "permission"] {
            live.workspace_hidden = blocked == "hidden";
            live.pending = usize::from(blocked == "pending");
            live.permission_recovery_visible = blocked == "permission";
            live.previews.cards.get_mut(&id).unwrap().busy =
                (blocked == "busy").then_some(crate::mini_preview::Busy::Save);
            live.preview_tx
                .send(PreviewMessage::Share {
                    artifact_id: id.clone(),
                    generation: guard.generation + u64::from(blocked == "stale"),
                })
                .unwrap();
            live.logic(&ctx, &mut frame);
            assert!(
                live.sharing.selection().is_none(),
                "{blocked} must not open Share"
            );
        }
        live.workspace_hidden = false;
        live.pending = 0;
        live.permission_recovery_visible = false;
        live.previews.cards.get_mut(&id).unwrap().busy = None;
        live.preview_tx
            .send(PreviewMessage::Share {
                artifact_id: id.clone(),
                generation: guard.generation,
            })
            .unwrap();
        live.logic(&ctx, &mut frame);
        let selected = live.sharing.selection().unwrap();
        assert_eq!(selected.artifact_id, id);
        assert_eq!(selected.path, original);
        assert_eq!(selected.content_type, "image/png");
        assert_eq!(
            live.selection.id.as_ref(),
            Some(&other_id),
            "sharing must not retarget History"
        );
        assert!(live.previews.dismiss(&id, guard.generation));
        assert_eq!(
            live.sharing.selection().unwrap().path,
            original,
            "preview dismissal does not own accepted sharing"
        );
        ctx.end_pass().textures_delta.clear();
        live.flush();
    }

    #[test]
    fn history_sharing_uses_recording_media_not_the_poster_or_export() {
        struct SignedOut;
        impl captures_account::Vault for SignedOut {
            fn load(&self) -> Result<Option<String>, captures_account::VaultError> {
                Ok(None)
            }
            fn save(&self, _: &str) -> Result<(), captures_account::VaultError> {
                unreachable!()
            }
            fn delete(&self) -> Result<(), captures_account::VaultError> {
                Ok(())
            }
        }
        let root = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut live = Live::new(ctx.clone(), Some(root.path().into()));
        live.flush();
        let mut artifact = preview_artifact(root.path(), [16, 122, 52, 255]);
        let id = artifact.entry.id.clone();
        artifact.entry.kind = captures_history::ArtifactKind::Video;
        artifact.entry.saved_path = Some(root.path().join("export.gif").to_string_lossy().into());
        let original = root.path().join(&id).join("media.webm");
        fs::write(&original, b"original recording bytes").unwrap();
        live.artifacts = vec![artifact];
        live.sharing =
            crate::sharing::Window::with_worker(captures_account::native::Worker::with_client(
                root.path().into(),
                captures_account::AccountClient::new("http://127.0.0.1:9", SignedOut).unwrap(),
                Arc::new(|| {}),
            ));
        live.open_share(&ctx, &id);
        let selected = live.sharing.selection().unwrap();
        assert_eq!(selected.path, original);
        assert_eq!(selected.content_type, "video/webm");
        assert_eq!(selected.name, format!("Capture-{id}.webm"));
        assert!(
            live.previews.cards.is_empty(),
            "History is usable without a preview"
        );
        live.flush();
    }

    #[test]
    fn hidden_root_bootstrap_requests_one_ui_pass_without_showing_root() {
        for wayland in [false, true] {
            let ctx = egui::Context::default();
            ctx.data_mut(|data| data.insert_temp(egui::Id::unique("wayland-surface"), wayland));
            let received = crate::root_repaint::observe_from_child(&ctx);
            request_hidden_root_ui(&ctx);
            assert_eq!(received.try_recv().unwrap(), egui::ViewportId::ROOT);
            let mut first = ctx.end_pass();
            assert_eq!(
                first.viewport_output[&egui::ViewportId::ROOT].commands,
                [egui::ViewportCommand::RequestPaintWhileHidden]
            );
            first.textures_delta.clear();

            let next = ctx.run_logic(&egui::RawInput::default(), |_| {});
            assert!(next.viewport_commands.is_empty());
        }
    }

    #[test]
    fn hidden_wayland_root_wakes_from_a_child_without_presenting_a_buffer() {
        let ctx = egui::Context::default();
        ctx.data_mut(|data| data.insert_temp(egui::Id::unique("wayland-surface"), true));
        let received = crate::root_repaint::observe_from_child(&ctx);
        request_hidden_root_paint(&ctx);
        assert_eq!(received.try_recv().unwrap(), egui::ViewportId::ROOT);
        let mut output = ctx.end_pass();
        assert!(
            output.viewport_output[&egui::ViewportId::ROOT]
                .commands
                .is_empty()
        );
        output.textures_delta.clear();
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

    #[cfg(target_os = "linux")]
    #[test]
    fn portal_recording_preserves_preferences_without_inventing_a_display() {
        let mut settings = captures_settings::RecordingSettings {
            video_fps: 15,
            video_max_resolution: captures_recording::MaxResolution::P1080,
            countdown_seconds: 5,
            show_cursor: true,
            capture_system_audio: false,
            microphone_device_id: Some("chosen microphone".into()),
            mono_audio: true,
            ..Default::default()
        };
        let options =
            portal_recording_options(&settings, capture_controls::TargetMode::Display).unwrap();
        assert_eq!(options.kind, RecordingKind::Video);
        assert_eq!(options.frames_per_second, 15);
        assert_eq!(
            options.max_resolution,
            captures_recording::MaxResolution::P1080
        );
        assert_eq!(options.countdown_seconds, 5);
        assert!(options.show_cursor && options.audio.mono_output);
        assert!(!options.audio.capture_system_audio);
        assert_eq!(
            options.audio.microphone_device_id.as_deref(),
            Some("chosen microphone")
        );
        assert_eq!(
            serde_json::to_value(options.target).unwrap(),
            serde_json::json!({"type":"portal_display"})
        );
        let window =
            portal_recording_options(&settings, capture_controls::TargetMode::Window).unwrap();
        assert_eq!(window.target, RecordingTarget::PortalWindow);
        assert_eq!(window.audio, options.audio);
        assert_eq!(window.countdown_seconds, 5);
        assert!(portal_recording_options(&settings, capture_controls::TargetMode::Region).is_err());
        settings.highlight_clicks = true;
        assert!(
            portal_recording_options(&settings, capture_controls::TargetMode::Display).is_err()
        );
        assert!(portal_recording_options(&settings, capture_controls::TargetMode::Window).is_err());
        settings.highlight_clicks = false;
        settings.show_keystrokes = true;
        assert!(
            portal_recording_options(&settings, capture_controls::TargetMode::Display).is_err()
        );
        assert!(portal_recording_options(&settings, capture_controls::TargetMode::Window).is_err());
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
        // Shipping ignores the second request silently (`CaptureInProgress`).
        assert_eq!(live.error, None);
        assert_eq!(live.take_capture_failure(), None);
        live.requested_capture = None;
        live.pending = 1;
        live.request_capture(CaptureRequest::Window);
        assert_eq!(live.requested_capture, None);
        assert_eq!(
            live.take_capture_failure().map(|failure| failure.error),
            Some("Another capture or history action is still in progress.".into()),
            "a History action in progress is reported in the capture dialog"
        );
        assert_eq!(live.error, None);
        live.pending = 0;
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
    fn portal_countdown_is_compact_while_direct_countdown_keeps_its_monitor() {
        let tokens = crate::tokens::load()["dark-mustard"].clone();
        let portal = countdown_viewport("Recording", None, &tokens);
        assert_eq!(portal.position, None);
        assert_eq!(portal.monitor, None);
        assert_ne!(portal.fullscreen, Some(true));
        assert_eq!(portal.inner_size, Some(egui::vec2(332., 300.)));
        assert_eq!(portal.transparent, Some(true));
        let target = CaptureTarget {
            monitor: 2,
            position: egui::pos2(-640., 75.),
            size: egui::vec2(1280., 900.),
            preview_bounds: None,
        };
        let direct = countdown_viewport("Recording", Some(target), &tokens);
        assert_eq!(direct.monitor, Some(2));
        assert_eq!(direct.position, Some(egui::pos2(-640., 75.)));
        assert_eq!(direct.inner_size, Some(egui::vec2(1280., 900.)));
        assert_eq!(direct.fullscreen, Some(true));
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
        let tween = tokens.transition(captures_app::motion::Transition::PreviewStackFan);
        let ids = live.previews.stack.ids().to_vec();
        let cards: Vec<(&str, usize)> = ids
            .iter()
            .enumerate()
            .map(|(index, id)| (id.as_str(), ids.len() - index - 1))
            .collect();
        let mut open = captures_app::preview_motion::StackFan::default();
        open.set(true, 0., &cards, &tween, false);
        ctx.data_mut(|data| data.insert_temp(fan_id(), open));
        // While reduction is enabled the pointer left the card. Re-enabling
        // motion must not resurrect the old open fan from the stored state.
        live.viewports(&ctx, &tokens, Ok(settings), true);
        let fan: captures_app::preview_motion::StackFan =
            ctx.data(|data| data.get_temp(fan_id())).unwrap();
        assert!(!fan.open());
        for (id, depth) in &cards {
            assert_eq!(fan.progress(id, *depth, 0., &tween, false), (0., 0.));
        }
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
    fn a_carried_pile_leans_after_its_gather_and_eases_back_on_drop() {
        let tween = captures_app::motion::Tween {
            duration_ms: 200.,
            easing: captures_app::motion::CubicBezier::LINEAR,
        };
        let gather = captures_app::preview_motion::StackFan::settle_ms(2, &tween);
        let mut carry = PileCarry::default();
        carry.start(1_000.);
        let at = |carry: &mut PileCarry, ms: f64, x: f32, pressed: bool| {
            let lean = carry.frame(ms, pressed, Some(egui::pos2(x, 300.)), gather, false);
            lean(2, &tween)
        };
        // The fanned pose holds while the fan gathers.
        assert_eq!(at(&mut carry, 1_016., 110., true), (0., 0.));
        assert!(carry.moving(1_016., 2, &tween, false));
        let mut x = 110.;
        let mut ms = 1_000. + gather;
        for _ in 0..8 {
            x += 30.;
            ms += 16.;
            at(&mut carry, ms, x, true);
        }
        let (lean, _) = at(&mut carry, ms + 16., x + 30., true);
        assert!(lean < -0.5, "a rightward carry leans the pile left: {lean}");
        // Dropping eases the lean back over the fan transition.
        let (dropped, _) = at(&mut carry, ms + 32., x + 30., false);
        assert!(dropped < 0., "{dropped}");
        assert!(carry.moving(ms + 40., 2, &tween, false));
        assert_eq!(at(&mut carry, ms + 32. + 400., x + 30., false), (0., 0.));
        assert!(!carry.moving(ms + 32. + 400., 2, &tween, false));
        // Reduced motion never leans.
        let mut reduced = PileCarry::default();
        reduced.start(0.);
        let lean = reduced.frame(16., true, Some(egui::pos2(0., 0.)), gather, true);
        assert_eq!(lean(2, &tween), (0., 0.));
        let lean = reduced.frame(32., true, Some(egui::pos2(90., 0.)), gather, true);
        assert_eq!(lean(2, &tween), (0., 0.));
    }

    #[test]
    fn saved_delete_dissolves_before_its_trash_and_a_failure_restores_the_slot() {
        let root = tempfile::tempdir().unwrap();
        let settings = AppSettings::default();
        let context = egui::Context::default();
        let texture = context.load_texture(
            "trash-exit-test",
            egui::ColorImage::new([1, 1], vec![egui::Color32::WHITE]),
            egui::TextureOptions::LINEAR,
        );
        let mut previews = MiniPreviews::default();
        let mut ids = Vec::new();
        for (frame, color) in [[10, 20, 30, 255], [30, 20, 10, 255]]
            .into_iter()
            .enumerate()
        {
            let artifact = preview_artifact(root.path(), color);
            previews
                .begin_capture(&settings, Some(preview_target()), frame as u64)
                .unwrap();
            let (guard, _) = previews.start_artifact(&artifact).unwrap().unwrap();
            previews.mark_ready(&guard.artifact_id);
            let card = previews.cards.get_mut(&guard.artifact_id).unwrap();
            card.texture = Some(texture.clone());
            card.saved_path = Some(root.path().join(format!("export-{frame}.png")));
            ids.push((guard.artifact_id, guard.generation));
        }
        let (id, generation) = ids[1].clone();
        let saved = previews.cards[&id].saved_path.clone();
        // The dust plays first; the request waits for it.
        assert!(!previews.begin_trash(&id, false));
        assert_eq!(previews.stack.ids(), &[ids[0].0.clone()]);
        assert!(previews.exiting.contains_key(&id));
        assert!(previews.due_trash(false).is_empty());
        // The dissolved card keeps its empty slot until the reply.
        assert!(!previews.settle_exits(true));
        assert!(previews.exiting.contains_key(&id));
        assert_eq!(previews.exits.display_ids().len(), 2);
        assert!(previews.is_visible());
        assert_eq!(
            previews.due_trash(true),
            vec![(id.clone(), generation, saved.clone())]
        );
        assert!(previews.due_trash(true).is_empty(), "sent once");
        // A failure puts the card back in its slot with the error.
        assert!(!previews.restore_trashed(&id, generation + 1, "stale".into()));
        assert!(previews.restore_trashed(&id, generation, "Trash failed: denied".into()));
        assert_eq!(previews.stack.ids(), &[ids[0].0.clone(), id.clone()]);
        assert_eq!(previews.exits.display_ids(), previews.stack.ids());
        assert!(!previews.exiting.contains_key(&id));
        assert_eq!(
            previews.cards[&id].message.as_deref(),
            Some("Trash failed: denied")
        );
        // Success forgets the dissolved card; reduced motion sends at once.
        assert!(!previews.begin_trash(&id, false));
        previews.settle_exits(true);
        assert_eq!(previews.due_trash(true).len(), 1);
        assert!(previews.finish_trash(&id, generation));
        assert!(previews.trashing.is_empty() && !previews.cards.contains_key(&id));
        assert!(!previews.exiting.contains_key(&id));
        assert_eq!(previews.exits.display_ids(), previews.stack.ids());
        assert!(
            previews.begin_trash(&ids[0].0, true),
            "reduced motion sends at once"
        );
        assert!(previews.due_trash(true).is_empty());
    }

    #[test]
    fn close_delete_and_clear_hold_slots_until_their_exits_end() {
        use captures_app::preview_motion::{ExitKind, ToolbarCause};
        let root = tempfile::tempdir().unwrap();
        let first = preview_artifact(root.path(), [10, 20, 30, 255]);
        let second = preview_artifact(root.path(), [30, 20, 10, 255]);
        let third = preview_artifact(root.path(), [50, 60, 70, 255]);
        let settings = AppSettings::default();
        let context = egui::Context::default();
        let texture = context.load_texture(
            "exit-preview-test",
            egui::ColorImage::new([1, 1], vec![egui::Color32::WHITE]),
            egui::TextureOptions::LINEAR,
        );
        let mut previews = MiniPreviews::default();
        let mut guards = Vec::new();
        for (frame, artifact) in [&first, &second, &third].into_iter().enumerate() {
            previews
                .begin_capture(&settings, Some(preview_target()), frame as u64)
                .unwrap();
            let (guard, _) = previews.start_artifact(artifact).unwrap().unwrap();
            previews.mark_ready(&guard.artifact_id);
            previews.cards.get_mut(&guard.artifact_id).unwrap().texture = Some(texture.clone());
            guards.push(guard);
        }
        let ids: Vec<String> = guards
            .iter()
            .map(|guard| guard.artifact_id.clone())
            .collect();
        previews.toolbar.set(true, ToolbarCause::Other, 0.);

        // Close keeps the card in its slot, out of the stack and its actions.
        assert!(previews.exit_card(&ids[2], ExitKind::Dismiss, false));
        assert_eq!(previews.stack.ids(), &ids[..2]);
        assert!(!previews.accepts(&ids[2], guards[2].generation));
        assert_eq!(previews.exits.display_ids(), &ids[..]);
        assert!(previews.exiting.contains_key(&ids[2]));
        assert_eq!(
            previews.toolbar.motion().map(|(motion, _)| motion),
            None,
            "the toolbar still shows for two live cards"
        );
        assert!(
            !previews.settle_exits(false),
            "the 1.03 s hold is still running"
        );
        assert!(previews.settle_exits(true));
        assert!(previews.exiting.is_empty());
        assert_eq!(previews.exits.display_ids(), &ids[..2]);

        // Delete dissolves from the trash control; fewer than two live cards
        // plays the toolbar's exit.
        previews.toolbar.set(true, ToolbarCause::Other, 0.);
        assert!(previews.exit_card(&ids[1], ExitKind::Dust, false));
        assert_eq!(previews.exiting[&ids[1]].dust.len(), 198);
        assert_eq!(
            previews.toolbar.motion().map(|(motion, _)| motion),
            Some(captures_app::motion::Motion::PreviewToolbarExit)
        );
        previews.settle_exits(true);

        // The last card's exit keeps the empty stack on its display.
        assert!(previews.exit_card(&ids[0], ExitKind::Dismiss, false));
        assert!(previews.stack.ids().is_empty());
        assert!(previews.is_visible() && previews.stack_target.is_some());
        previews.settle_exits(true);
        assert!(!previews.is_visible() && previews.stack_target.is_none());

        // Reduced motion never holds a slot; Clear all streaks bottom first.
        let mut guards = Vec::new();
        for (frame, artifact) in [&first, &second].into_iter().enumerate() {
            previews
                .begin_capture(&settings, Some(preview_target()), 10 + frame as u64)
                .unwrap();
            let (guard, _) = previews.start_artifact(artifact).unwrap().unwrap();
            previews.mark_ready(&guard.artifact_id);
            previews.cards.get_mut(&guard.artifact_id).unwrap().texture = Some(texture.clone());
            guards.push(guard);
        }
        let ids: Vec<String> = guards
            .iter()
            .map(|guard| guard.artifact_id.clone())
            .collect();
        assert_eq!(previews.clear_with_exit(&ids, true), 2);
        assert!(previews.exiting.is_empty() && !previews.is_visible());
    }

    #[test]
    fn show_less_flies_to_the_pile_and_expanding_flies_back() {
        let root = tempfile::tempdir().unwrap();
        let settings = AppSettings::default();
        let context = egui::Context::default();
        let texture = context.load_texture(
            "fly-preview-test",
            egui::ColorImage::new([1, 1], vec![egui::Color32::WHITE]),
            egui::TextureOptions::LINEAR,
        );
        let mut previews = MiniPreviews::default();
        for frame in 0..2u8 {
            let artifact = preview_artifact(root.path(), [frame * 40, 20, 30, 255]);
            previews
                .begin_capture(&settings, Some(preview_target()), u64::from(frame))
                .unwrap();
            let (guard, _) = previews.start_artifact(&artifact).unwrap().unwrap();
            previews.mark_ready(&guard.artifact_id);
            previews.cards.get_mut(&guard.artifact_id).unwrap().texture = Some(texture.clone());
        }
        previews
            .toolbar
            .set(true, captures_app::preview_motion::ToolbarCause::Other, 0.);
        previews.toggle_collapsed(false);
        assert!(previews.stack.is_collapsed());
        assert!(previews.fly.is_some_and(|fly| fly.collapsing));
        assert_eq!(
            previews.toolbar.motion().map(|(motion, _)| motion),
            Some(captures_app::motion::Motion::PreviewToolbarOut)
        );
        assert!(
            !previews.settle_exits(false),
            "the 520 ms flight is running"
        );
        assert!(previews.settle_exits(true));
        assert!(previews.fly.is_none());
        previews.toggle_collapsed(false);
        assert!(!previews.stack.is_collapsed());
        assert!(previews.fly.is_some_and(|fly| !fly.collapsing));
        assert_eq!(
            previews.toolbar.motion().map(|(motion, _)| motion),
            Some(captures_app::motion::Motion::PreviewToolbarIn)
        );
        previews.toggle_collapsed(true);
        assert!(previews.fly.is_none(), "reduced motion snaps");
        assert_eq!(
            STACK_FLY.as_secs_f64() * 1000.,
            match captures_app::motion::Transition::PreviewStackFly
                .spec()
                .duration
            {
                captures_app::motion::Timing::Millis(ms) => ms,
                captures_app::motion::Timing::Token(_) => unreachable!(),
            }
        );
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
    fn history_restore_reopens_a_screenshot_preview_once() {
        use captures_app::history_view::CardAction;
        let root = tempfile::tempdir().unwrap();
        let artifact = preview_artifact(root.path(), [10, 20, 30, 255]);
        let id = artifact.entry.id.clone();
        let ctx = egui::Context::default();
        let mut frame = eframe::Frame::_new_kittest();
        let mut live = Live::new(ctx.clone(), Some(root.path().into()));
        live.flush();
        let (jobs, requests) = mpsc::channel();
        live.tx = jobs;
        let (results, replies) = mpsc::channel();
        live.rx = replies;
        live.pending = 0;
        let mut recording = Artifact {
            entry: artifact.entry.clone(),
            image_path: artifact.image_path.clone(),
            preview_path: artifact.preview_path.clone(),
        };
        recording.entry.id = "recording".into();
        recording.entry.kind = captures_history::ArtifactKind::Video;
        live.artifacts.push(artifact);
        live.artifacts.push(recording);
        let settings = AppSettings::default();
        let decoded = |generation, artifact_id: &str| Reply::PreviewDecoded {
            generation,
            artifact_id: artifact_id.to_owned(),
            result: Ok(Decoded {
                image: egui::ColorImage::new([1, 1], vec![egui::Color32::WHITE]),
            }),
            blurred: None,
            arrive_blurred: None,
            media: None,
        };
        let decode_job = || match requests.try_recv() {
            Ok(Job::DecodePreview {
                generation,
                artifact_id,
                ..
            }) => (generation, artifact_id),
            _ => panic!("no preview decode"),
        };
        ctx.begin_pass(Default::default());

        // Recordings are never restored (shipping rejects them).
        live.restore(&ctx, "recording", &settings, Some(preview_target()));
        assert!(live.previews.stack.ids().is_empty());
        // An empty stack needs a display to open on. Like shipping, the error
        // shows on the card (`.history-card-error`), not the status line.
        live.restore(&ctx, &id, &settings, None);
        assert!(live.previews.stack.ids().is_empty());
        assert!(live.card_errors.contains_key(&id) && live.error.is_none());
        assert!(requests.try_recv().is_err());

        live.restore(&ctx, &id, &settings, Some(preview_target()));
        assert!(live.card_errors.is_empty(), "the next restore clears it");
        assert_eq!(live.previews.stack.ids(), std::slice::from_ref(&id));
        let (generation, artifact_id) = decode_job();
        assert_eq!(artifact_id, id);
        assert_eq!(
            live.card_restoring.as_ref().map(|guard| guard.generation),
            Some(generation)
        );
        // Card actions wait for the restore; no clipboard copy is queued.
        live.card_action(&ctx, &id, CardAction::Restore, Ok(settings.clone()), &frame);
        assert!(requests.try_recv().is_err());
        results.send(decoded(generation, &id)).unwrap();
        live.logic(&ctx, &mut frame);
        assert!(live.card_restoring.is_none());
        assert!(live.previews.cards[&id].texture.is_some());
        let now = Instant::now();
        assert_eq!(live.restored_feedback(&ctx, now), Some(id.clone()));
        let expiry = Duration::from_millis(captures_app::history_view::ACTION_FEEDBACK_MS);
        assert_eq!(live.restored_feedback(&ctx, now + expiry), None);

        // Restoring a card that is already showing neither duplicates nor
        // reorders it, and still confirms "Restored".
        live.restore(&ctx, &id, &settings, None);
        assert_eq!(live.previews.stack.ids(), std::slice::from_ref(&id));
        assert!(live.card_restoring.is_none() && live.card_restored.is_some());
        assert!(requests.try_recv().is_err());

        // A card dismissed before it decodes ends the restore without feedback.
        let generation = live.previews.cards[&id].generation;
        assert!(live.previews.dismiss(&id, generation));
        live.card_restored = None;
        live.restore(&ctx, &id, &settings, Some(preview_target()));
        let (generation, _) = decode_job();
        assert!(live.previews.dismiss(&id, generation));
        results.send(decoded(generation, &id)).unwrap();
        live.logic(&ctx, &mut frame);
        assert!(live.card_restoring.is_none() && live.card_restored.is_none());
        assert!(live.previews.cards.is_empty());

        // A decode failure reports the error and leaves no card behind.
        live.restore(&ctx, &id, &settings, Some(preview_target()));
        let (generation, _) = decode_job();
        results
            .send(Reply::PreviewDecoded {
                generation,
                artifact_id: id.clone(),
                result: Err("unreadable".into()),
                blurred: None,
                arrive_blurred: None,
                media: None,
            })
            .unwrap();
        live.logic(&ctx, &mut frame);
        assert!(live.card_restoring.is_none() && live.card_restored.is_none());
        assert!(live.previews.stack.ids().is_empty());
        assert!(
            live.card_errors
                .get(&id)
                .is_some_and(|error| error.contains("unreadable"))
        );
        assert!(live.error.is_none());
        live.flush();
    }

    #[test]
    fn history_edit_restores_the_preview_before_opening_the_editor() {
        use captures_app::history_view::CardAction;
        let root = tempfile::tempdir().unwrap();
        let shown = preview_artifact(root.path(), [10, 20, 30, 255]);
        let edited = preview_artifact(root.path(), [30, 20, 10, 255]);
        let (shown_id, edited_id) = (shown.entry.id.clone(), edited.entry.id.clone());
        let ctx = egui::Context::default();
        let frame = eframe::Frame::_new_kittest();
        let mut live = Live::new(ctx.clone(), Some(root.path().into()));
        live.flush();
        let deadline = Instant::now() + Duration::from_secs(5);
        while live.recovery.blocking() {
            live.recovery.receive();
            assert!(
                Instant::now() < deadline,
                "initial recovery list did not settle"
            );
            thread::sleep(Duration::from_millis(5));
        }
        let (jobs, requests) = mpsc::channel();
        live.tx = jobs;
        live.pending = 0;
        live.artifacts = vec![shown, edited];
        let settings = AppSettings::default();
        ctx.begin_pass(Default::default());

        // An empty stack has no display to open on here: nothing opens.
        live.card_action(
            &ctx,
            &edited_id,
            CardAction::Edit,
            Ok(settings.clone()),
            &frame,
        );
        assert!(live.editors.is_empty() && live.card_errors.contains_key(&edited_id));
        assert!(live.error.is_none());
        assert!(live.previews.stack.ids().is_empty());

        // With a pile on screen, Edit brings the capture back as the front
        // card, then opens its editor, without Restore's busy state.
        assert!(matches!(
            live.previews.restore_artifact(
                &live.artifacts[0],
                &settings,
                Some(preview_target()),
                false
            ),
            Ok(RestoreStart::Decode(..))
        ));
        live.card_action(
            &ctx,
            &edited_id,
            CardAction::Edit,
            Ok(settings.clone()),
            &frame,
        );
        assert_eq!(live.previews.stack.ids(), [shown_id, edited_id.clone()]);
        assert!(matches!(
            requests.try_recv(),
            Ok(Job::DecodePreview { artifact_id, .. }) if artifact_id == edited_id
        ));
        assert!(live.editors.contains_key(&edited_id));
        assert!(live.card_restoring.is_none() && live.card_restored.is_none());
        assert!(
            live.card_errors.is_empty(),
            "acting on the card clears its error"
        );

        // Editing again neither duplicates nor reorders the card.
        live.card_action(&ctx, &edited_id, CardAction::Edit, Ok(settings), &frame);
        assert_eq!(live.previews.stack.ids().len(), 2);
        assert!(requests.try_recv().is_err());
        live.flush();
    }

    #[test]
    fn wayland_history_edit_opens_without_preview_and_restore_explains_the_limit() {
        use captures_app::history_view::CardAction;
        let root = tempfile::tempdir().unwrap();
        let artifact = preview_artifact(root.path(), [31, 109, 207, 255]);
        let id = artifact.entry.id.clone();
        let ctx = egui::Context::default();
        ctx.data_mut(|data| data.insert_temp(egui::Id::unique("wayland-surface"), true));
        let frame = eframe::Frame::_new_kittest();
        let mut live = Live::new(ctx.clone(), Some(root.path().into()));
        live.flush();
        let deadline = Instant::now() + Duration::from_secs(5);
        while live.recovery.blocking() {
            live.recovery.receive();
            assert!(Instant::now() < deadline, "initial recovery did not settle");
            thread::sleep(Duration::from_millis(5));
        }
        let (jobs, requests) = mpsc::channel();
        live.tx = jobs;
        live.pending = 0;
        live.artifacts = vec![artifact];
        let settings = AppSettings::default();
        ctx.begin_pass(Default::default());

        live.card_action(&ctx, &id, CardAction::Restore, Ok(settings.clone()), &frame);
        assert_eq!(
            live.card_errors[&id],
            "Floating previews are not available on Wayland. Use Edit instead."
        );
        assert!(live.previews.stack.ids().is_empty());
        assert!(requests.try_recv().is_err());

        live.card_action(&ctx, &id, CardAction::Edit, Ok(settings.clone()), &frame);
        assert!(
            live.editors.contains_key(&id),
            "the saved screenshot must open without monitor geometry"
        );
        assert!(live.card_errors.is_empty());
        assert!(live.previews.stack.ids().is_empty());
        assert!(
            requests.try_recv().is_err(),
            "no floating preview decode may be queued"
        );
        assert!(
            live.restore_for_edit(&ctx, "missing", &settings, None)
                .is_err()
        );
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
