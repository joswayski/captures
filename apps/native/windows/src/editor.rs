//! The screenshot editor, shipping `ScreenshotEditor.tsx` with the
//! `styles/editor-image.css` rules, as one Win32 window per History artifact.
//! Copy, tools, viewport math, layers and the export plan come from the
//! shared `captures_app::editor_*` modules, as in the GTK4 and AppKit hosts.
//!
//! One worker thread owns the `EditorSession`: rendering, undo, drafts,
//! export and clipboard encoding never run on the UI thread. The worker also
//! converts the session's pixels to premultiplied BGRA and reports the
//! rectangle that changed.
//!
//! The canvas is three retained composition layers inside the viewport:
//! - the page (shadow, border, background or checkerboard), redrawn only when
//!   its on-screen size changes;
//! - the image, one surface at document resolution, uploaded only where an
//!   edit changed pixels; zoom and pan are a compositor transform on its
//!   visual, so they never redraw it;
//! - the overlay (selection outline, in-progress shape or stroke), redrawn
//!   only inside the union of what it showed and what it shows now.
//!
//! Chrome follows the Preferences and History nodes: hover layers, focus
//! rings and the zoom thumb move on the compositor, and nothing redraws at
//! idle.
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Weak};
use std::time::Instant;

use captures_app::editor::{
    AlignmentGuide, ClosedShapeCreate, ClosedShapeKind, CropDrag, Document, ElementStyle,
    FreehandPathCreate, GuideOrientation, LayerEdit, OpenShapeCreate, OpenShapeKind, Point,
    Rect as DocRect, ResizeHandle,
};
use captures_app::editor_chrome::{self, RAIL_TOOLS, SHAPE_TOOLS, header};
use captures_app::editor_export::{self, EstimateState, ExportSource, ExportTarget, SavePlan};
use captures_app::editor_output::SavedExport;
use captures_app::editor_session::{
    DraftAutosave, EditorSession, ExportFormat, ExportOptions, ExportQuality, ExportSize,
    OpenRequest, PngOptions, PreviewDamage, Request,
};
use captures_app::editor_viewport::{self, Viewport};
use captures_app::motion::CubicBezier;
use captures_capture::CaptureMode;
use image::RgbaImage;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectComposition::*;
use windows::Win32::Graphics::DirectWrite::DWRITE_TEXT_ALIGNMENT_TRAILING;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;
use windows_numerics::{Matrix3x2, Vector2};

use crate::gfx::{self, Gfx, Style, Text, rect};
use crate::scene::Rect;
use crate::tokens::Tokens;
use crate::{Options, appearance};

mod background;
mod crop;
pub use crop::ASPECT_LABEL;
mod eraser;
mod fields;
mod menu;
mod props;
mod text;
mod text_section;
pub use menu::MenuKind;
pub use props::{CUSTOM, Check, Field, Num, Prop, Range, bounds as range_bounds, range_text};

/// `.screenshot-editor { min-width: 860px; min-height: 600px }`; GTK's default.
pub const SIZE: (f32, f32) = (1280., 840.);
const MIN_SIZE: (f32, f32) = (860., 600.);
/// Shipping Fit: `(clientWidth - 56) / width`, 28 px a side.
const FIT_INSET: f64 = 28.;
/// `.screenshot-editor-header` plus its bottom border.
const HEADER_H: f32 = editor_chrome::HEADER_HEIGHT as f32 + 1.;
/// `.screenshot-export-bar`: 8 px padding around a 60 px row, top border.
const BAR_H: f32 = 77.;
const RAIL_W: f32 = editor_chrome::RAIL_WIDTH as f32;
const RAIL_BUTTON: f32 = editor_chrome::RAIL_BUTTON as f32;
const CONTROL_H: f32 = editor_chrome::HEADER_CONTROL as f32;
const ZOOM_BUTTON: f32 = editor_chrome::ZOOM_BUTTON as f32;
const CANVAS_FIELD: f32 = editor_chrome::CANVAS_FIELD as f32;
/// Shipping `sidebarMinWidth` and the 480 px cap.
const SIDEBAR_MIN: f32 = 280.;
const SIDEBAR_MAX: f32 = 480.;
/// `.screenshot-sidebar-resize { width: var(--s-4) }`, centred on the edge.
const DIVIDER_W: f32 = 8.;
/// `.screenshot-layers-heading { min-height: 48px }`.
const HEADING_H: f32 = 48.;
/// A layer row (`min-height: 54px` plus its border) and its 2 px margins.
const ROW_H: f32 = 56.;
const ROW_GAP: f32 = 4.;
/// `.screenshot-layer-quick-actions > button`.
const QUICK: (f32, f32) = (25., 28.);
/// `.screenshot-make-copy` switch.
const SWITCH: (f32, f32) = (28., 16.);
/// `--shadow-lg` reach around the page.
const PAGE_BLEED: f32 = 48.;
/// The checkerboard's 16 px period (`background-size: 16px 16px`).
const CHECKER: f32 = 16.;
const KEYBOARD_ZOOM: f64 = 1.25;
const ZOOM_MIN: f64 = 5.;
const ZOOM_MAX: f64 = 800.;
/// A gesture shorter than this many screen DIPs is a click.
const DRAG_MIN: f64 = 3.;
const SEGMENTS: usize = 16;

const WM_APP_OUTCOME: u32 = WM_APP + 21;
pub(crate) const WM_APP_WAKE: u32 = WM_APP + 22;
const WM_MOUSELEAVE: u32 = 0x02A3;
const TIMER_MOTION: usize = 1;
const TIMER_AUTOSAVE: usize = 2;
const TIMER_PREVIEW: usize = 3;
const TIMER_CLOSE: usize = 4;
/// How long a closed editor's hidden window waits for its draft flush before
/// it is destroyed anyway, as shipping's `SCREENSHOT_EDITOR_DRAFT_CLOSE_FLUSH_MS`.
/// The worker keeps going after that (see `wait_for_workers`).
const CLOSE_FLUSH_MS: u32 = 400;
/// The most quitting, logging off or reopening an artifact waits for a
/// closed editor's worker to finish its queue, so a slow disk cannot hang it.
const WORKER_EXIT_WAIT: std::time::Duration = std::time::Duration::from_secs(5);
/// Brush and grip previews render on the worker (a grip re-renders the
/// whole document, about 45 ms at 3840×2160; a brush only its new stroke's
/// area where nothing else paints, about 16 ms); a pointer reports far more
/// often than that is useful, so they go at most this often, and never keep
/// the worker busy more than half the time.
const PREVIEW_INTERVAL: std::time::Duration = std::time::Duration::from_millis(33);

thread_local! {
    static EDITORS: RefCell<Vec<Editor>> = const { RefCell::new(Vec::new()) };
    /// Every editor worker by artifact ID, including closed editors' workers
    /// still finishing their queue.
    static WORKERS: RefCell<Vec<(String, std::thread::JoinHandle<()>)>> =
        const { RefCell::new(Vec::new()) };
}

/// Wait, at most until `deadline`, for `workers` to finish.
fn join_until(workers: Vec<std::thread::JoinHandle<()>>, deadline: Instant) {
    for worker in workers {
        while !worker.is_finished() && Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        if worker.is_finished() {
            let _ = worker.join();
        }
    }
}

/// Before the process exits: let closed editors' workers finish committing
/// open labels and writing drafts, for at most `WORKER_EXIT_WAIT`.
pub fn wait_for_workers() {
    let workers: Vec<_> = WORKERS.with(|w| w.borrow_mut().drain(..).map(|(_, w)| w).collect());
    join_until(workers, Instant::now() + WORKER_EXIT_WAIT);
}

/// Run `f` on the editor window `hwnd` unless it is borrowed or closed;
/// whatever `f` leaves dirty is guaranteed a frame (see `host::with_host`).
pub fn with_editor<R>(hwnd: HWND, f: impl FnOnce(&mut Editor) -> R) -> Option<R> {
    EDITORS.with(|e| {
        let mut e = e.try_borrow_mut().ok()?;
        let editor = e.iter_mut().find(|editor| editor.hwnd == hwnd)?;
        let result = f(editor);
        editor.wake();
        Some(result)
    })
}

/// Every open editor window.
pub fn windows() -> Vec<HWND> {
    EDITORS.with(|e| {
        e.try_borrow()
            .map(|e| e.iter().map(|editor| editor.hwnd).collect())
            .unwrap_or_default()
    })
}

pub fn exists() -> bool {
    EDITORS.with(|e| e.try_borrow().map_or(true, |e| !e.is_empty()))
}

/// Something that takes focus or a click.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// The restored-draft banner's Dismiss.
    Dismiss,
    /// The Canvas group's Trim edges.
    Trim,
    Undo,
    Redo,
    Fit,
    ZoomOut,
    Slider,
    ZoomIn,
    /// A rail tool, in `RAIL_TOOLS` order (Shapes opens the flyout).
    Tool(usize),
    /// A Shapes flyout item, in `SHAPE_TOOLS` order.
    Shape(usize),
    /// `.screenshot-sidebar-resize`.
    Divider,
    /// A Layers row (top layer first), its visibility and lock toggles.
    Layer(usize),
    Eye(usize),
    Lock(usize),
    Copy,
    SaveAsNew,
    Save,
    /// A Tool properties control.
    Prop(Prop),
    /// The Canvas group's Background color trigger.
    Background,
}

/// Shipping controls this host does not implement yet: drawn disabled and
/// exposed to UI Automation as disabled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stub {
    Preset,
    AddImages,
    AddLayer,
    LayerMenu(usize),
    ExportSettings,
    Change,
    Format,
}

/// Accessibility notifications, raised after the frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Focus(Target),
    /// A toggle's pressed or checked state changed.
    Toggled(Target),
    /// A slider's or separator's value changed.
    Value(Target),
    /// Which elements exist changed (layers, the flyout, the banner).
    Structure,
    /// The export notice changed (`role="status"`).
    Notice,
    /// The inline text field took focus.
    InlineFocus,
    /// The inline text field's value changed.
    InlineText,
    /// A popup trigger opened or closed (ExpandCollapse state).
    Expanded(Target),
    /// The open popup's highlighted row moved.
    MenuActive(usize),
    /// The Properties body scrolled.
    Scrolled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Layer {
    Base,
    Lit,
    Ring,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Group,
    /// `.screenshot-editor-draft-banner`.
    Banner,
    /// `.screenshot-editor-header` with the Canvas toolbar.
    Header,
    /// `.screenshot-tool-rail`.
    Rail,
    /// `.screenshot-canvas-viewport`'s well; clips the canvas layers.
    Well,
    Page,
    Image,
    Overlay,
    /// The inline text caret, blinking on the compositor.
    Caret,
    /// `.screenshot-sidebar` background, borders and section rule.
    Sidebar,
    /// `.screenshot-layers-heading`.
    LayersHeading,
    /// The layer list's clip.
    List,
    /// `.screenshot-properties-heading`.
    Properties,
    /// The Properties section under the heading: legends and its rule.
    PropsBody,
    /// `.screenshot-export-bar`.
    Bar,
    /// The zoom group's frame, the disabled preset readout and Add images.
    Zoom,
    /// `.screenshot-editor-zoom-slider`'s thumb, moved on the compositor.
    Thumb,
    /// `.screenshot-tool-flyout`.
    Flyout,
    /// `.screenshot-canvas-bg-menu`.
    BgMenu,
    /// The Text style menu or the Font list, and its highlighted row.
    Menu,
    MenuLit,
    /// A focused property input's caret, blinking on the compositor.
    FieldCaret,
    /// The Properties scrollbar thumb (`base.css` `::-webkit-scrollbar`),
    /// moved on the compositor while scrolling.
    PropsThumb,
    Ctl(Target, Layer),
}

struct Node {
    parent: Option<usize>,
    kind: Kind,
    rect: Rect,
    bleed: f32,
    opacity: f32,
    visible: bool,
    clip: bool,
    dirty: bool,
    props: bool,
}

struct Motion {
    node: usize,
    from: f32,
    to: f32,
    start: Instant,
    seconds: f32,
    curve: CubicBezier,
    applied: bool,
    fresh: bool,
}

impl Motion {
    fn value_at(&self, progress: f32) -> f32 {
        self.from + (self.to - self.from) * self.curve.ease(f64::from(progress)) as f32
    }

    fn progress(&self, now: Instant) -> f32 {
        (now.duration_since(self.start).as_secs_f32() / self.seconds).clamp(0., 1.)
    }
}

#[derive(Clone, Copy)]
struct Layered {
    base: usize,
    lit: usize,
    ring: usize,
}

/// One Layers row, top layer first.
pub struct LayerItem {
    pub id: String,
    pub name: String,
    pub kind: &'static str,
    pub icon: &'static str,
    pub visible: bool,
    pub locked: bool,
    pub visibility_label: String,
    pub lock_label: String,
    pub menu_label: String,
}

/// Changed document pixels as premultiplied BGRA: only the `damage`
/// rectangle `(x, y, w, h)`, packed row by row, or with no damage the whole
/// `width`×`height` image. Only the changed area crosses threads and gets
/// converted; the UI patches it into its own `Image`.
pub struct Pixels {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
    pub damage: Option<(u32, u32, u32, u32)>,
}

/// The UI's copy of the document's pixels as premultiplied BGRA, owned by
/// the UI alone so a patch never copies the whole image.
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

/// What the worker publishes after every edit.
pub struct Presented {
    pub document: Document,
    pub layers: Vec<LayerItem>,
    pub outlines: BTreeMap<String, [Point; 4]>,
    pub can_undo: bool,
    pub can_redo: bool,
    pub style: ElementStyle,
    pub has_draft: bool,
    /// Shipping `canTrimEdges`.
    pub can_trim: bool,
    /// The session's active inline text input.
    pub active_input: Option<text::ActiveInput>,
    /// Font family keys to the bundled family names.
    pub font_families: BTreeMap<String, String>,
    /// Shipping's initial text size for new labels.
    pub text_size: f64,
    /// Resolved shape and path controls per layer ID.
    pub controls: BTreeMap<String, props::Values>,
    pub source: Option<ExportSource>,
    /// New pixels, when they changed.
    pub pixels: Option<Pixels>,
}

enum Job {
    Apply(Request),
    Save {
        plan: SavePlan,
        options: ExportOptions,
        mode: CaptureMode,
    },
    Copy,
    Autosave,
    /// Closing: write the draft, then report `Outcome::Flushed`.
    Flush,
    /// Render an uncommitted gesture without touching history.
    Preview(Request),
    /// Publish the session's pixels again (a cancelled preview).
    Refresh,
}

/// A preview's changed pixels and selection outlines.
type Previewed = (
    Option<Pixels>,
    BTreeMap<String, [Point; 4]>,
    Vec<AlignmentGuide>,
);

enum Outcome {
    Presented(Box<Presented>),
    Preview(std::result::Result<Previewed, String>),
    Saved(std::result::Result<(SavePlan, SavedExport), String>),
    Copied(std::result::Result<(Vec<u8>, Vec<u8>), String>),
    Failed(String),
    /// `Job::Flush` finished.
    Flushed,
}

#[derive(Clone, Debug, PartialEq)]
enum Gesture {
    /// Staging a crop rectangle.
    Crop(CropDrag),
    /// Drawing a shape, line or arrow.
    Shape {
        tool: &'static str,
        start: Point,
        end: Point,
    },
    Freehand(Vec<Point>),
    /// Moving the selected layer by the pointer's document delta.
    Move {
        id: String,
        start: Point,
        end: Point,
    },
    /// An Erase or Restore brush stroke.
    Brush(Vec<Point>),
    /// Dragging a selection grip.
    Resize {
        id: String,
        handle: ResizeHandle,
        start: Point,
        end: Point,
    },
    /// Panning from the viewport it started with, from a client point.
    Pan {
        from: Viewport,
        x: f32,
        y: f32,
    },
}

/// Laid-out regions in client DIPs.
#[derive(Clone, Copy, Default)]
pub struct Layout {
    pub banner: Rect,
    pub header: Rect,
    pub rail: Rect,
    pub well: Rect,
    pub sidebar: Rect,
    pub layers: Rect,
    pub list: Rect,
    pub properties: Rect,
    pub bar: Rect,
    pub canvas_group: Rect,
    pub width_box: Rect,
    pub height_box: Rect,
    pub trim: Rect,
    pub background: Rect,
    pub zoom: Rect,
    pub preset: Rect,
    pub add_images: Rect,
    pub add_layer: Rect,
    pub flyout: Rect,
    pub export_settings: Rect,
    pub filename: Rect,
    pub suffix: Rect,
    pub change: Rect,
    pub folder: Rect,
    pub hint: Rect,
    pub switch_label: Rect,
    pub buttons: [Rect; SLOTS],
    compact: bool,
}

pub struct Editor {
    pub hwnd: HWND,
    pub g: Gfx,
    pub t: Tokens,
    pub artifact_id: String,
    settings_file: PathBuf,
    appearance: Option<String>,
    /// The settings' save folder, for screenshots without a source file;
    /// empty when the settings file exists but can't be read.
    output_directory: PathBuf,
    /// Why `output_directory` is empty: Save reports it rather than
    /// quietly targeting the default Captures folder.
    settings_error: Option<String>,
    default_stem: String,
    mode: CaptureMode,
    pub scale: f32,
    reduced_motion: bool,
    jobs: Sender<Job>,
    outcomes: Receiver<Outcome>,
    pub presented: Option<Presented>,
    /// The last pixels, for new surfaces and the offscreen renderer.
    image: Option<Image>,
    /// The rectangle of `image` still to upload to the image surface.
    upload: Option<(u32, u32, u32, u32)>,
    pub viewport: Viewport,
    pub tool: &'static str,
    pub last_shape: usize,
    pub selected: Option<String>,
    gesture: Option<Gesture>,
    target: Option<ExportTarget>,
    options: ExportOptions,
    pub notice: Option<(String, bool)>,
    busy: bool,
    pub banner: bool,
    /// Shipping `preferredSidebarWidth`: window-local, never in the document.
    sidebar_pref: Option<f32>,
    divider_drag: Option<(f32, f32)>,
    slider_drag: bool,
    pub flyout: bool,
    pub list_scroll: f32,
    /// New-tool defaults, adopted once from the session.
    defaults: Option<props::Defaults>,
    props: Vec<props::Laid>,
    props_height: f32,
    pub props_scroll: f32,
    props_clip: usize,
    props_body: usize,
    props_thumb: usize,
    /// A thumb drag: the pointer y and the scroll it started from.
    thumb_drag: Option<(f32, f32)>,
    thumb_hover: bool,
    /// A slider drag in progress and its value.
    range_drag: Option<(Range, f64)>,
    /// The latest live edit waiting for the one in flight.
    pending: Option<Request>,
    /// Numbers each label Size or Text change, so none fold into another.
    edit_seq: u64,
    /// A custom color to ask for once the window is released.
    pick_color: Option<Field>,
    /// Eraser mode and settings, window-local like shipping's.
    pub eraser: props::Eraser,
    /// The staged crop rectangle (shipping `cropSelection`).
    pub crop: Option<DocRect>,
    /// The Aspect ratio select's choice, in `crop::ASPECTS` order.
    pub crop_aspect: usize,
    /// The Text tool's open label.
    inline: Option<text::Inline>,
    /// The open Text style menu or Font list.
    menu: Option<menu::Menu>,
    menu_node: usize,
    menu_lit: usize,
    /// The menu row under a press, chosen on release.
    menu_press: Option<usize>,
    /// The focused property input and its caret.
    field: Option<fields::FieldEdit>,
    field_caret: usize,
    /// The caret of a focused Canvas W or H, in the header.
    header_caret: usize,
    /// Shipping `defaultTextStyle` and `defaultFontSize`: window-local; the
    /// size starts at the session's `text_size`.
    text_preset: &'static str,
    text_size: Option<f64>,
    input_counter: u32,
    caret: usize,
    caret_blink: bool,
    caret_color: Option<[u32; 4]>,
    high_surrogate: Option<u16>,
    /// The IME is enabled (only while a label is open).
    ime: bool,
    /// Selecting text with the pointer.
    text_drag: bool,
    /// A gesture preview is with the worker; another is due after it.
    preview_busy: bool,
    preview_dirty: bool,
    /// When the last preview went to the worker, when the worker may take
    /// the next (idle at least as long as the last one took), and whether a
    /// timer will send it.
    preview_sent: Option<Instant>,
    preview_ready: Option<Instant>,
    preview_timer: bool,
    /// The resized layer's outline from the latest preview.
    preview_outline: Option<[Point; 4]>,
    /// Shipping's alignment snap guides from the latest move or grip preview.
    guides: Vec<AlignmentGuide>,
    autosave: DraftAutosave,
    pub w: f32,
    pub h: f32,
    pub l: Layout,
    pub hover: Option<Target>,
    pub focus: Option<Target>,
    focus_visible: bool,
    pressed: Option<Target>,
    pointer: Option<(f32, f32)>,
    tracking: bool,
    nodes: Vec<Node>,
    children: Vec<Vec<usize>>,
    well: usize,
    page: usize,
    image_node: usize,
    overlay: usize,
    layers_heading: usize,
    list: usize,
    properties: usize,
    bar: usize,
    header: usize,
    banner_node: usize,
    zoom: usize,
    thumb: usize,
    flyout_node: usize,
    /// The Background color card, whether it's open, and shipping's
    /// `lastSolid`.
    bg_menu: usize,
    bg_open: bool,
    last_solid: String,
    controls: Vec<(Target, Layered)>,
    /// Layer rows built so far; rows past the layer count are hidden.
    rows: usize,
    /// Where the page was last drawn relative to the image, for its redraw.
    page_key: (f32, f32, f32, f32),
    /// The overlay's last drawn box, well-relative DIPs.
    overlay_drawn: (Option<Rect>, Option<Rect>),
    /// The overlay surface was just created and has never been drawn:
    /// DirectComposition rejects a partial `BeginDraw` on it (E_INVALIDARG).
    overlay_fresh: bool,
    motions: Vec<Motion>,
    dcomp: Option<IDCompositionDesktopDevice>,
    _target: Option<IDCompositionTarget>,
    visuals: Vec<Option<IDCompositionVisual3>>,
    surfaces: Vec<Option<(IDCompositionSurface, (u32, u32))>>,
    offsets: Vec<(f32, f32)>,
    timer: bool,
    pub need: bool,
    relayout: bool,
    /// A `WM_APP_WAKE` was posted since the loop last checked `need`.
    pub(crate) woken: bool,
    pub events: Vec<Event>,
    /// Close was requested: the window is hidden while the worker commits
    /// an open label and flushes the draft.
    closing: bool,
    /// The flush finished or timed out; `wndproc` destroys the window.
    close_ready: bool,
}

fn closed_kind(tool: &str) -> Option<ClosedShapeKind> {
    Some(match tool {
        "rectangle" => ClosedShapeKind::Rectangle,
        "ellipse" => ClosedShapeKind::Ellipse,
        "triangle" => ClosedShapeKind::Triangle,
        "diamond" => ClosedShapeKind::Diamond,
        "star" => ClosedShapeKind::Star,
        _ => return None,
    })
}

fn open_kind(tool: &str) -> Option<OpenShapeKind> {
    Some(match tool {
        "line" => OpenShapeKind::Line,
        "a" => OpenShapeKind::Arrow,
        _ => return None,
    })
}

/// The rail index of the grouped Shapes button.
fn shapes_index() -> usize {
    RAIL_TOOLS
        .iter()
        .position(|tool| tool.key == editor_chrome::SHAPES_KEY)
        .expect("shapes")
}

fn post(hwnd: isize, msg: u32) {
    unsafe {
        let _ = PostMessageW(Some(HWND(hwnd as *mut _)), msg, WPARAM(0), LPARAM(0));
    }
}

/// Straight RGBA inside `(x, y, w, h)` as packed premultiplied BGRA.
fn convert(src: &RgbaImage, (x, y, w, h): (u32, u32, u32, u32)) -> Vec<u8> {
    let width = src.width() as usize;
    let raw = src.as_raw();
    let mut out = Vec::with_capacity(w as usize * h as usize * 4);
    for row in y as usize..(y + h) as usize {
        let start = (row * width + x as usize) * 4;
        out.extend(
            raw[start..start + w as usize * 4]
                .chunks_exact(4)
                .flat_map(|s| {
                    let a = u32::from(s[3]);
                    let pm = |c: u8| ((u32::from(c) * a + 127) / 255) as u8;
                    [pm(s[2]), pm(s[1]), pm(s[0]), s[3]]
                }),
        );
    }
    out
}

/// The rectangle where two equally sized images differ, if any.
fn diff(old: &RgbaImage, new: &RgbaImage) -> Option<(u32, u32, u32, u32)> {
    let width = new.width() as usize;
    let stride = width * 4;
    let (a, b) = (old.as_raw(), new.as_raw());
    let rows: Vec<usize> = (0..new.height() as usize)
        .filter(|&row| a[row * stride..(row + 1) * stride] != b[row * stride..(row + 1) * stride])
        .collect();
    let (&top, &bottom) = (rows.first()?, rows.last()?);
    let (mut left, mut right) = (width, 0);
    for &row in &rows {
        let span = row * stride..(row + 1) * stride;
        let (la, lb) = (&a[span.clone()], &b[span]);
        if let Some(first) = (0..width).find(|&x| la[x * 4..x * 4 + 4] != lb[x * 4..x * 4 + 4]) {
            left = left.min(first);
            let last = (first..width)
                .rev()
                .find(|&x| la[x * 4..x * 4 + 4] != lb[x * 4..x * 4 + 4])
                .unwrap_or(first);
            right = right.max(last);
        }
    }
    Some((
        left as u32,
        top as u32,
        (right + 1 - left) as u32,
        (bottom + 1 - top) as u32,
    ))
}

/// What the worker last published. `base` is the last frame published
/// without preview damage, which the next diff compares with. While a brush
/// stroke reports its damage, `chain` holds only the identity of the frame on
/// screen and the union of everything it changed since `base`: keeping it
/// weak lets the shared session reuse that frame for the next preview
/// instead of copying it.
#[derive(Default)]
struct Published {
    base: Option<Arc<RgbaImage>>,
    chain: Option<(Weak<RgbaImage>, PixelRect)>,
}

/// `(x, y, width, height)` in canvas pixels.
type PixelRect = (u32, u32, u32, u32);

impl Published {
    /// Whether `frame` is the frame on screen.
    fn shows(&self, frame: *const RgbaImage) -> bool {
        match &self.chain {
            Some((shown, _)) => std::ptr::eq(shown.as_ptr(), frame),
            None => self
                .base
                .as_ref()
                .is_some_and(|base| std::ptr::eq(Arc::as_ptr(base), frame)),
        }
    }
}

/// New pixels against the ones on screen: the changed rectangle as
/// premultiplied BGRA (all of it after a resize), or `None` when nothing
/// changed. A preview's own damage replaces the full-image diff when it is
/// relative to the frame on screen.
fn publish(
    rgba: Arc<RgbaImage>,
    damage: Option<PreviewDamage>,
    published: &mut Published,
) -> Option<Pixels> {
    let size = rgba.dimensions();
    if published.shows(Arc::as_ptr(&rgba)) {
        return None;
    }
    let pixels = |rect: Option<(u32, u32, u32, u32)>| Pixels {
        width: size.0,
        height: size.1,
        bgra: convert(&rgba, rect.unwrap_or((0, 0, size.0, size.1))),
        damage: rect,
    };
    if let Some(damage) = damage
        && published.shows(damage.since.as_ptr())
        && published
            .base
            .as_ref()
            .is_some_and(|b| b.dimensions() == size)
    {
        let shown = published
            .chain
            .as_ref()
            .map_or(damage.rect, |(_, u)| union(*u, damage.rect));
        published.chain = Some((Arc::downgrade(&rgba), shown));
        return (!empty(damage.rect)).then(|| pixels(Some(damage.rect)));
    }
    // Everything the chain showed differs from `base` too.
    let chained = published.chain.take().map(|(_, shown)| shown);
    let changed = match &published.base {
        Some(old) if old.dimensions() == size => {
            match (diff(old, &rgba), chained.filter(|r| !empty(*r))) {
                (Some(a), Some(b)) => Some(Some(union(a, b))),
                (Some(r), None) | (None, Some(r)) => Some(Some(r)),
                (None, None) => None,
            }
        }
        _ => Some(None),
    };
    published.base = Some(rgba.clone());
    changed.map(pixels)
}

fn empty((_, _, w, h): (u32, u32, u32, u32)) -> bool {
    w == 0 || h == 0
}

/// The rectangle covering both `a` and `b`; an empty one adds nothing.
fn union(a: (u32, u32, u32, u32), b: (u32, u32, u32, u32)) -> (u32, u32, u32, u32) {
    if empty(a) {
        return b;
    }
    if empty(b) {
        return a;
    }
    let (x0, y0) = (a.0.min(b.0), a.1.min(b.1));
    let (x1, y1) = ((a.0 + a.2).max(b.0 + b.2), (a.1 + a.3).max(b.1 + b.3));
    (x0, y0, x1 - x0, y1 - y0)
}

fn present(session: &EditorSession, artifact_id: &str, published: &mut Published) -> Presented {
    let snapshot = session.snapshot();
    let layers = snapshot
        .document
        .elements
        .iter()
        .rev()
        .map(|element| {
            let base = element.base();
            let row = snapshot.layer_rows.get(base.id.as_str());
            LayerItem {
                id: base.id.clone(),
                name: row.map_or_else(|| editor_chrome::layer_name(element), |r| r.name.clone()),
                kind: row.map_or(editor_chrome::layer_kind(element), |r| r.kind),
                icon: row.map_or(editor_chrome::layer_icon(element), |r| r.icon),
                visible: base.visible,
                locked: base.locked,
                visibility_label: editor_chrome::visibility_label(element),
                lock_label: editor_chrome::lock_label(element),
                menu_label: editor_chrome::menu_label(element),
            }
        })
        .collect();
    let pixels = publish(session.pixels(), None, published);
    Presented {
        document: snapshot.document.clone(),
        layers,
        outlines: snapshot
            .selection_outlines
            .iter()
            .map(|(id, outline)| ((*id).to_owned(), *outline))
            .collect(),
        can_undo: snapshot.can_undo,
        can_redo: snapshot.can_redo,
        style: snapshot.initial_annotation_style.clone(),
        has_draft: snapshot.has_draft,
        can_trim: snapshot.can_trim,
        active_input: snapshot
            .active_text_input
            .as_ref()
            .map(|active| text::ActiveInput {
                input_id: active.input_id.to_owned(),
                layer_id: active.layer_id.to_owned(),
                layout: active.layout.clone(),
            }),
        font_families: snapshot.font_families.cloned().unwrap_or_default(),
        text_size: snapshot.initial_text_size,
        controls: snapshot
            .annotation_controls
            .iter()
            .map(|(id, controls)| {
                let opacity = snapshot
                    .document
                    .elements
                    .iter()
                    .find(|e| e.base().id == *id)
                    .map_or(100., |e| e.base().opacity);
                (
                    (*id).to_owned(),
                    props::Values::from_controls(controls, opacity),
                )
            })
            .collect(),
        source: snapshot.original_export_path.map(|path| ExportSource {
            artifact_id: snapshot
                .source_artifact_id
                .unwrap_or(artifact_id)
                .to_owned(),
            path: path.to_owned(),
        }),
        pixels,
    }
}

fn open_request(root: &std::path::Path, id: &str) -> OpenRequest {
    OpenRequest {
        drafts_root: root.with_file_name("editor-drafts"),
        history_root: root.to_owned(),
        artifact_id: id.to_owned(),
        source_artifact_id: None,
    }
}

/// `CF_DIB`: a bottom-up 32 bpp BGRA bitmap.
fn dib(image: &RgbaImage) -> Vec<u8> {
    let (w, h) = image.dimensions();
    let mut out = Vec::with_capacity(40 + (w * h * 4) as usize);
    for value in [40u32, w, h] {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    for value in [0u32, w * h * 4, 0, 0, 0, 0] {
        out.extend_from_slice(&value.to_le_bytes());
    }
    for row in image.rows().rev() {
        for px in row {
            out.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
        }
    }
    out
}

fn spawn_worker(
    root: PathBuf,
    id: String,
    inbox: Receiver<Job>,
    results: Sender<Outcome>,
    hwnd: isize,
    previous: Vec<std::thread::JoinHandle<()>>,
) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("captures-editor".into())
        .spawn(move || {
            // A just-closed editor for this artifact may still be committing
            // its label and writing the draft this session restores.
            join_until(previous, Instant::now() + WORKER_EXIT_WAIT);
            let mut published = Published::default();
            let send = |outcome| {
                let sent = results.send(outcome).is_ok();
                post(hwnd, WM_APP_OUTCOME);
                sent
            };
            let mut session = match EditorSession::open_with_fonts(
                open_request(&root, &id),
                Some(captures_app::editor_fonts::bundled()),
            ) {
                Ok(opened) => {
                    let first = present(&opened, &id, &mut published);
                    if !send(Outcome::Presented(Box::new(first))) {
                        return;
                    }
                    opened
                }
                Err(error) => {
                    send(Outcome::Failed(error));
                    return;
                }
            };
            while let Ok(job) = inbox.recv() {
                let session = &mut session;
                let outcome = match job {
                    Job::Apply(request) => match session.execute(request) {
                        Ok(()) => {
                            Outcome::Presented(Box::new(present(session, &id, &mut published)))
                        }
                        Err(error) => Outcome::Failed(error),
                    },
                    Job::Save {
                        plan,
                        options,
                        mode,
                    } => Outcome::Saved(
                        editor_export::publish(&root, &session.pixels(), &plan, options, mode)
                            .map(|saved| (plan, saved)),
                    ),
                    Job::Preview(request) => Outcome::Preview(
                        session.preview_drawing_with_geometry(request).map(|frame| {
                            (
                                publish(frame.pixels, frame.damage, &mut published),
                                frame.selection_outlines,
                                frame.guides,
                            )
                        }),
                    ),
                    Job::Refresh => {
                        Outcome::Presented(Box::new(present(session, &id, &mut published)))
                    }
                    Job::Copy => {
                        let pixels = session.pixels();
                        Outcome::Copied(
                            captures_history::encode_png(&pixels)
                                .map(|png| (png, dib(&pixels)))
                                .map_err(|error| error.to_string()),
                        )
                    }
                    Job::Autosave => {
                        if let Err(error) = session.execute(Request::AutosaveDraft {
                            updated_at_ms: now_ms(),
                        }) {
                            eprintln!("draft autosave failed: {error}");
                        }
                        continue;
                    }
                    Job::Flush => {
                        if let Err(error) = session.execute(Request::AutosaveDraft {
                            updated_at_ms: now_ms(),
                        }) {
                            eprintln!("draft flush on close failed: {error}");
                        }
                        Outcome::Flushed
                    }
                };
                // A closed window no longer takes results, but the jobs
                // queued before it closed (its open label's Finish and the
                // draft flush) still run; the loop ends once they are done.
                send(outcome);
            }
        })
        .expect("editor worker")
}

/// The worker's job queue and its replies.
type Worker = (Sender<Job>, Receiver<Outcome>);

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

impl Editor {
    #[allow(clippy::too_many_arguments)]
    fn new(
        hwnd: HWND,
        g: Gfx,
        t: Tokens,
        options: &Options,
        artifact_id: String,
        created_at: &str,
        mode: CaptureMode,
        worker: Option<Worker>,
    ) -> Editor {
        // Only a missing settings file means defaults (see History's Save
        // file): an unreadable one leaves no folder and says why.
        let (output_directory, settings_error) =
            match captures_settings::load(&options.settings_file) {
                Ok(settings) => (settings.output_directory.into(), None),
                Err(error) => (
                    PathBuf::new(),
                    Some(format!("Couldn’t read settings: {error}")),
                ),
            };
        let (jobs, outcomes) = worker.unwrap_or_else(|| {
            let (jobs, _) = channel();
            let (_, outcomes) = channel();
            (jobs, outcomes)
        });
        let mut editor = Editor {
            hwnd,
            g,
            t,
            artifact_id,
            settings_file: options.settings_file.clone(),
            appearance: options.appearance.clone(),
            output_directory,
            settings_error,
            default_stem: captures_app::recording_editor_ui::timestamp_stem(created_at),
            mode,
            scale: 1.,
            reduced_motion: false,
            jobs,
            outcomes,
            presented: None,
            image: None,
            upload: None,
            viewport: Viewport::default(),
            tool: "v",
            last_shape: 0,
            selected: None,
            gesture: None,
            target: None,
            options: ExportOptions {
                format: ExportFormat::Png,
                quality: ExportQuality::Preserve,
                quality_value: 98,
                max_size_bytes: None,
                png: PngOptions::default(),
                size: ExportSize::Original,
            },
            notice: None,
            busy: true,
            banner: false,
            sidebar_pref: None,
            divider_drag: None,
            slider_drag: false,
            flyout: false,
            list_scroll: 0.,
            defaults: None,
            props: Vec::new(),
            props_height: 0.,
            props_scroll: 0.,
            props_clip: 0,
            props_body: 0,
            props_thumb: 0,
            thumb_drag: None,
            thumb_hover: false,
            range_drag: None,
            pending: None,
            edit_seq: 0,
            pick_color: None,
            eraser: props::Eraser::default(),
            crop: None,
            crop_aspect: 0,
            inline: None,
            menu: None,
            menu_node: 0,
            menu_lit: 0,
            menu_press: None,
            field: None,
            field_caret: 0,
            header_caret: 0,
            text_preset: text::NEW_TEXT_PRESET,
            text_size: None,
            input_counter: 0,
            caret: 0,
            caret_blink: false,
            caret_color: None,
            high_surrogate: None,
            ime: true,
            text_drag: false,
            preview_busy: false,
            preview_dirty: false,
            preview_sent: None,
            preview_ready: None,
            preview_timer: false,
            preview_outline: None,
            guides: Vec::new(),
            autosave: DraftAutosave::default(),
            w: SIZE.0,
            h: SIZE.1,
            l: Layout::default(),
            hover: None,
            focus: None,
            focus_visible: false,
            pressed: None,
            pointer: None,
            tracking: false,
            nodes: Vec::new(),
            children: Vec::new(),
            well: 0,
            page: 0,
            image_node: 0,
            overlay: 0,
            layers_heading: 0,
            list: 0,
            properties: 0,
            bar: 0,
            header: 0,
            banner_node: 0,
            zoom: 0,
            thumb: 0,
            flyout_node: 0,
            bg_menu: 0,
            bg_open: false,
            last_solid: editor_chrome::colors::DEFAULT_CANVAS_BACKGROUND.into(),
            controls: Vec::new(),
            rows: 0,
            page_key: (0., 0., 0., 0.),
            overlay_drawn: (None, None),
            overlay_fresh: true,
            motions: Vec::new(),
            dcomp: None,
            _target: None,
            visuals: Vec::new(),
            surfaces: Vec::new(),
            offsets: Vec::new(),
            timer: false,
            need: true,
            relayout: true,
            woken: false,
            events: Vec::new(),
            closing: false,
            close_ready: false,
        };
        editor.build();
        editor
    }

    fn add(&mut self, parent: Option<usize>, kind: Kind, bleed: f32) -> usize {
        let index = self.nodes.len();
        self.nodes.push(Node {
            parent,
            kind,
            rect: Rect::default(),
            bleed,
            opacity: 1.,
            visible: true,
            clip: false,
            dirty: true,
            props: true,
        });
        self.children.push(Vec::new());
        self.visuals.push(None);
        self.surfaces.push(None);
        self.offsets.push((0., 0.));
        if let Some(parent) = parent {
            self.children[parent].push(index);
        }
        index
    }

    fn control(&mut self, parent: usize, target: Target, bleed: f32) -> Layered {
        let base = self.add(Some(parent), Kind::Ctl(target, Layer::Base), bleed);
        let lit = self.add(Some(base), Kind::Ctl(target, Layer::Lit), 0.);
        let ring = self.add(Some(base), Kind::Ctl(target, Layer::Ring), 4.);
        self.nodes[lit].opacity = 0.;
        self.nodes[ring].visible = false;
        let layered = Layered { base, lit, ring };
        self.controls.push((target, layered));
        layered
    }

    fn build(&mut self) {
        let root = self.add(None, Kind::Group, 0.);
        self.well = self.add(Some(root), Kind::Well, 0.);
        self.nodes[self.well].clip = true;
        self.page = self.add(Some(self.well), Kind::Page, PAGE_BLEED);
        self.image_node = self.add(Some(self.well), Kind::Image, 0.);
        self.overlay = self.add(Some(self.well), Kind::Overlay, 0.);
        self.caret = self.add(Some(self.well), Kind::Caret, 0.);
        self.nodes[self.caret].visible = false;
        let rail = self.add(Some(root), Kind::Rail, 0.);
        for i in 0..RAIL_TOOLS.len() {
            self.control(rail, Target::Tool(i), 0.);
        }
        let sidebar = self.add(Some(root), Kind::Sidebar, 0.);
        self.layers_heading = self.add(Some(sidebar), Kind::LayersHeading, 0.);
        self.list = self.add(Some(sidebar), Kind::List, 0.);
        self.nodes[self.list].clip = true;
        self.properties = self.add(Some(sidebar), Kind::Properties, 0.);
        self.props_clip = self.add(Some(self.properties), Kind::Group, 0.);
        self.nodes[self.props_clip].clip = true;
        self.props_body = self.add(Some(self.props_clip), Kind::PropsBody, 0.);
        for prop in props::all() {
            let bleed = if matches!(prop, Prop::Swatch(..)) {
                4.
            } else {
                0.
            };
            let base = self
                .control(self.props_body, Target::Prop(prop), bleed)
                .base;
            // A number field's steppers sit inside it.
            if let Prop::Number(num) = prop {
                for up in [true, false] {
                    self.control(base, Target::Prop(Prop::Step(num, up)), 0.);
                }
            }
        }
        self.field_caret = self.add(Some(self.props_body), Kind::FieldCaret, 0.);
        self.nodes[self.field_caret].visible = false;
        self.props_thumb = self.add(Some(self.properties), Kind::PropsThumb, 0.);
        self.nodes[self.props_thumb].visible = false;
        self.control(root, Target::Divider, 0.);
        self.header = self.add(Some(root), Kind::Header, 0.);
        // DOM order: Canvas W and H (with their steppers), then Trim edges.
        for num in [Num::CanvasWidth, Num::CanvasHeight] {
            let base = self
                .control(self.header, Target::Prop(Prop::Number(num)), 0.)
                .base;
            for up in [true, false] {
                self.control(base, Target::Prop(Prop::Step(num, up)), 0.);
            }
        }
        self.header_caret = self.add(Some(self.header), Kind::FieldCaret, 0.);
        self.nodes[self.header_caret].visible = false;
        self.control(self.header, Target::Trim, 0.);
        self.control(self.header, Target::Background, 0.);
        self.banner_node = self.add(Some(root), Kind::Banner, 0.);
        self.control(self.banner_node, Target::Dismiss, 0.);
        for target in [Target::Undo, Target::Redo] {
            self.control(self.header, target, 0.);
        }
        self.zoom = self.add(Some(self.header), Kind::Zoom, 0.);
        for target in [Target::Fit, Target::ZoomOut, Target::Slider, Target::ZoomIn] {
            self.control(self.zoom, target, 0.);
        }
        let slider = self.layered(Target::Slider).base;
        self.thumb = self.add(Some(slider), Kind::Thumb, 2.);
        self.bar = self.add(Some(root), Kind::Bar, 0.);
        for target in [Target::Copy, Target::SaveAsNew, Target::Save] {
            self.control(self.bar, target, 0.);
        }
        // The flyout floats above the canvas and the sidebar.
        self.flyout_node = self.add(Some(root), Kind::Flyout, 16.);
        self.nodes[self.flyout_node].visible = false;
        for i in 0..SHAPE_TOOLS.len() {
            self.control(self.flyout_node, Target::Shape(i), 0.);
        }
        // The Background color card, portalled after the page content.
        self.bg_menu = self.add(Some(root), Kind::BgMenu, 24.);
        self.nodes[self.bg_menu].visible = false;
        for prop in Self::bg_menu_props() {
            let bleed = if matches!(prop, Prop::Swatch(..)) {
                4.
            } else {
                0.
            };
            self.control(self.bg_menu, Target::Prop(prop), bleed);
        }
        // Property popups float above everything.
        self.menu_node = self.add(Some(root), Kind::Menu, menu::MENU_BLEED);
        self.nodes[self.menu_node].visible = false;
        self.menu_lit = self.add(Some(self.menu_node), Kind::MenuLit, 0.);
        self.nodes[self.menu_lit].visible = false;
    }

    /// Rows for `count` layers.
    fn ensure_rows(&mut self, count: usize) {
        while self.rows < count {
            let i = self.rows;
            let row = self.control(self.list, Target::Layer(i), 0.);
            self.control(row.base, Target::Eye(i), 0.);
            self.control(row.base, Target::Lock(i), 0.);
            self.rows += 1;
        }
    }

    fn layered(&self, target: Target) -> Layered {
        self.controls
            .iter()
            .find(|(t, _)| *t == target)
            .map(|(_, layered)| *layered)
            .expect("control")
    }

    fn wake(&mut self) {
        if self.need && !self.woken && !self.hwnd.is_invalid() {
            self.woken = true;
            unsafe {
                let _ = PostMessageW(Some(self.hwnd), WM_APP_WAKE, WPARAM(0), LPARAM(0));
            }
        }
    }

    // ---- state ----

    pub fn layer_count(&self) -> usize {
        self.presented.as_ref().map_or(0, |p| p.layers.len())
    }

    pub fn layer(&self, index: usize) -> Option<&LayerItem> {
        self.presented.as_ref()?.layers.get(index)
    }

    pub fn exists(&self, target: Target) -> bool {
        match target {
            Target::Dismiss => self.banner,
            Target::Undo | Target::Redo => !self.l.compact,
            Target::Shape(_) => self.flyout,
            Target::Layer(i) | Target::Eye(i) | Target::Lock(i) => i < self.layer_count(),
            // Shipping hides the toggle when the format can only save a copy.
            Target::SaveAsNew => self.export_view().is_some_and(|v| !v.format_requires_copy),
            Target::Prop(prop) => self.prop_exists(prop),
            _ => true,
        }
    }

    pub fn enabled(&self, target: Target) -> bool {
        let p = self.presented.as_ref();
        let percent = self.zoom_percent().unwrap_or(100.);
        match target {
            Target::Undo => p.is_some_and(|p| p.can_undo),
            Target::Trim => p.is_some_and(|p| p.can_trim) && !self.busy,
            Target::Redo => p.is_some_and(|p| p.can_redo),
            Target::ZoomOut => p.is_some() && percent > ZOOM_MIN + 0.05,
            Target::ZoomIn => p.is_some() && percent < ZOOM_MAX - 0.05,
            Target::Fit | Target::Slider | Target::Background => p.is_some(),
            Target::Tool(_) => true,
            Target::Save => {
                self.export_view()
                    .is_some_and(|v| v.plan.is_some() && v.error.is_none())
                    && !self.busy
            }
            Target::SaveAsNew => self.exists(target) && !self.busy,
            Target::Copy => self.image.is_some(),
            Target::Prop(Prop::Step(num, up)) => {
                self.exists(target) && !self.step_disabled(num, up)
            }
            Target::Prop(Prop::Number(num)) => self.exists(target) && !self.num_disabled(num),
            _ => self.exists(target),
        }
    }

    /// Shipping `aria-pressed` / `aria-checked` state.
    pub fn pressed(&self, target: Target) -> Option<bool> {
        Some(match target {
            Target::Tool(i) => self.tool_active(i),
            Target::Shape(i) => SHAPE_TOOLS[i].key == SHAPE_TOOLS[self.last_shape].key,
            Target::Eye(i) => !self.layer(i)?.visible,
            Target::Lock(i) => self.layer(i)?.locked,
            Target::SaveAsNew => self.export_view().is_some_and(|v| v.saving_copy),
            Target::Prop(prop) => return self.prop_pressed(prop),
            _ => return None,
        })
    }

    fn tool_active(&self, index: usize) -> bool {
        let key = RAIL_TOOLS[index].key;
        key == self.tool
            || (key == editor_chrome::SHAPES_KEY && SHAPE_TOOLS.iter().any(|s| s.key == self.tool))
    }

    /// Accessible name, from shipping's `aria-label`s.
    pub fn name(&self, target: Target) -> String {
        match target {
            Target::Dismiss => header::DRAFT_DISMISS_LABEL.into(),
            Target::Trim => header::TRIM.into(),
            Target::Undo => header::UNDO.into(),
            Target::Redo => header::REDO.into(),
            Target::Fit => header::FIT.into(),
            Target::ZoomOut => header::ZOOM_OUT.into(),
            Target::Slider => header::ZOOM_SLIDER.into(),
            Target::ZoomIn => header::ZOOM_IN.into(),
            Target::Tool(i) if RAIL_TOOLS[i].key == editor_chrome::SHAPES_KEY => {
                editor_chrome::SHAPES_LABEL.into()
            }
            Target::Tool(i) => RAIL_TOOLS[i].name(),
            Target::Shape(i) => SHAPE_TOOLS[i].name(),
            Target::Divider => "Resize properties panel".into(),
            Target::Layer(i) => self
                .layer(i)
                .map(|l| format!("{}, {}", l.name, l.kind))
                .unwrap_or_default(),
            Target::Eye(i) => self
                .layer(i)
                .map(|l| l.visibility_label.clone())
                .unwrap_or_default(),
            Target::Lock(i) => self
                .layer(i)
                .map(|l| l.lock_label.clone())
                .unwrap_or_default(),
            Target::Copy => "Copy image".into(),
            Target::SaveAsNew => "Save as new file".into(),
            Target::Save => "Save".into(),
            Target::Prop(prop) => self.prop_name(prop),
            Target::Background => self.background_label(),
        }
    }

    /// Shipping `title` tooltips, as UI Automation help text.
    pub fn help(&self, target: Target) -> String {
        match target {
            Target::Fit => header::FIT_TOOLTIP.into(),
            Target::Trim => header::TRIM_TOOLTIP.into(),
            Target::Background => editor_chrome::colors::BACKGROUND_TOOLTIP.into(),
            Target::Slider => header::ZOOM_SLIDER_TOOLTIP.into(),
            Target::Tool(i) if RAIL_TOOLS[i].key == editor_chrome::SHAPES_KEY => {
                editor_chrome::shapes_tooltip(SHAPE_TOOLS[self.last_shape].key)
            }
            Target::Divider => "Drag to resize · Double-click to reset".into(),
            Target::SaveAsNew => "Save as a new file and leave the original untouched".into(),
            Target::Save => self.export_view().map(|v| v.hint).unwrap_or_default(),
            Target::Prop(prop) => self.prop_help(prop),
            _ => String::new(),
        }
    }

    pub fn stub_name(&self, stub: Stub) -> String {
        match stub {
            Stub::Preset => header::ZOOM_PRESET.into(),
            Stub::AddImages => header::ADD_IMAGES.into(),
            Stub::AddLayer => editor_chrome::layers::ADD.into(),
            Stub::LayerMenu(i) => self
                .layer(i)
                .map(|l| l.menu_label.clone())
                .unwrap_or_default(),
            Stub::ExportSettings => "Export settings".into(),
            Stub::Change => "Change save location".into(),
            Stub::Format => "Format".into(),
        }
    }

    pub fn stub_value(&self, stub: Stub) -> Option<String> {
        self.presented.as_ref()?;
        Some(match stub {
            Stub::Preset => self.zoom_text(),
            _ => return None,
        })
    }

    pub fn stub_rect(&self, stub: Stub) -> Rect {
        let l = &self.l;
        match stub {
            Stub::Preset => l.preset,
            Stub::AddImages => l.add_images,
            Stub::AddLayer => l.add_layer,
            Stub::LayerMenu(i) => {
                let row = self.target_rect(Target::Layer(i));
                let lock = self.target_rect(Target::Lock(i));
                Rect::new(lock.x + QUICK.0 + 2., lock.y, QUICK.0, QUICK.1.min(row.h))
            }
            Stub::ExportSettings => l.export_settings,
            Stub::Change => l.change,
            Stub::Format => l.suffix,
        }
    }

    pub fn export_view(&self) -> Option<editor_export::ExportBarView> {
        let (p, target) = (self.presented.as_ref()?, self.target.as_ref()?);
        Some(editor_export::present(
            target,
            self.options,
            (
                p.document.width.round() as u32,
                p.document.height.round() as u32,
            ),
            p.document.background.is_none(),
            EstimateState::default(),
        ))
    }

    pub fn filename(&self) -> String {
        self.target
            .as_ref()
            .map_or_else(|| self.default_stem.clone(), |t| t.stem.clone())
    }

    /// Shipping `destinationDirectory`.
    pub fn save_location(&self) -> String {
        self.target.as_ref().map_or_else(
            || self.output_directory.display().to_string(),
            |t| t.directory.display().to_string(),
        )
    }

    pub fn hint_text(&self) -> String {
        self.export_view().map(|v| v.hint).unwrap_or_default()
    }

    /// The notice line: export problems first, then the last action's.
    pub fn notice_text(&self) -> Option<(String, bool)> {
        let no_folder = self
            .target
            .as_ref()
            .is_some_and(|t| t.directory.as_os_str().is_empty());
        if let Some(error) = self.settings_error.as_ref().filter(|_| no_folder) {
            return Some((error.clone(), true));
        }
        match self.export_view().and_then(|v| v.error) {
            Some(error) => Some((error, true)),
            None => self.notice.clone(),
        }
    }

    // ---- viewport ----

    fn document_size(&self) -> Option<(f64, f64)> {
        self.presented
            .as_ref()
            .map(|p| (p.document.width, p.document.height))
    }

    /// `commitCanvasSize`: Canvas W or H, rounded and clamped by the
    /// field, as its own undo step (the session drops a no-op).
    pub(super) fn resize_canvas(&mut self, num: Num, value: f64) {
        let Some((width, height)) = self.document_size() else {
            return;
        };
        let (width, height) = match num {
            Num::CanvasWidth => (value, height),
            _ => (width, value),
        };
        if (width, height) == (self.document_size().unwrap_or_default()) {
            return;
        }
        self.notice = None;
        self.submit(Request::ResizeCanvas { width, height });
    }

    /// The fitted rect inside the well (shipping Fit), well-relative.
    fn fit(&self) -> Option<(DocRect, f64, f64)> {
        let (dw, dh) = self.document_size()?;
        let (width, height) = (f64::from(self.l.well.w), f64::from(self.l.well.h));
        let scale = ((width - 2. * FIT_INSET) / dw)
            .max(0.02)
            .min(((height - 2. * FIT_INSET) / dh).max(0.02))
            .min(1.);
        Some((
            DocRect {
                x: (width - dw * scale) / 2.,
                y: (height - dh * scale) / 2.,
                width: dw * scale,
                height: dh * scale,
            },
            dw,
            dh,
        ))
    }

    /// Where the document lands, well-relative DIPs.
    pub fn image_rect(&self) -> Option<DocRect> {
        let (fit, dw, dh) = self.fit()?;
        self.viewport.rect(fit, dw, dh)
    }

    fn to_document(&self, x: f32, y: f32) -> Option<Point> {
        let r = self.image_rect()?;
        let (dw, dh) = self.document_size()?;
        let (x, y) = (f64::from(x - self.l.well.x), f64::from(y - self.l.well.y));
        Some(Point {
            x: (x - r.x) / r.width * dw,
            y: (y - r.y) / r.height * dh,
        })
    }

    /// A document point in well-relative DIPs.
    fn to_well(&self, point: Point) -> Option<(f32, f32)> {
        let r = self.image_rect()?;
        let (dw, dh) = self.document_size()?;
        Some((
            (r.x + point.x / dw * r.width) as f32,
            (r.y + point.y / dh * r.height) as f32,
        ))
    }

    /// Displayed percent of image pixels.
    pub fn zoom_percent(&self) -> Option<f64> {
        let r = self.image_rect()?;
        let (dw, _) = self.document_size()?;
        Some(r.width / dw * 100.)
    }

    pub fn fit_mode(&self) -> bool {
        self.viewport.zoom_percent == 0.
    }

    /// The preset readout: Fit, or the manual zoom.
    pub fn zoom_text(&self) -> String {
        if self.fit_mode() {
            "Fit".into()
        } else {
            editor_chrome::zoom_label(self.viewport.zoom_percent)
        }
    }

    /// Shipping's slider `aria-valuetext`.
    pub fn slider_text(&self) -> String {
        let percent = self.zoom_percent().unwrap_or(100.);
        if self.fit_mode() {
            format!(
                "Fit ({})",
                editor_chrome::zoom_label((percent * 10.).round() / 10.)
            )
        } else {
            editor_chrome::zoom_label(self.viewport.zoom_percent)
        }
    }

    pub fn slider_value(&self) -> f64 {
        let percent = if self.fit_mode() {
            self.zoom_percent().unwrap_or(100.)
        } else {
            self.viewport.zoom_percent
        };
        (percent.clamp(ZOOM_MIN, ZOOM_MAX) * 10.).round() / 10.
    }

    fn set_viewport(&mut self, viewport: Viewport) {
        if viewport == self.viewport {
            return;
        }
        let zoom_changed = viewport.zoom_percent != self.viewport.zoom_percent;
        self.viewport = viewport;
        self.place_canvas();
        if zoom_changed {
            self.events.push(Event::Value(Target::Slider));
            for target in [Target::Fit, Target::ZoomOut, Target::ZoomIn] {
                let base = self.layered(target).base;
                self.nodes[base].dirty = true;
            }
            self.nodes[self.zoom].dirty = true;
            self.refresh(false);
        }
        self.need = true;
    }

    pub fn zoom_to(&mut self, percent: f64, anchor: Option<(f32, f32)>) {
        let Some((fit, dw, dh)) = self.fit() else {
            return;
        };
        let (x, y) = anchor.map_or(
            (f64::from(self.l.well.w) / 2., f64::from(self.l.well.h) / 2.),
            |(x, y)| (f64::from(x - self.l.well.x), f64::from(y - self.l.well.y)),
        );
        let mut viewport = self.viewport;
        if viewport.zoom_percent == 0. {
            viewport.zoom_percent = fit.width / dw * 100.;
        }
        if let Some(next) = viewport.zoom_at(fit, dw, dh, percent, Point { x, y }) {
            self.set_viewport(next);
        }
    }

    fn zoom_by(&mut self, factor: f64) {
        if let Some(percent) = self.zoom_percent() {
            self.zoom_to(percent * factor, None);
        }
    }

    pub fn set_slider(&mut self, position: f64) {
        if let Some(percent) = editor_viewport::zoom_from_slider(position) {
            self.zoom_to(percent, None);
        }
    }

    // ---- sidebar ----

    pub fn sidebar_bounds(&self) -> (f32, f32) {
        let max = SIDEBAR_MIN.max(SIDEBAR_MAX.min(self.w - RAIL_W - 360.));
        (SIDEBAR_MIN, max)
    }

    pub fn sidebar_width(&self) -> f32 {
        let (min, max) = self.sidebar_bounds();
        let default = if self.w <= editor_chrome::COMPACT_WIDTH as f32 {
            280.
        } else {
            320.
        };
        self.sidebar_pref.unwrap_or(default).clamp(min, max)
    }

    /// Shipping `resizeSidebar`, or `None` to reset.
    pub fn resize_sidebar(&mut self, width: Option<f32>) {
        let (min, max) = self.sidebar_bounds();
        let next = width.map(|w| w.clamp(min, max).round());
        if next == self.sidebar_pref {
            return;
        }
        let before = self.sidebar_width();
        self.sidebar_pref = next;
        if self.sidebar_width() != before {
            self.relayout = true;
            self.events.push(Event::Value(Target::Divider));
        }
        self.need = true;
    }

    // ---- tools ----

    pub fn select_tool(&mut self, key: &'static str) {
        if let Some(i) = SHAPE_TOOLS.iter().position(|s| s.key == key) {
            self.last_shape = i;
            let base = self.layered(Target::Tool(shapes_index())).base;
            self.nodes[base].dirty = true;
        }
        if key == self.tool {
            return;
        }
        if self.inline.is_some() {
            self.finish_text();
        }
        self.tool = key;
        self.gesture = None;
        if key != "c" {
            self.crop = None;
        }
        for i in 0..RAIL_TOOLS.len() {
            let base = self.layered(Target::Tool(i)).base;
            self.nodes[base].dirty = true;
            self.events.push(Event::Toggled(Target::Tool(i)));
        }
        for i in 0..SHAPE_TOOLS.len() {
            let base = self.layered(Target::Shape(i)).base;
            self.nodes[base].dirty = true;
        }
        self.range_drag = None;
        self.props_scroll = 0.;
        self.props_changed();
        self.overlay_changed();
        self.refresh(false);
        self.need = true;
    }

    /// `--tool KEY`: start with a rail tool or a grouped shape.
    fn start_tool(&mut self, options: &Options) {
        if let Some(id) = &options.select {
            if self.layer_count() > 0
                && (0..self.layer_count()).any(|i| self.layer(i).is_some_and(|l| l.id == *id))
            {
                self.set_selected(Some(id.clone()));
            } else {
                eprintln!("--select {id}: no such layer");
            }
        }
        let Some(key) = options.tool.as_deref() else {
            return;
        };
        let rail = RAIL_TOOLS
            .iter()
            .enumerate()
            .find(|(_, t)| t.key == key)
            .map(|(_, t)| t.key);
        let shape = SHAPE_TOOLS.iter().find(|t| t.key == key).map(|t| t.key);
        match rail.or(shape) {
            Some(key) => self.select_tool(key),
            None => eprintln!("--tool {key}: not an editor tool here"),
        }
    }

    pub fn set_flyout(&mut self, open: bool) {
        if open == self.flyout {
            return;
        }
        self.flyout = open;
        self.nodes[self.flyout_node].visible = open;
        self.nodes[self.flyout_node].props = true;
        if !open && matches!(self.focus, Some(Target::Shape(_))) {
            self.set_focus(Some(Target::Tool(shapes_index())), true);
        }
        if open {
            // Shipping focuses the checked item.
            self.set_focus(Some(Target::Shape(self.last_shape)), self.focus_visible);
        }
        self.events.push(Event::Structure);
        self.refresh(false);
        self.need = true;
    }

    // ---- actions ----

    fn submit(&mut self, request: Request) {
        self.busy = true;
        let _ = self.jobs.send(Job::Apply(request));
    }

    pub fn activate(&mut self, target: Target) {
        if !self.enabled(target) {
            return;
        }
        match target {
            Target::Dismiss => {
                self.banner = false;
                self.relayout = true;
                self.events.push(Event::Structure);
                if self.focus == Some(Target::Dismiss) {
                    self.focus = None;
                }
            }
            Target::Trim => {
                self.notice = None;
                self.submit(Request::TrimCanvas);
            }
            Target::Background => self.set_bg_menu(!self.bg_open),
            Target::Undo => {
                self.set_crop(None);
                self.submit(Request::Undo)
            }
            Target::Redo => {
                self.set_crop(None);
                self.submit(Request::Redo)
            }
            Target::Fit => self.set_viewport(Viewport::default()),
            Target::ZoomOut => self.zoom_by(1. / KEYBOARD_ZOOM),
            Target::ZoomIn => self.zoom_by(KEYBOARD_ZOOM),
            Target::Slider => {}
            Target::Tool(i) if i == shapes_index() => {
                // Shipping: a grouped shape toggles the flyout; another tool
                // activates the last shape and opens it.
                if SHAPE_TOOLS.iter().any(|s| s.key == self.tool) {
                    self.set_flyout(!self.flyout);
                } else {
                    self.select_tool(SHAPE_TOOLS[self.last_shape].key);
                    self.set_flyout(true);
                }
            }
            Target::Tool(i) => {
                self.set_flyout(false);
                self.select_tool(RAIL_TOOLS[i].key);
            }
            Target::Shape(i) => {
                self.select_tool(SHAPE_TOOLS[i].key);
                self.set_flyout(false);
            }
            Target::Divider => self.resize_sidebar(None),
            Target::Layer(i) => {
                let id = self.layer(i).map(|l| l.id.clone());
                self.set_selected(id);
            }
            Target::Eye(i) => {
                if let Some(layer) = self.layer(i) {
                    let (id, visible) = (layer.id.clone(), layer.visible);
                    self.submit(Request::Layer {
                        id,
                        edit: LayerEdit::Visibility { visible: !visible },
                    });
                }
            }
            Target::Lock(i) => {
                if let Some(layer) = self.layer(i) {
                    let (id, locked) = (layer.id.clone(), layer.locked);
                    self.submit(Request::Layer {
                        id,
                        edit: LayerEdit::Lock { locked: !locked },
                    });
                }
            }
            Target::Copy => {
                let _ = self.jobs.send(Job::Copy);
            }
            Target::SaveAsNew => {
                let on = !self.pressed(Target::SaveAsNew).unwrap_or(false);
                let format = self.options.format;
                if let Some(target) = self.target.as_mut() {
                    target.set_save_as_new(on, format);
                }
                self.export_changed();
                self.events.push(Event::Toggled(Target::SaveAsNew));
            }
            Target::Save => self.save(),
            Target::Prop(prop) => self.activate_prop(prop),
        }
        self.need = true;
    }

    fn save(&mut self) {
        let Some(plan) = self
            .target
            .as_ref()
            .and_then(|t| t.plan(self.options.format).ok())
        else {
            return;
        };
        self.busy = true;
        self.set_notice(Some(("Saving…".into(), false)));
        let _ = self.jobs.send(Job::Save {
            plan,
            options: self.options,
            mode: self.mode,
        });
    }

    fn set_notice(&mut self, notice: Option<(String, bool)>) {
        self.notice = notice;
        self.events.push(Event::Notice);
        self.export_changed();
    }

    /// The export bar and the controls that read its state.
    fn export_changed(&mut self) {
        self.nodes[self.bar].dirty = true;
        for target in [Target::Save, Target::SaveAsNew, Target::Copy] {
            let base = self.layered(target).base;
            self.nodes[base].dirty = true;
        }
        self.refresh(false);
        self.need = true;
    }

    /// The selected layer as the session last presented it.
    fn selected_element(&self) -> Option<captures_app::editor::Element> {
        let id = self.selected.as_deref()?;
        self.presented
            .as_ref()?
            .document
            .elements
            .iter()
            .find(|e| e.base().id == id)
            .cloned()
    }

    fn set_selected(&mut self, id: Option<String>) {
        if id == self.selected {
            return;
        }
        self.selected = id;
        // A property input shows the new selection's values; nothing it
        // held is pending (valid entries apply as they are typed). Canvas W
        // and H don't depend on the selection and keep their draft.
        if self
            .field
            .take_if(|e| !matches!(e.prop, Prop::Number(n) if n.is_canvas()))
            .is_some()
        {
            self.hide_field_carets();
            if matches!(self.focus, Some(Target::Prop(_))) {
                self.focus = None;
            }
        }
        self.close_menu();
        for i in 0..self.rows {
            let base = self.layered(Target::Layer(i)).base;
            self.nodes[base].dirty = true;
        }
        self.range_drag = None;
        self.props_scroll = 0.;
        self.props_changed();
        self.overlay_changed();
        self.need = true;
    }

    fn delete_selected(&mut self) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        let locked = self
            .presented
            .as_ref()
            .is_some_and(|p| p.layers.iter().any(|l| l.id == id && l.locked));
        if !locked {
            self.submit(Request::Layer {
                id,
                edit: LayerEdit::Delete,
            });
        }
    }

    fn receive(&mut self, outcome: Outcome) {
        match outcome {
            Outcome::Presented(mut presented) => {
                self.busy = false;
                let first = self.presented.is_none();
                if self.defaults.is_none() {
                    self.defaults = Some(props::Defaults {
                        style: presented.style.clone(),
                        opacity: 100.,
                    });
                }
                let shown = self.values();
                // Label formatting (Bold, alignment, Style, Font, Size, text)
                // is not in `values`; any change to the selected layer
                // redraws the panel, as after an undo or redo.
                let shown_layer = self.selected_element();
                let old_count = self.layer_count();
                if let Some(pixels) = presented.pixels.take() {
                    self.take_pixels(pixels);
                }
                if first {
                    self.banner = presented.has_draft;
                    self.target = Some(ExportTarget::new(
                        presented.source.clone(),
                        &self.output_directory,
                        &self.default_stem,
                    ));
                } else {
                    self.autosave.edited(Instant::now());
                    self.arm_autosave();
                }
                let count = presented.layers.len();
                let selected = if !first && count > old_count {
                    // A new layer is on top; select it, as shipping does.
                    presented.layers.first().map(|l| l.id.clone())
                } else {
                    self.selected
                        .clone()
                        .filter(|id| presented.layers.iter().any(|l| l.id == *id))
                };
                let size_changed = self.presented.as_ref().is_none_or(|old| {
                    (old.document.width, old.document.height)
                        != (presented.document.width, presented.document.height)
                });
                let background_changed = self
                    .presented
                    .as_ref()
                    .is_none_or(|old| old.document.background != presented.document.background);
                self.presented = Some(*presented);
                self.selected = selected;
                self.ensure_rows(count);
                if count != old_count {
                    self.events.push(Event::Structure);
                }
                for node in &mut self.nodes {
                    if matches!(
                        node.kind,
                        Kind::Ctl(
                            Target::Layer(_)
                                | Target::Eye(_)
                                | Target::Lock(_)
                                | Target::Undo
                                | Target::Redo,
                            _
                        ) | Kind::LayersHeading
                            | Kind::Bar
                    ) {
                        node.dirty = true;
                    }
                }
                if size_changed {
                    // A crop or first frame: the fit and every canvas layer move.
                    self.nodes[self.header].dirty = true;
                    self.relayout = true;
                    for num in [Num::CanvasWidth, Num::CanvasHeight] {
                        self.prop_dirty(Prop::Number(num));
                        for up in [true, false] {
                            self.prop_dirty(Prop::Step(num, up));
                        }
                    }
                    if self.pending.is_none() {
                        self.sync_field();
                    }
                }
                if background_changed {
                    self.follow_background();
                    self.bg_dirty();
                    self.nodes[self.page].dirty = true;
                    self.events
                        .push(Event::Toggled(Target::Prop(Prop::Check(Check::Solid))));
                }
                self.keep_drag();
                if first || self.values() != shown || self.selected_element() != shown_layer {
                    self.props_changed();
                    // A reply that a queued edit already supersedes must not
                    // reset the text being typed.
                    if self.pending.is_none() {
                        self.sync_field();
                    }
                }
                self.overlay_changed();
                self.events.push(Event::Toggled(Target::Undo));
                self.relayout = true;
                if let Some(next) = self.pending.take() {
                    self.submit(next);
                }
                self.received_text();
            }
            Outcome::Preview(result) => {
                self.preview_busy = false;
                let now = Instant::now();
                self.preview_ready = self.preview_sent.map(|sent| now + (now - sent));
                let layer = match &self.gesture {
                    Some(Gesture::Resize { id, .. } | Gesture::Move { id, .. }) => Some(id.clone()),
                    _ => None,
                };
                let previewing = layer.is_some() || matches!(self.gesture, Some(Gesture::Brush(_)));
                match result {
                    // Only a drag still in progress shows its preview; the
                    // committed result replaces it once the worker publishes.
                    Ok((pixels, outlines, guides)) => {
                        if let Some(pixels) = pixels {
                            self.take_pixels(pixels);
                        }
                        if let Some(id) = layer {
                            self.preview_outline = outlines.get(&id).copied();
                            self.guides = guides;
                            self.overlay_changed();
                        }
                    }
                    Err(error) => self.set_notice(Some((error, true))),
                }
                if self.preview_dirty && previewing {
                    self.preview_gesture();
                }
                self.need = true;
                return;
            }
            Outcome::Saved(result) => {
                self.busy = false;
                match result {
                    Ok((plan, saved)) => {
                        let text = editor_export::saved_notice(&plan, &saved);
                        if let Some(target) = self.target.as_mut() {
                            target.record_saved(&saved);
                        }
                        self.set_notice(Some((text, false)));
                    }
                    Err(error) => self.set_notice(Some((error, true))),
                }
            }
            Outcome::Copied(result) => match result.and_then(|(png, dib)| {
                set_clipboard(self.hwnd, &png, &dib).map_err(|e| e.message().to_string())
            }) {
                Ok(()) => self.set_notice(Some(("Copied image".into(), false))),
                Err(error) => self.set_notice(Some((format!("Couldn’t copy: {error}"), true))),
            },
            Outcome::Flushed => self.close_ready = self.closing,
            Outcome::Failed(error) => {
                self.busy = false;
                self.set_notice(Some((error, true)));
                if let Some(next) = self.pending.take() {
                    self.submit(next);
                }
                self.text_failed();
            }
        }
        self.export_changed();
        self.need = true;
    }

    /// Queue new pixels for the image surface.
    /// Patch new pixels into `image` and widen the pending upload to them.
    fn take_pixels(&mut self, pixels: Pixels) {
        self.need = true;
        let damage = patch(&mut self.image, pixels);
        self.upload = Some(self.upload.map_or(damage, |pending| union(pending, damage)));
    }

    fn arm_autosave(&mut self) {
        if let Some(due) = self.autosave.deadline()
            && !self.hwnd.is_invalid()
            && !self.closing
        {
            let ms = due.saturating_duration_since(Instant::now()).as_millis() as u32;
            unsafe { SetTimer(Some(self.hwnd), TIMER_AUTOSAVE, ms.max(1), None) };
        }
    }

    /// The close button, Alt+F4 or the system menu. Shipping commits an
    /// open label and flushes the draft before the window goes; the window
    /// hides at once and is destroyed when the flush reports back, or after
    /// `CLOSE_FLUSH_MS` (the worker still finishes the queue then).
    fn request_close(&mut self) {
        if self.closing {
            return;
        }
        self.queue_close();
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
            SetTimer(Some(self.hwnd), TIMER_CLOSE, CLOSE_FLUSH_MS, None);
        }
    }

    /// Queue, behind every accepted edit, the open label's commit and the
    /// final draft flush. Autosave skips while a label is open, so without
    /// the commit a label typed just before closing would be lost.
    fn queue_close(&mut self) {
        if self.closing {
            return;
        }
        self.closing = true;
        self.gesture = None;
        if !self.hwnd.is_invalid() {
            unsafe {
                let _ = KillTimer(Some(self.hwnd), TIMER_AUTOSAVE);
            }
        }
        // The newest coalesced live edit, then the label, then the draft.
        if let Some(next) = self.pending.take() {
            let _ = self.jobs.send(Job::Apply(next));
        }
        self.queue_text_close();
        let _ = self.jobs.send(Job::Flush);
    }

    /// `WM_ENDSESSION`: the process may end once this returns, so wait,
    /// bounded, for the flush here.
    fn flush_now(&mut self) {
        self.queue_close();
        let deadline = Instant::now() + WORKER_EXIT_WAIT;
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            match self.outcomes.recv_timeout(left) {
                Ok(Outcome::Flushed) | Err(_) => break,
                Ok(_) => {}
            }
        }
    }

    // ---- gestures ----

    fn begin(&mut self, x: f32, y: f32, double: bool) {
        if self.inline.is_some() {
            // Inside the label places the caret; anywhere else commits it.
            if self.in_text(x, y) {
                self.text_click(x, y, double, self.shift_down());
            } else {
                self.finish_text();
            }
            return;
        }
        let Some(point) = self.to_document(x, y) else {
            return;
        };
        if let Some(handle) = self.handle_at(point)
            && let Some(id) = self.selected.clone()
        {
            self.gesture = Some(Gesture::Resize {
                id,
                handle,
                start: point,
                end: point,
            });
            self.preview_outline = None;
            self.overlay_changed();
            return;
        }
        let tool = self.tool;
        if tool == "t" {
            self.begin_text(point);
            return;
        }
        if tool == "b" {
            self.begin_eraser(point);
            return;
        }
        if tool == "c" {
            self.begin_crop(point);
            self.overlay_changed();
            return;
        }
        if tool == "v" && double && self.edit_text_at(point) {
            return;
        }
        let gesture = match tool {
            "v" => {
                let scale = self.zoom_percent().unwrap_or(100.) / 100.;
                let hit = self.presented.as_ref().and_then(|p| {
                    p.document
                        .hit_test(point, 4. / scale)
                        .ok()
                        .flatten()
                        .filter(|element| !element.base().locked)
                        .map(|element| element.base().id.clone())
                });
                self.set_selected(hit.clone());
                match hit {
                    Some(id) => Gesture::Move {
                        id,
                        start: point,
                        end: point,
                    },
                    None => Gesture::Pan {
                        from: self.viewport,
                        x,
                        y,
                    },
                }
            }
            "p" => Gesture::Freehand(vec![point]),
            "a" => Gesture::Shape {
                tool,
                start: point,
                end: point,
            },
            key if SHAPE_TOOLS.iter().any(|s| s.key == key) => Gesture::Shape {
                tool: key,
                start: point,
                end: point,
            },
            _ => return,
        };
        self.preview_outline = None;
        self.guides.clear();
        self.gesture = Some(gesture);
        self.overlay_changed();
    }

    fn update(&mut self, x: f32, y: f32) {
        let point = self.to_document(x, y);
        match self.gesture.as_mut() {
            Some(Gesture::Pan { from, x: x0, y: y0 }) => {
                let mut viewport = *from;
                viewport.pan_x += f64::from(x - *x0);
                viewport.pan_y += f64::from(y - *y0);
                self.set_viewport(viewport);
                return;
            }
            Some(Gesture::Crop(_)) => {
                if let Some(point) = point {
                    self.update_crop(point);
                }
            }
            Some(Gesture::Shape { end, .. }) => {
                if let Some(point) = point {
                    *end = point;
                }
            }
            Some(Gesture::Move { start, end, .. }) => {
                if let Some(point) = point {
                    *end = point;
                }
                // A click selects without moving (shipping's `didMove`).
                let distance = (end.x - start.x).hypot(end.y - start.y);
                let scale = self.zoom_percent().unwrap_or(100.) / 100.;
                if distance * scale >= DRAG_MIN || self.preview_outline.is_some() {
                    self.preview_gesture();
                }
            }
            Some(Gesture::Resize { end, .. }) => {
                if let Some(point) = point {
                    *end = point;
                }
                self.preview_gesture();
            }
            Some(Gesture::Brush(points)) => {
                if let Some(point) = point {
                    points.push(point);
                }
                self.preview_gesture();
            }
            Some(Gesture::Freehand(points)) => {
                if let Some(point) = point {
                    points.push(point);
                }
            }
            None => return,
        }
        self.overlay_changed();
    }

    fn finish(&mut self) {
        let Some(gesture) = self.gesture.take() else {
            return;
        };
        self.guides.clear();
        let scale = self.zoom_percent().unwrap_or(100.) / 100.;
        let moved = |a: Point, b: Point| (b.x - a.x).hypot(b.y - a.y) * scale >= DRAG_MIN;
        let (style, opacity) = self
            .defaults
            .as_ref()
            .map(|d| (d.style.clone(), d.opacity))
            .unwrap_or_else(|| (ElementStyle::default(), 100.));
        let lock = self.shift_down();
        let request = match gesture {
            Gesture::Pan { .. } => None,
            Gesture::Resize {
                id,
                handle,
                start,
                end,
            } => {
                if moved(start, end) {
                    Some(self.resize_request(&id, handle, end, lock))
                } else {
                    None
                }
            }
            // Commit what the preview showed: shipping's snapped move.
            Gesture::Move { id, start, end } if moved(start, end) => Some(Request::Layer {
                id,
                edit: LayerEdit::DragMove {
                    delta_x: end.x - start.x,
                    delta_y: end.y - start.y,
                    display_scale: scale,
                },
            }),
            Gesture::Move { .. } => {
                // Back under the click threshold: drop any preview.
                if self.preview_outline.take().is_some() {
                    let _ = self.jobs.send(Job::Refresh);
                }
                None
            }
            Gesture::Freehand(points) if points.len() >= 2 => Some(Request::CreateFreehandPath {
                create: FreehandPathCreate {
                    points,
                    style,
                    opacity,
                },
            }),
            Gesture::Freehand(_) => None,
            Gesture::Brush(points) => self.brush_request(points),
            Gesture::Shape { start, end, .. } if !moved(start, end) => None,
            // The rectangle stays staged for Apply crop.
            Gesture::Crop(_) => None,
            Gesture::Shape { tool, start, end } => match closed_kind(tool) {
                Some(shape) => Some(Request::CreateClosedShape {
                    create: ClosedShapeCreate {
                        shape,
                        start,
                        end,
                        style,
                        opacity,
                    },
                }),
                None => open_kind(tool).map(|shape| Request::CreateOpenShape {
                    create: OpenShapeCreate {
                        shape,
                        start,
                        end,
                        style,
                        opacity,
                    },
                }),
            },
        };
        if let Some(request) = request {
            self.notice = None;
            self.submit(request);
        }
        self.overlay_changed();
    }

    fn overlay_changed(&mut self) {
        self.nodes[self.overlay].dirty = true;
        if self.inline.is_some() {
            self.place_caret();
        }
        self.need = true;
    }

    fn shift_down(&self) -> bool {
        !self.hwnd.is_invalid() && unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0
    }

    /// Drop a gesture; a resize that previewed restores the real pixels.
    fn cancel_gesture(&mut self) -> bool {
        let Some(gesture) = self.gesture.take() else {
            return false;
        };
        self.guides.clear();
        if matches!(
            gesture,
            Gesture::Resize { .. } | Gesture::Brush(_) | Gesture::Move { .. }
        ) {
            self.preview_outline = None;
            let _ = self.jobs.send(Job::Refresh);
        }
        self.overlay_changed();
        true
    }

    // ---- layout ----

    pub fn layout(&mut self, w: f32, h: f32) -> Result<()> {
        self.w = w;
        self.h = h;
        let t = self.t.clone();
        let g = self.g.clone();
        let n = |name: &str| t.n(name);
        let (s2, s3, s4, s5) = (n("s-2"), n("s-3"), n("s-4"), n("s-5"));
        let mut l = Layout {
            compact: w <= editor_chrome::COMPACT_WIDTH as f32,
            ..Layout::default()
        };
        let header_layout = editor_chrome::header_layout(f64::from(w));

        // `.screenshot-editor-draft-banner`.
        let banner_h = if self.banner {
            let text = g.text(&t, Style::Button, header::DRAFT_RESTORED, 1e4)?;
            (2. * s3 + text.height.max(n("h-sm")) + 1.).round()
        } else {
            0.
        };
        l.banner = Rect::new(0., 0., w, banner_h);
        l.header = Rect::new(0., banner_h, w, HEADER_H);
        let body = banner_h + HEADER_H;
        let sidebar_w = self.sidebar_width();
        l.bar = Rect::new(0., (h - BAR_H).max(body), w, BAR_H);
        let body_h = (l.bar.y - body).max(0.);
        l.rail = Rect::new(0., body, RAIL_W, body_h);
        l.sidebar = Rect::new(w - sidebar_w, body, sidebar_w, body_h);
        l.well = Rect::new(RAIL_W, body, (w - RAIL_W - sidebar_w).max(0.), body_h);
        let layers_h = (body_h * 0.4).max(188.).min(body_h);
        l.layers = Rect::new(l.sidebar.x, body, sidebar_w, layers_h);
        l.properties = Rect::new(l.sidebar.x, body + layers_h, sidebar_w, body_h - layers_h);
        l.list = Rect::new(
            l.sidebar.x + 1. + s4,
            body + HEADING_H + s3,
            sidebar_w - 1. - 2. * s4,
            (layers_h - HEADING_H - s3 - s5).max(0.),
        );
        l.add_layer = Rect::new(w - s4 - 30., body + (HEADING_H - 30.) / 2., 30., 30.);

        let mid = banner_h + (editor_chrome::HEADER_HEIGHT as f32 - CONTROL_H) / 2.;
        let mut buttons = [Rect::default(); SLOTS];
        let mut set = |target: Target, r: Rect| buttons[button_slot(target)] = r;

        // Header left: `.screenshot-canvas-toolbar`.
        let label = g.text(&t, Style::Status, header::CANVAS, 1e4)?;
        let dim = g.text(&t, Style::Status, "W", 1e4)?;
        let times = g.text(&t, Style::Small, "×", 1e4)?;
        let trim = g.text(&t, Style::Button, header::TRIM, 1e4)?;
        let background = g.text(&t, Style::Button, editor_chrome::colors::BACKGROUND, 1e4)?;
        let mut x = s5 + 1. + 3.;
        x += s4 + label.width + s3 + 2.;
        let dim_w = dim.width + s2 + CANVAS_FIELD;
        l.width_box = Rect::new(x + dim.width + s2, mid + 4., CANVAS_FIELD, 28.);
        x += dim_w + 2. + times.width + 2.;
        l.height_box = Rect::new(x + dim.width + s2, mid + 4., CANVAS_FIELD, 28.);
        x += dim_w + 2. + s2 + 1. + s2 + 2.;
        l.trim = Rect::new(x, mid + 4., 2. + 2. * s4 + 13. + s3 + trim.width, 28.);
        set(Target::Trim, l.trim);
        x += l.trim.w + 2.;
        l.background = Rect::new(
            x,
            mid + 4.,
            2. + s4 + 14. + s3 + background.width + s3 + 11. + s3,
            28.,
        );
        set(Target::Background, l.background);
        x += l.background.w + 3. + 1.;
        l.canvas_group = Rect::new(s5, mid, x - s5, CONTROL_H);

        // Header right: undo/redo, zoom group, Add images.
        let add = g.text(&t, Style::Button, header::ADD_IMAGES, 1e4)?;
        let add_w = 2. + 2. * s5 + 16. + s3 + add.width;
        l.add_images = Rect::new(w - s5 - add_w, mid, add_w, CONTROL_H);
        let slider_w = header_layout.zoom_slider_width as f32;
        let preset_w = header_layout.zoom_preset_width as f32;
        let zoom_w = 2. + 3. * ZOOM_BUTTON + slider_w + preset_w;
        l.zoom = Rect::new(l.add_images.x - s2 - s2 - zoom_w, mid, zoom_w, CONTROL_H);
        let mut zx = 1.;
        for (target, width) in [
            (Target::Fit, ZOOM_BUTTON),
            (Target::ZoomOut, ZOOM_BUTTON),
            (Target::Slider, slider_w),
            (Target::ZoomIn, ZOOM_BUTTON),
        ] {
            set(target, Rect::new(zx, 1., width, CONTROL_H - 2.));
            zx += width;
        }
        l.preset = Rect::new(l.zoom.x + zx, mid + 1., preset_w, CONTROL_H - 2.);
        let redo_x = l.zoom.x - s2 - s2 - CONTROL_H;
        set(Target::Redo, Rect::new(redo_x, mid, CONTROL_H, CONTROL_H));
        set(
            Target::Undo,
            Rect::new(redo_x - s2 - CONTROL_H, mid, CONTROL_H, CONTROL_H),
        );
        l.compact = !header_layout.show_history;

        // Banner Dismiss.
        let dismiss = g.text(&t, Style::Button, header::DRAFT_DISMISS, 1e4)?;
        let dismiss_w = 2. + 2. * s4 + dismiss.width;
        set(
            Target::Dismiss,
            Rect::new(
                w - s5 - dismiss_w,
                ((banner_h - 1. - n("h-sm")) / 2.).round(),
                dismiss_w,
                n("h-sm"),
            ),
        );

        // Rail.
        let rail_x = ((RAIL_W - 1. - RAIL_BUTTON) / 2.).round();
        for i in 0..RAIL_TOOLS.len() {
            set(
                Target::Tool(i),
                Rect::new(
                    rail_x,
                    body + s4 + i as f32 * (RAIL_BUTTON + n("s-1")),
                    RAIL_BUTTON,
                    RAIL_BUTTON,
                ),
            );
        }
        // Flyout: right of the Shapes button, its first row level with it.
        let shapes = Rect::new(
            rail_x,
            body + s4 + shapes_index() as f32 * (RAIL_BUTTON + n("s-1")),
            RAIL_BUTTON,
            RAIL_BUTTON,
        );
        let cell = editor_chrome::SHAPE_FLYOUT_BUTTON as f32;
        let columns = editor_chrome::SHAPE_FLYOUT_COLUMNS;
        let rows = SHAPE_TOOLS.len().div_ceil(columns);
        let fw = 2. + 2. * s3 + columns as f32 * cell + (columns - 1) as f32 * s2;
        let fh = 2. + 2. * s3 + rows as f32 * cell + (rows - 1) as f32 * s2;
        l.flyout = Rect::new(shapes.x + shapes.w + 10., shapes.y - s3 - 1., fw, fh);

        // Divider over the sidebar edge.
        set(
            Target::Divider,
            Rect::new(l.sidebar.x - DIVIDER_W / 2., body, DIVIDER_W, body_h),
        );

        // Export bar: `.screenshot-export-main`.
        let top = l.bar.y + 1. + s4;
        l.export_settings = Rect::new(s5, top + 16., 210., 44.);
        let fx = l.export_settings.x + l.export_settings.w + s5;
        let fw = 390.;
        l.filename = Rect::new(fx, top + 24., fw, 36.);
        l.suffix = Rect::new(fx + fw - 62., top + 25., 61., 34.);
        let change = g.text(&t, Style::Tiny, "Change…", 1e4)?;
        l.change = Rect::new(
            fx + fw - change.width - 2. * s3,
            top,
            change.width + 2. * s3,
            20.,
        );
        l.folder = Rect::new(fx, top, fw, 20.);
        let copy = g.text(&t, Style::Button, "Copy image", 1e4)?;
        let copy_w = 2. + 2. * s5 + 15. + s3 + copy.width;
        set(
            Target::Copy,
            Rect::new(fx + fw + s5, top + 24., copy_w, 36.),
        );
        let save = g.text(&t, Style::Strong, "Save", 1e4)?;
        let save_w = (2. + 2. * n("s-8") + 15. + s3 + save.width).max(106.);
        set(
            Target::Save,
            Rect::new(w - s5 - save_w, top + 24., save_w, 36.),
        );
        let as_new = g.text(&t, Style::Status, "Save as new file", 1e4)?;
        let switch_w = SWITCH.0 + s3 + as_new.width;
        // Without the toggle the hint runs up to Save.
        let sx = if self.exists(Target::SaveAsNew) {
            w - s5 - save_w - s5 - 1. - s5 - switch_w
        } else {
            w - s5 - save_w
        };
        set(
            Target::SaveAsNew,
            Rect::new(sx - s2, top + 24., switch_w + 2. * s2, 36.),
        );
        l.switch_label = Rect::new(sx + SWITCH.0 + s3, top + 24., as_new.width, 36.);
        let copy_end = fx + fw + s5 + copy_w + s5;
        l.hint = Rect::new(copy_end, top, (sx - s5 - copy_end).max(0.), 60.);

        l.buttons = buttons;
        self.l = l;
        self.layout_props(l.properties.w);
        self.place();
        Ok(())
    }

    /// Put every node at its laid-out place.
    fn place(&mut self) {
        let l = self.l;
        let root_rects = [
            (self.banner_node, l.banner, self.banner),
            (self.header, l.header, true),
            (self.well, l.well, true),
            (self.bar, l.bar, true),
        ];
        for (node, r, visible) in root_rects {
            self.set_rect(node, r);
            self.set_visible(node, visible);
        }
        let rail = self.nodes[self.layered(Target::Tool(0)).base]
            .parent
            .expect("rail");
        self.set_rect(rail, l.rail);
        let sidebar = self.nodes[self.layers_heading].parent.expect("sidebar");
        self.set_rect(sidebar, l.sidebar);
        let local = |r: Rect, o: Rect| Rect::new(r.x - o.x, r.y - o.y, r.w, r.h);
        self.set_rect(
            self.layers_heading,
            Rect::new(0., 0., l.sidebar.w, HEADING_H),
        );
        self.set_rect(self.list, local(l.list, l.sidebar));
        self.set_rect(self.properties, local(l.properties, l.sidebar));
        let view = self.props_view();
        self.props_scroll = self
            .props_scroll
            .min((self.props_height - view.h).max(0.))
            .max(0.);
        self.set_rect(
            self.props_clip,
            Rect::new(0., view.y - l.properties.y, view.w, view.h),
        );
        self.set_rect(
            self.props_body,
            Rect::new(0., -self.props_scroll, view.w, self.props_height.max(1.)),
        );
        let has_props = !self.props.is_empty();
        self.set_visible(self.props_body, has_props);
        self.place_props_thumb();
        self.set_rect(self.zoom, local(l.zoom, l.header));
        self.set_rect(self.flyout_node, l.flyout);
        let bg_menu = self.bg_menu_rect();
        self.set_rect(self.bg_menu, bg_menu);
        for (target, layered) in self.controls.clone() {
            let parent = self.nodes[layered.base].parent.expect("parent");
            let origin = match self.nodes[parent].kind {
                Kind::Rail => l.rail,
                Kind::Header => l.header,
                Kind::Banner => l.banner,
                Kind::Bar => l.bar,
                Kind::Flyout => l.flyout,
                Kind::BgMenu => Rect::default(),
                Kind::Zoom | Kind::List | Kind::Ctl(..) => Rect::default(),
                _ => Rect::default(),
            };
            let r = match target {
                Target::Layer(_) | Target::Eye(_) | Target::Lock(_) => self.row_part(target),
                Target::Shape(i) => {
                    let (s2, s3) = (self.t.n("s-2"), self.t.n("s-3"));
                    let cell = editor_chrome::SHAPE_FLYOUT_BUTTON as f32;
                    let columns = editor_chrome::SHAPE_FLYOUT_COLUMNS;
                    Rect::new(
                        1. + s3 + (i % columns) as f32 * (cell + s2),
                        1. + s3 + (i / columns) as f32 * (cell + s2),
                        cell,
                        cell,
                    )
                }
                Target::Fit | Target::ZoomOut | Target::Slider | Target::ZoomIn => {
                    l.buttons[button_slot(target)]
                }
                Target::Prop(prop) => self.prop_rect(prop).unwrap_or_default(),
                _ => local(l.buttons[button_slot(target)], origin),
            };
            self.set_rect(layered.base, r);
            let inner = Rect::new(0., 0., r.w, r.h);
            self.set_rect(layered.lit, inner);
            self.set_rect(layered.ring, inner);
            let shown = self.exists(target);
            self.set_visible(layered.base, shown);
        }
        self.place_thumb();
        self.place_canvas();
        // A focused input whose control went away (another section) ends.
        if let Some(prop) = self.field.as_ref().map(|e| e.prop)
            && !self.prop_exists(prop)
        {
            self.field = None;
            self.hide_field_carets();
        }
        self.field_relayout();
        self.place_menu();
    }

    /// A Layers row (list-relative, scrolled) or its toggle (row-relative).
    fn row_part(&self, target: Target) -> Rect {
        let w = self.l.list.w - 4.;
        let s3 = self.t.n("s-3");
        match target {
            Target::Layer(i) => Rect::new(
                2.,
                2. + i as f32 * (ROW_H + ROW_GAP) - self.list_scroll,
                w,
                ROW_H,
            ),
            Target::Eye(_) => Rect::new(
                w - s3 - 3. * QUICK.0 - 4.,
                (ROW_H - QUICK.1) / 2.,
                QUICK.0,
                QUICK.1,
            ),
            Target::Lock(_) => Rect::new(
                w - s3 - 2. * QUICK.0 - 2.,
                (ROW_H - QUICK.1) / 2.,
                QUICK.0,
                QUICK.1,
            ),
            _ => Rect::default(),
        }
    }

    fn place_thumb(&mut self) {
        let slider = self.l.buttons[button_slot(Target::Slider)];
        let s4 = self.t.n("s-4");
        let position =
            editor_viewport::zoom_slider_position(self.slider_value()).unwrap_or(0.) as f32;
        let track = (slider.w - 2. * s4 - 12.).max(0.);
        let r = Rect::new(s4 + track * position, (slider.h - 12.) / 2., 12., 12.);
        self.set_rect(self.thumb, r);
    }

    /// The page, image and overlay for the current viewport.
    fn place_canvas(&mut self) {
        let well = Rect::new(0., 0., self.l.well.w, self.l.well.h);
        self.set_rect(self.overlay, well);
        self.place_thumb();
        let Some(r) = self.image_rect() else {
            self.set_visible(self.page, false);
            self.set_visible(self.image_node, false);
            return;
        };
        let image = Rect::new(r.x as f32, r.y as f32, r.width as f32, r.height as f32);
        self.set_visible(self.image_node, true);
        self.set_visible(self.page, true);
        let n = &mut self.nodes[self.image_node];
        n.rect = image;
        n.props = true;
        // The page covers the visible part of the image (its surface stays
        // window-sized at any zoom); it redraws only when that part changes
        // shape relative to the image.
        let (x0, y0) = (image.x.max(-PAGE_BLEED), image.y.max(-PAGE_BLEED));
        let (x1, y1) = (
            (image.x + image.w).min(well.w + PAGE_BLEED),
            (image.y + image.h).min(well.h + PAGE_BLEED),
        );
        let page = Rect::new(x0, y0, (x1 - x0).max(1.), (y1 - y0).max(1.));
        let key = (x0 - image.x, y0 - image.y, page.w, page.h);
        let n = &mut self.nodes[self.page];
        n.rect = page;
        n.props = true;
        if key != self.page_key {
            self.page_key = key;
            n.dirty = true;
        }
        self.overlay_changed();
        self.need = true;
    }

    fn set_rect(&mut self, node: usize, r: Rect) {
        let n = &mut self.nodes[node];
        if n.rect != r {
            if (n.rect.w, n.rect.h) != (r.w, r.h) {
                n.dirty = true;
            }
            n.rect = r;
            n.props = true;
        }
    }

    fn set_visible(&mut self, node: usize, visible: bool) {
        let n = &mut self.nodes[node];
        if n.visible != visible {
            n.visible = visible;
            n.props = true;
            n.dirty = true;
        }
    }

    /// A target's rect in client DIPs.
    pub fn target_rect(&self, target: Target) -> Rect {
        let Some((_, layered)) = self.controls.iter().find(|(t, _)| *t == target) else {
            return Rect::default();
        };
        self.absolute(layered.base)
    }

    pub fn absolute(&self, node: usize) -> Rect {
        let mut r = self.nodes[node].rect;
        let mut parent = self.nodes[node].parent;
        while let Some(p) = parent {
            r.x += self.nodes[p].rect.x;
            r.y += self.nodes[p].rect.y;
            parent = self.nodes[p].parent;
        }
        r
    }

    // ---- interaction ----

    fn curve(&self) -> CubicBezier {
        let [x1, y1, x2, y2] = self.t.easing("ease-standard");
        CubicBezier::new(x1, y1, x2, y2).unwrap_or(CubicBezier::LINEAR)
    }

    fn fade(&mut self, node: usize, to: f32) {
        let now = Instant::now();
        let from = self
            .motions
            .iter()
            .find(|m| m.node == node)
            .map_or(self.nodes[node].opacity, |m| m.value_at(m.progress(now)));
        self.motions.retain(|m| m.node != node);
        self.nodes[node].opacity = to;
        self.nodes[node].props = true;
        if self.reduced_motion || (from - to).abs() < 1e-4 || self.hwnd.is_invalid() {
            return;
        }
        let seconds = self.t.n("dur-2") / 1000.;
        let curve = self.curve();
        self.motions.push(Motion {
            node,
            from,
            to,
            start: now,
            seconds,
            curve,
            applied: false,
            fresh: true,
        });
    }

    /// Hover layers, focus rings and disabled fades for the current state.
    fn refresh(&mut self, animate: bool) {
        for (target, layered) in self.controls.clone() {
            let enabled = self.enabled(target);
            let dragging = target == Target::Divider && self.divider_drag.is_some();
            let lit = enabled
                && (self.hover == Some(target)
                    || dragging
                    || (target == Target::Divider
                        && self.focus_visible
                        && self.focus == Some(target)));
            let to = f32::from(lit);
            if animate {
                if (self.nodes[layered.lit].opacity - to).abs() > 1e-4 {
                    self.fade(layered.lit, to);
                }
            } else if self.nodes[layered.lit].opacity != to {
                self.motions.retain(|m| m.node != layered.lit);
                self.nodes[layered.lit].opacity = to;
                self.nodes[layered.lit].props = true;
            }
            // Inputs and selects show their focus ring for any focus
            // (`:focus`, `:focus-within`), other controls for keyboard focus.
            let input = matches!(
                target,
                Target::Prop(Prop::Font | Prop::Aspect | Prop::Number(_) | Prop::Textarea)
            );
            let ring = (self.focus_visible || input)
                && self.focus == Some(target)
                && target != Target::Divider;
            if self.nodes[layered.ring].visible != ring {
                self.nodes[layered.ring].visible = ring;
                self.nodes[layered.ring].props = true;
            }
            let faded = match target {
                Target::Undo | Target::Redo | Target::Fit | Target::ZoomOut | Target::ZoomIn => {
                    0.32
                }
                Target::Tool(_) => 0.4,
                // A stepper draws its own disabled ink.
                Target::Prop(Prop::Step(..)) => 1.,
                _ => 0.45,
            };
            let opacity = if enabled { 1. } else { faded };
            if self.nodes[layered.base].opacity != opacity {
                self.nodes[layered.base].opacity = opacity;
                self.nodes[layered.base].props = true;
            }
        }
    }

    /// The control under a client point.
    pub fn hit(&self, x: f32, y: f32) -> Option<Target> {
        let inside =
            |target: Target| self.exists(target) && self.target_rect(target).contains(x, y);
        if self.flyout {
            if let Some(i) = (0..SHAPE_TOOLS.len()).find(|&i| inside(Target::Shape(i))) {
                return Some(Target::Shape(i));
            }
            if self.l.flyout.contains(x, y) {
                return None;
            }
        }
        // The open Background card takes the points over it.
        if self.bg_open && self.bg_menu_rect().contains(x, y) {
            return Self::bg_menu_props().map(Target::Prop).find(|&t| inside(t));
        }
        if inside(Target::Divider) {
            return Some(Target::Divider);
        }
        if self.l.list.contains(x, y) {
            for i in 0..self.layer_count() {
                for target in [Target::Eye(i), Target::Lock(i), Target::Layer(i)] {
                    if inside(target) {
                        return Some(target);
                    }
                }
            }
            return None;
        }
        let in_props = self.props_view().contains(x, y);
        let header_prop = |t: Target| match t {
            Target::Prop(Prop::Number(n) | Prop::Step(n, _)) => n.is_canvas(),
            Target::Prop(prop) => Self::in_bg_menu(prop),
            _ => false,
        };
        // Steppers sit inside their number field.
        let step = self
            .controls
            .iter()
            .map(|(t, _)| *t)
            .filter(|t| matches!(t, Target::Prop(Prop::Step(..))))
            .find(|&t| (in_props || header_prop(t)) && inside(t));
        if step.is_some() {
            return step;
        }
        self.controls
            .iter()
            .map(|(t, _)| *t)
            .filter(|t| !matches!(t, Target::Layer(_) | Target::Eye(_) | Target::Lock(_)))
            .filter(|&t| in_props || header_prop(t) || !matches!(t, Target::Prop(_)))
            .find(|&t| inside(t))
    }

    fn set_pointer(&mut self, pointer: Option<(f32, f32)>) {
        self.pointer = pointer;
        let hover = pointer.and_then(|(x, y)| self.hit(x, y));
        if hover != self.hover {
            self.hover = hover;
            self.refresh(true);
            self.need = true;
        }
    }

    /// Tab order, in DOM order, skipping disabled controls.
    pub fn focusable(&self) -> Vec<Target> {
        let mut list = Vec::new();
        if self.flyout {
            list.extend((0..SHAPE_TOOLS.len()).map(Target::Shape));
            return list;
        }
        if self.banner {
            list.push(Target::Dismiss);
        }
        list.extend([
            Target::Prop(Prop::Number(Num::CanvasWidth)),
            Target::Prop(Prop::Number(Num::CanvasHeight)),
            Target::Trim,
            Target::Background,
            Target::Undo,
            Target::Redo,
            Target::Fit,
            Target::ZoomOut,
            Target::Slider,
            Target::ZoomIn,
        ]);
        list.extend((0..RAIL_TOOLS.len()).map(Target::Tool));
        list.push(Target::Divider);
        for i in 0..self.layer_count() {
            list.extend([Target::Layer(i), Target::Eye(i), Target::Lock(i)]);
        }
        list.extend(self.prop_order().into_iter().map(Target::Prop));
        list.extend([Target::Copy, Target::SaveAsNew, Target::Save]);
        // Shipping portals the open card to the end of `body`.
        if self.bg_open {
            list.extend(Self::bg_menu_props().map(Target::Prop));
        }
        list.retain(|&t| self.exists(t) && self.enabled(t));
        list
    }

    pub fn set_focus(&mut self, focus: Option<Target>, visible: bool) {
        if focus == self.focus && visible == self.focus_visible {
            return;
        }
        let changed = focus != self.focus;
        // Inputs and the select draw their focus as an accent border.
        for target in [self.focus, focus].into_iter().flatten() {
            if let Target::Prop(
                prop @ (Prop::Font | Prop::Aspect | Prop::Number(_) | Prop::Textarea),
            ) = target
            {
                self.prop_dirty(prop);
            }
        }
        self.focus = focus;
        self.focus_visible = visible;
        if self
            .menu
            .is_some_and(|m| focus != Some(Target::Prop(m.kind.trigger())))
        {
            self.close_menu();
        }
        self.field_focus(focus, visible);
        if let (Some(Target::Layer(i) | Target::Eye(i) | Target::Lock(i)), true) = (focus, visible)
        {
            self.reveal_row(i);
        }
        if let (Some(Target::Prop(prop)), true) = (focus, visible) {
            self.reveal_prop(prop);
        }
        if changed && let Some(target) = focus {
            self.events.push(Event::Focus(target));
        }
        self.refresh(true);
        self.need = true;
    }

    fn focus_step(&mut self, forward: bool) {
        let list = self.focusable();
        if list.is_empty() {
            return;
        }
        let next = match self.focus.and_then(|f| list.iter().position(|&t| t == f)) {
            Some(i) if forward => list[(i + 1) % list.len()],
            Some(i) => list[(i + list.len() - 1) % list.len()],
            None if forward => list[0],
            None => list[list.len() - 1],
        };
        self.set_focus(Some(next), true);
    }

    fn reveal_row(&mut self, i: usize) {
        let top = 2. + i as f32 * (ROW_H + ROW_GAP);
        let h = self.l.list.h;
        let scroll = if top < self.list_scroll {
            top - 2.
        } else if top + ROW_H > self.list_scroll + h {
            top + ROW_H + 2. - h
        } else {
            return;
        };
        self.scroll_list(scroll - self.list_scroll);
    }

    fn scroll_list(&mut self, delta: f32) {
        let content = self.layer_count() as f32 * (ROW_H + ROW_GAP);
        let max = (content - self.l.list.h).max(0.);
        let next = (self.list_scroll + delta).clamp(0., max);
        if next != self.list_scroll {
            self.list_scroll = next;
            self.place();
            self.need = true;
        }
    }

    /// Editor shortcuts and focused-control keys; true when consumed.
    fn key(&mut self, vk: VIRTUAL_KEY, ctrl: bool, shift: bool) -> bool {
        // `closeOnEscape` runs before anything else sees the key.
        if self.bg_open && vk == VK_ESCAPE {
            self.set_bg_menu(false);
            return true;
        }
        if let Some(Target::Prop(Prop::Range(range))) = self.focus
            && !ctrl
            && self.range_key(range, vk.0)
        {
            return true;
        }
        // The separator keeps every key but Tab (shipping `stopPropagation`).
        if self.focus == Some(Target::Divider) && vk != VK_TAB {
            let step = if shift { 40. } else { 10. };
            let (min, max) = self.sidebar_bounds();
            let width = self.sidebar_width();
            match vk {
                VK_LEFT => self.resize_sidebar(Some(width + step)),
                VK_RIGHT => self.resize_sidebar(Some(width - step)),
                VK_HOME => self.resize_sidebar(Some(min)),
                VK_END => self.resize_sidebar(Some(max)),
                VK_RETURN => self.resize_sidebar(None),
                _ => {}
            }
            return true;
        }
        if self.focus == Some(Target::Slider) && !ctrl {
            let position = editor_viewport::zoom_slider_position(self.slider_value()).unwrap_or(0.);
            let next = match vk {
                VK_LEFT | VK_DOWN => Some(position - 0.01),
                VK_RIGHT | VK_UP => Some(position + 0.01),
                VK_PRIOR => Some(position + 0.1),
                VK_NEXT => Some(position - 0.1),
                VK_HOME => Some(0.),
                VK_END => Some(1.),
                _ => None,
            };
            if let Some(next) = next {
                self.set_slider(next);
                return true;
            }
        }
        if self.flyout {
            let columns = editor_chrome::SHAPE_FLYOUT_COLUMNS as isize;
            let count = SHAPE_TOOLS.len() as isize;
            let current = match self.focus {
                Some(Target::Shape(i)) => i as isize,
                _ => self.last_shape as isize,
            };
            let next = match vk {
                VK_ESCAPE | VK_TAB => {
                    self.set_flyout(false);
                    return true;
                }
                VK_RIGHT => current + 1,
                VK_LEFT => current - 1,
                VK_DOWN => current + columns,
                VK_UP => current - columns,
                VK_HOME => 0,
                VK_END => count - 1,
                VK_RETURN | VK_SPACE => {
                    if let Some(target) = self.focus {
                        self.activate(target);
                    }
                    return true;
                }
                _ => return false,
            };
            let next = next.rem_euclid(count) as usize;
            self.set_focus(Some(Target::Shape(next)), true);
            return true;
        }
        // Arrows on the Shapes button open its flyout.
        if self.focus == Some(Target::Tool(shapes_index()))
            && matches!(vk, VK_DOWN | VK_UP | VK_LEFT | VK_RIGHT)
        {
            self.activate(Target::Tool(shapes_index()));
            return true;
        }
        let letter = |key: VIRTUAL_KEY| vk == key;
        match vk {
            VK_TAB => self.focus_step(!shift),
            VK_SPACE | VK_RETURN if self.focus.is_some() => {
                if let Some(focus) = self.focus {
                    self.set_focus(Some(focus), true);
                    self.activate(focus);
                }
            }
            _ if ctrl && letter(VK_Z) && shift => self.activate(Target::Redo),
            _ if ctrl && letter(VK_Z) => self.activate(Target::Undo),
            _ if ctrl && letter(VK_Y) => self.activate(Target::Redo),
            _ if ctrl && letter(VK_C) => self.activate(Target::Copy),
            _ if ctrl && letter(VK_S) => self.activate(Target::Save),
            _ if ctrl && (letter(VK_0) || letter(VK_NUMPAD0)) => self.activate(Target::Fit),
            _ if ctrl && matches!(vk, VK_OEM_PLUS | VK_ADD) => self.zoom_by(KEYBOARD_ZOOM),
            _ if ctrl && matches!(vk, VK_OEM_MINUS | VK_SUBTRACT) => {
                self.zoom_by(1. / KEYBOARD_ZOOM)
            }
            _ if ctrl => return false,
            VK_DELETE | VK_BACK => self.delete_selected(),
            VK_ESCAPE => {
                if !self.cancel_gesture() {
                    self.set_selected(None);
                }
                self.set_crop(None);
                self.overlay_changed();
            }
            _ => {
                let ch = char::from_u32(u32::from(vk.0)).map(|c| c.to_ascii_lowercase());
                let Some(ch) = ch.filter(char::is_ascii_alphabetic) else {
                    return false;
                };
                let key = ch.to_string();
                if let Some(i) = RAIL_TOOLS
                    .iter()
                    .position(|t| t.key == key && t.key != editor_chrome::SHAPES_KEY)
                {
                    self.activate(Target::Tool(i));
                } else if let Some(i) = SHAPE_TOOLS
                    .iter()
                    .position(|t| t.shortcut.is_some_and(|s| s.eq_ignore_ascii_case(&key)))
                {
                    self.set_flyout(false);
                    self.select_tool(SHAPE_TOOLS[i].key);
                } else {
                    return false;
                }
            }
        }
        true
    }

    // ---- drawing ----

    fn draw(&self, dc: &ID2D1DeviceContext, node: usize) -> Result<()> {
        let n = &self.nodes[node];
        let t = &self.t;
        let g = &self.g;
        let c = |name: &str| t.color(name);
        let (w, h) = (n.rect.w, n.rect.h);
        let all = rect(0., 0., w, h);
        let (s2, s3, s4, s5) = (t.n("s-2"), t.n("s-3"), t.n("s-4"), t.n("s-5"));
        match n.kind {
            Kind::Group | Kind::List | Kind::Image => {}
            Kind::Banner => {
                // `--caution-surface` is translucent over the header's
                // surface; the window itself has no redirection bitmap.
                gfx::fill(dc, all, c("surface-raised"))?;
                gfx::fill(dc, all, c("caution-surface"))?;
                gfx::fill(dc, rect(0., h - 1., w, h), c("border-subtle"))?;
                let text = g.text(t, Style::Button, header::DRAFT_RESTORED, 1e4)?;
                // Never under Dismiss (`gap: var(--s-5)`) in a narrow window.
                let dismiss = self.l.buttons[button_slot(Target::Dismiss)];
                g.ellipsis(&text, (dismiss.x - 2. * s5).max(1.))?;
                gfx::text(
                    dc,
                    &text,
                    s5,
                    ((h - 1. - text.height) / 2.).round(),
                    c("caution-text"),
                )?;
            }
            Kind::Header => self.draw_header(dc, (w, h))?,
            Kind::Rail => {
                gfx::fill(dc, all, c("surface-raised"))?;
                gfx::fill(dc, rect(w - 1., 0., w, h), c("border-subtle"))?;
            }
            Kind::Well => {
                let light = t.appearance == "light";
                gfx::fill(dc, all, c(if light { "n-5" } else { "surface-sunken" }))?;
            }
            Kind::Page => self.draw_page(dc, (w, h))?,
            Kind::Overlay => self.draw_overlay(dc)?,
            Kind::Caret => gfx::fill(dc, all, self.caret_fill())?,
            Kind::FieldCaret => gfx::fill(dc, all, c("text"))?,
            Kind::Menu => self.draw_menu(dc, (w, h))?,
            Kind::MenuLit => self.draw_menu_lit(dc, (w, h))?,
            Kind::Sidebar => {
                gfx::fill(dc, all, c("surface-raised"))?;
                gfx::fill(dc, rect(0., 0., 1., h), c("border-subtle"))?;
                let rule = self.l.properties.y - self.l.sidebar.y;
                gfx::fill(dc, rect(1., rule, w, rule + 1.), c("border-subtle"))?;
            }
            Kind::LayersHeading => {
                gfx::fill(dc, all, c("surface-raised"))?;
                gfx::fill(dc, rect(0., 0., 1., h), c("border-subtle"))?;
                gfx::fill(dc, rect(1., h - 1., w, h), c("border-subtle"))?;
                let title = g.text(t, Style::Brand, editor_chrome::layers::TITLE, 1e4)?;
                let x = 1. + s5;
                gfx::text(
                    dc,
                    &title,
                    x,
                    ((h - 1. - title.height) / 2.).round(),
                    c("text"),
                )?;
                let count = g.text(t, Style::Kbd, &self.layer_count().to_string(), 1e4)?;
                let pill_w = (count.width + 2. * s2).max(19.);
                let px = x + title.width + s3;
                let py = ((h - 1. - 19.) / 2.).round();
                let pill = rect(px, py, px + pill_w, py + 19.);
                gfx::fill_round(dc, pill, 9.5, c("surface-sunken"))?;
                gfx::text(
                    dc,
                    &count,
                    px + (pill_w - count.width) / 2.,
                    py + (19. - count.height) / 2.,
                    c("text-subtle"),
                )?;
                // Add image layer: not in this host yet.
                let add = self.l.add_layer;
                let ax = add.x - self.l.sidebar.x;
                let ay = add.y - self.l.sidebar.y;
                gfx::icon(
                    dc,
                    &g.factory,
                    "plus",
                    &[],
                    (ax + 7., ay + 7., 16.),
                    1.8,
                    t.color_alpha("text-muted", 0.45),
                )?;
            }
            Kind::Properties => {
                gfx::fill(dc, rect(0., 0., 1., h), c("border-subtle"))?;
                // Shipping shows the heading only for a selection or a tool
                // other than Select, which the rail already names.
                if self.selected.is_none() && self.tool == "v" {
                    return Ok(());
                }
                gfx::fill(dc, rect(1., 0., w, HEADING_H), c("surface-raised"))?;
                gfx::fill(
                    dc,
                    rect(1., HEADING_H - 1., w, HEADING_H),
                    c("border-subtle"),
                )?;
                let heading = g.text(t, Style::Brand, &self.properties_title(), 1e4)?;
                gfx::text(
                    dc,
                    &heading,
                    1. + s5,
                    ((HEADING_H - 1. - heading.height) / 2.).round(),
                    c("text"),
                )?;
            }
            Kind::Bar => self.draw_bar(dc, (w, h))?,
            Kind::PropsBody => self.draw_props_body(dc, w)?,
            Kind::PropsThumb => {
                // A 10 px gutter; the thumb's 3 px transparent border leaves
                // a 4 px pill.
                let pill = rect(3., 3., (w - 3.).max(3.), (h - 3.).max(3.));
                let color = if self.thumb_hover || self.thumb_drag.is_some() {
                    c("text-faint")
                } else {
                    c("border-strong")
                };
                gfx::fill_round(dc, pill, ((w - 6.) / 2.).max(0.), color)?;
            }
            Kind::Zoom => {
                let radius = t.n("r-lg");
                gfx::fill_round(dc, all, radius, c("surface-sunken"))?;
                gfx::border(dc, all, radius, 1., c("border-subtle"))?;
                // Separators after Fit, Zoom out, the slider and Zoom in.
                let b = &self.l.buttons;
                for target in [Target::Fit, Target::ZoomOut, Target::Slider, Target::ZoomIn] {
                    let r = b[button_slot(target)];
                    gfx::fill(
                        dc,
                        rect(r.x + r.w - 1., 1., r.x + r.w, h - 1.),
                        c("border-subtle"),
                    )?;
                }
                // The preset readout (`select`): not interactive yet.
                let preset =
                    Rect::new(self.l.preset.x - self.l.zoom.x, 1., self.l.preset.w, h - 2.);
                let text = g.text(t, Style::MonoSmall, &self.zoom_text(), 1e4)?;
                gfx::text(
                    dc,
                    &text,
                    preset.x + s4,
                    ((h - text.height) / 2.).round(),
                    c("text"),
                )?;
                gfx::icon(
                    dc,
                    &g.factory,
                    "chevron-down",
                    &[],
                    (preset.x + preset.w - 16., (h - 10.) / 2., 10.),
                    1.8,
                    t.color_alpha("text-subtle", 0.6),
                )?;
            }
            Kind::Thumb => {
                gfx::shadow(dc, all, w / 2., t.shadow("shadow-xs"))?;
                gfx::fill_round(dc, all, w / 2., c("surface-raised"))?;
                gfx::border(dc, all, w / 2., 1., c("border-strong"))?;
            }
            Kind::Flyout => {
                let radius = t.n("r-lg");
                gfx::shadow(dc, all, radius, t.shadow("shadow-lg"))?;
                gfx::fill_round(dc, all, radius, c("surface-overlay"))?;
                gfx::border(dc, all, radius, 1., c("border-subtle"))?;
            }
            Kind::Ctl(target, layer) => self.draw_control(dc, target, layer, (w, h))?,
            Kind::BgMenu => self.draw_bg_menu(dc, (w, h))?,
        }
        Ok(())
    }

    /// Shipping `propertiesTitle`: the selected layer only under a tool that
    /// shows transform chrome (`toolShowsTransformChrome`: Select, a grouped
    /// shape or Arrow); other tools, such as Eraser, show their own label.
    fn properties_title(&self) -> String {
        let transform =
            self.tool == "v" || closed_kind(self.tool).is_some() || open_kind(self.tool).is_some();
        let selection = self.selected.as_ref().filter(|_| transform).and_then(|id| {
            self.presented.as_ref().and_then(|p| {
                p.document
                    .elements
                    .iter()
                    .find(|element| element.base().id == *id)
                    .map(editor_chrome::element_label)
            })
        });
        selection.unwrap_or_else(|| editor_chrome::tool_label(self.tool).into())
    }

    fn draw_header(&self, dc: &ID2D1DeviceContext, (w, h): (f32, f32)) -> Result<()> {
        let t = &self.t;
        let g = &self.g;
        let c = |name: &str| t.color(name);
        let (s2, s3, s4) = (t.n("s-2"), t.n("s-3"), t.n("s-4"));
        gfx::fill(dc, rect(0., 0., w, h), c("surface-raised"))?;
        gfx::fill(dc, rect(0., h - 1., w, h), c("border-subtle"))?;
        let l = &self.l;
        let y0 = l.header.y;
        let local = |r: Rect| rect(r.x, r.y - y0, r.x + r.w, r.y - y0 + r.h);
        let group = local(l.canvas_group);
        let radius = t.n("r-lg");
        gfx::fill_round(dc, group, radius, c("surface-sunken"))?;
        gfx::border(dc, group, radius, 1., c("border-subtle"))?;
        let cy = |text: &Text| ((group.top + group.bottom - text.height) / 2.).round();
        let label = g.text(t, Style::Status, header::CANVAS, 1e4)?;
        gfx::text(
            dc,
            &label,
            group.left + 4. + s4,
            cy(&label),
            c("text-subtle"),
        )?;
        // The W and H captions; their fields are controls of their own.
        for (letter, field) in [("W", l.width_box), ("H", l.height_box)] {
            let dim = g.text(t, Style::Status, letter, 1e4)?;
            let field = local(field);
            gfx::text(
                dc,
                &dim,
                field.left - s2 - dim.width,
                cy(&dim),
                c("text-subtle"),
            )?;
        }
        let times = g.text(t, Style::Small, "×", 1e4)?;
        let tx = local(l.width_box).right + 2.;
        gfx::text(dc, &times, tx, cy(&times), c("text-subtle"))?;
        let split = local(l.trim).left - 2. - s2 - 1.;
        gfx::fill(
            dc,
            rect(split, group.top + 9., split + 1., group.top + 25.),
            c("border"),
        )?;
        let dim = |name: &str| t.color_alpha(name, 0.4);
        // Add images: not in this host yet (disabled `.screenshot-add-image`).
        let add = local(l.add_images);
        let radius = t.n("r-md");
        gfx::fill_round(dc, add, radius, t.color_alpha("control", 0.45))?;
        gfx::border(dc, add, radius, 1., dim("control-border"))?;
        let text = g.text(t, Style::Button, header::ADD_IMAGES, 1e4)?;
        let x = add.left + 1. + t.n("s-5");
        gfx::icon(
            dc,
            &g.factory,
            "image",
            &[],
            (x, (add.top + add.bottom - 16.) / 2., 16.),
            1.8,
            t.color_alpha("text", 0.45),
        )?;
        gfx::text(
            dc,
            &text,
            x + 16. + s3,
            cy(&text),
            t.color_alpha("text", 0.45),
        )?;
        Ok(())
    }

    /// `.screenshot-canvas-surface`: shadow, background or checkerboard,
    /// and the 1 px `--border` ring, for the visible part of the page.
    fn draw_page(&self, dc: &ID2D1DeviceContext, (w, h): (f32, f32)) -> Result<()> {
        let t = &self.t;
        let (ox, oy, _, _) = self.page_key;
        let Some(r) = self.image_rect() else {
            return Ok(());
        };
        // The image box in page coordinates (it may reach past the page).
        let image = rect(-ox, -oy, -ox + r.width as f32, -oy + r.height as f32);
        let visible = rect(
            image.left.max(-PAGE_BLEED),
            image.top.max(-PAGE_BLEED),
            image.right.min(w + PAGE_BLEED),
            image.bottom.min(h + PAGE_BLEED),
        );
        gfx::shadow(dc, visible, 0., t.shadow("shadow-lg"))?;
        let inner = rect(
            image.left.max(0.),
            image.top.max(0.),
            image.right.min(w),
            image.bottom.min(h),
        );
        match self
            .presented
            .as_ref()
            .map(|p| p.document.background.clone())
        {
            Some(Some(hex)) => gfx::fill(dc, inner, hex_color(&hex))?,
            Some(None) => {
                gfx::fill(dc, inner, t.color("canvas-checker-b"))?;
                let a = t.color("canvas-checker-a");
                let half = CHECKER / 2.;
                // Squares aligned to the image's top-left, as shipping's
                // `background-position: 0 0`.
                let first =
                    |edge: f32, origin: f32| origin + ((edge - origin) / half).floor() * half;
                let mut y = first(inner.top, image.top);
                while y < inner.bottom {
                    let row = ((y - image.top) / half).round() as i64;
                    let mut x = first(inner.left, image.left);
                    while x < inner.right {
                        let column = ((x - image.left) / half).round() as i64;
                        if (row + column).rem_euclid(2) == 0 {
                            let cell = rect(
                                x.max(inner.left),
                                y.max(inner.top),
                                (x + half).min(inner.right),
                                (y + half).min(inner.bottom),
                            );
                            gfx::fill(dc, cell, a)?;
                        }
                        x += half;
                    }
                    y += half;
                }
            }
            None => {}
        }
        // `box-shadow: 0 0 0 1px var(--border)` outside the page.
        gfx::ring(dc, image, 0., 0., 1., t.color("border"))?;
        Ok(())
    }

    /// Selection outline and the in-progress gesture, well-relative.
    /// The overlay's content box without the brush ring, and the ring's:
    /// a ring that moves alone redraws only where it was and is.
    fn overlay_parts(&self) -> (Option<Rect>, Option<Rect>) {
        let ring = self
            .brush_ring()
            .map(|(x, y, r)| Rect::new(x - r - 13., y - r - 13., 2. * r + 26., 2. * r + 26.));
        (self.overlay_rest(), ring)
    }

    fn overlay_rest(&self) -> Option<Rect> {
        let mut points: Vec<(f32, f32)> = Vec::new();
        let offset = self.move_offset();
        if !self.guides.is_empty()
            && let Some(r) = self.image_rect()
        {
            // Guides span the canvas, with room for their glow.
            points.push((r.x as f32 - 12., r.y as f32 - 12.));
            points.push(((r.x + r.width) as f32 + 12., (r.y + r.height) as f32 + 12.));
        }
        if let Some(outline) = self.selection_outline() {
            points.extend(outline.iter().filter_map(|p| {
                self.to_well(Point {
                    x: p.x + offset.0,
                    y: p.y + offset.1,
                })
            }));
        }
        match &self.gesture {
            Some(Gesture::Shape { start, end, .. }) => {
                points.extend([*start, *end].iter().filter_map(|p| self.to_well(*p)));
            }
            Some(Gesture::Freehand(stroke)) => {
                points.extend(stroke.iter().filter_map(|p| self.to_well(*p)));
            }
            _ => {}
        }
        points.extend(self.text_corners());
        if self.crop.is_some()
            && let Some(r) = self.image_rect()
        {
            // The dim covers the canvas.
            points.push((r.x as f32, r.y as f32));
            points.push(((r.x + r.width) as f32, (r.y + r.height) as f32));
        }
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for (x, y) in &points {
            x0 = x0.min(*x);
            y0 = y0.min(*y);
            x1 = x1.max(*x);
            y1 = y1.max(*y);
        }
        (!points.is_empty()).then(|| {
            // Room for the stroke and the selection grips.
            let pad = 10.;
            Rect::new(x0 - pad, y0 - pad, x1 - x0 + 2. * pad, y1 - y0 + 2. * pad)
        })
    }

    /// How far to shift the selection outline: a move not previewed yet
    /// follows the pointer; a previewed one has its snapped outline.
    fn move_offset(&self) -> (f64, f64) {
        match &self.gesture {
            Some(Gesture::Move { start, end, .. }) if self.preview_outline.is_none() => {
                (end.x - start.x, end.y - start.y)
            }
            _ => (0., 0.),
        }
    }

    fn selection_outline(&self) -> Option<[Point; 4]> {
        if matches!(
            self.gesture,
            Some(Gesture::Resize { .. } | Gesture::Move { .. })
        ) && let Some(outline) = self.preview_outline
        {
            return Some(outline);
        }
        let id = self.selected.as_ref()?;
        self.presented.as_ref()?.outlines.get(id).copied()
    }

    /// `.screenshot-align-snap-guide`: a 2 px accent line across the
    /// canvas, centred on the snapped edge, with a 10 px accent glow
    /// (`box-shadow: 0 0 10px rgba(accent, 0.75)`, approximated by wider,
    /// fainter strokes).
    fn draw_guides(&self, dc: &ID2D1DeviceContext) -> Result<()> {
        let (Some(r), Some((dw, dh))) = (self.image_rect(), self.document_size()) else {
            return Ok(());
        };
        let (x0, y0) = (r.x as f32, r.y as f32);
        let (x1, y1) = ((r.x + r.width) as f32, (r.y + r.height) as f32);
        for guide in &self.guides {
            let line = match guide.orientation {
                GuideOrientation::Vertical => {
                    let x = (r.x + guide.position / dw * r.width) as f32;
                    [(x, y0), (x, y1)]
                }
                GuideOrientation::Horizontal => {
                    let y = (r.y + guide.position / dh * r.height) as f32;
                    [(x0, y), (x1, y)]
                }
            };
            for (width, alpha) in [(14., 0.12), (8., 0.22), (4., 0.4)] {
                let glow = self.t.color_alpha("theme-accent", alpha);
                polyline(dc, &self.g, &line, false, glow, width)?;
            }
            polyline(dc, &self.g, &line, false, self.t.color("theme-accent"), 2.)?;
        }
        Ok(())
    }

    fn draw_overlay(&self, dc: &ID2D1DeviceContext) -> Result<()> {
        let t = &self.t;
        let accent = t.color("theme-accent");
        let offset = self.move_offset();
        self.draw_guides(dc)?;
        if let Some(outline) = self.selection_outline() {
            let screen: Vec<(f32, f32)> = outline
                .iter()
                .filter_map(|p| {
                    self.to_well(Point {
                        x: p.x + offset.0,
                        y: p.y + offset.1,
                    })
                })
                .collect();
            polyline(dc, &self.g, &screen, true, accent, 1.5)?;
        }
        match &self.gesture {
            Some(Gesture::Shape { tool, start, end }) => {
                let (Some(a), Some(b)) = (self.to_well(*start), self.to_well(*end)) else {
                    return Ok(());
                };
                let corners = [(a.0, a.1), (b.0, a.1), (b.0, b.1), (a.0, b.1)];
                if open_kind(tool).is_some() {
                    polyline(dc, &self.g, &[a, b], false, accent, 2.)?;
                } else {
                    polyline(dc, &self.g, &corners, true, accent, 1.5)?;
                }
            }
            Some(Gesture::Freehand(points)) => {
                let screen: Vec<(f32, f32)> =
                    points.iter().filter_map(|p| self.to_well(*p)).collect();
                polyline(dc, &self.g, &screen, false, accent, 2.)?;
            }
            _ => {}
        }
        self.draw_crop(dc)?;
        self.draw_grips(dc)?;
        self.draw_brush_ring(dc)?;
        self.draw_text(dc)
    }

    fn draw_bar(&self, dc: &ID2D1DeviceContext, (w, h): (f32, f32)) -> Result<()> {
        let t = &self.t;
        let g = &self.g;
        let c = |name: &str| t.color(name);
        let (s3, s4, s5) = (t.n("s-3"), t.n("s-4"), t.n("s-5"));
        gfx::fill(dc, rect(0., 0., w, h), c("surface-raised"))?;
        gfx::fill(dc, rect(0., 0., w, 1.), c("border-subtle"))?;
        let l = &self.l;
        let y0 = l.bar.y;
        let local = |r: Rect| rect(r.x, r.y - y0, r.x + r.w, r.y - y0 + r.h);
        let view = self.export_view();
        let radius = t.n("r-md");
        let dim = |name: &str| t.color_alpha(name, 0.45);

        // Export settings: not in this host yet (disabled).
        let es = local(l.export_settings);
        gfx::fill_round(dc, es, radius, dim("control"))?;
        gfx::border(dc, es, radius, 1., dim("control-border"))?;
        let title = g.text(t, Style::Button, "Export settings", 1e4)?;
        gfx::text(dc, &title, es.left + 13., es.top + 5., dim("text"))?;
        let summary = view
            .as_ref()
            .map_or_else(String::new, |v| v.summary.clone());
        let summary = g.text(t, Style::MonoTiny, &summary, 1e4)?;
        g.ellipsis(&summary, es.right - es.left - 45.)?;
        gfx::text(
            dc,
            &summary,
            es.left + 13.,
            es.top + 23.,
            dim("text-subtle"),
        )?;
        gfx::icon(
            dc,
            &g.factory,
            "chevron-down",
            &[],
            (es.right - 22., (es.top + es.bottom - 12.) / 2., 12.),
            1.8,
            dim("text-subtle"),
        )?;

        // Filename and its folder line.
        let folder = local(l.folder);
        let label = g.text(t, Style::Status, "Filename", 1e4)?;
        gfx::text(dc, &label, folder.left, folder.top + 2., c("text-muted"))?;
        let change = local(l.change);
        let change_text = g.text(t, Style::Tiny, "Change…", 1e4)?;
        gfx::text(
            dc,
            &change_text,
            change.left + s3,
            change.top + 3.,
            dim("text"),
        )?;
        let directory = self.save_location();
        let path = g.text(t, Style::MonoTiny, &directory, 1e4)?;
        let saving = g.text(t, Style::Tiny, "Saving to", 1e4)?;
        let room =
            (change.left - s3 - (folder.left + label.width + s5) - saving.width - 4.).max(20.);
        if path.width > room {
            g.ellipsis(&path, room)?;
        }
        let path_w = path.width.min(room);
        let px = change.left - s3 - path_w;
        gfx::text(dc, &path, px, folder.top + 3., c("text-subtle"))?;
        gfx::text(
            dc,
            &saving,
            px - 4. - saving.width,
            folder.top + 3.,
            c("text-subtle"),
        )?;

        // The filename field (read-only here) and its format suffix.
        let field = local(l.filename);
        gfx::fill_round(dc, field, radius, c("surface-field"))?;
        gfx::border(dc, field, radius, 1., c("control-border"))?;
        let suffix = local(l.suffix);
        gfx::fill(
            dc,
            rect(
                suffix.left - 1.,
                field.top + 1.,
                suffix.left,
                field.bottom - 1.,
            ),
            c("border-subtle"),
        )?;
        let stem = g.text(t, Style::Button, &self.filename(), 1e4)?;
        g.ellipsis(&stem, suffix.left - field.left - 2. * s4)?;
        gfx::text(
            dc,
            &stem,
            field.left + s4,
            ((field.top + field.bottom - stem.height) / 2.).round(),
            c("text"),
        )?;
        let ext = view
            .as_ref()
            .map_or_else(|| ".png".into(), |v| v.suffix.clone());
        let ext = g.text(t, Style::Option, &ext, 1e4)?;
        gfx::text(
            dc,
            &ext,
            suffix.left + s4,
            ((suffix.top + suffix.bottom - ext.height) / 2.).round(),
            dim("text-subtle"),
        )?;
        gfx::icon(
            dc,
            &g.factory,
            "chevron-down",
            &[],
            (
                suffix.right - 18.,
                (suffix.top + suffix.bottom - 11.) / 2.,
                11.,
            ),
            1.8,
            dim("text-subtle"),
        )?;

        // Notice over the hint, right-aligned before the switch.
        let hint = local(l.hint);
        let width = (hint.right - hint.left).max(1.);
        let mut lines = Vec::new();
        if let Some((text, error)) = self.notice_text() {
            let color = if error {
                "danger-text"
            } else {
                "positive-text"
            };
            lines.push((g.text(t, Style::Note, &text, width)?, c(color)));
        }
        let hint_text = self.hint_text();
        if !hint_text.is_empty() {
            let warning = view.as_ref().is_some_and(|v| v.hint_warning);
            lines.push((
                g.text(t, Style::Note, &hint_text, width)?,
                c(if warning {
                    "caution-text"
                } else {
                    "text-subtle"
                }),
            ));
        }
        let total: f32 = lines.iter().map(|(text, _)| text.height).sum();
        let mut y = (hint.top + hint.bottom - total) / 2. + s4 / 2.;
        for (text, color) in lines {
            unsafe {
                text.layout.SetMaxWidth(width)?;
                text.layout
                    .SetTextAlignment(DWRITE_TEXT_ALIGNMENT_TRAILING)?;
            }
            gfx::text(dc, &text, hint.left, y, color)?;
            y += text.height;
        }

        // Save as new file's label and the separator before Save.
        let switch = local(l.switch_label);
        let label = g.text(t, Style::Status, "Save as new file", 1e4)?;
        let enabled = self.enabled(Target::SaveAsNew);
        if self.exists(Target::SaveAsNew) {
            gfx::text(
                dc,
                &label,
                switch.left,
                ((switch.top + switch.bottom - label.height) / 2.).round(),
                if enabled {
                    c("text-muted")
                } else {
                    dim("text-muted")
                },
            )?;
            // `.screenshot-make-copy { border-right }`, gone with the toggle.
            let save = local(l.buttons[button_slot(Target::Save)]);
            let sep = (save.left - s5 - 1. + (s5 - 1.) / 2.).round();
            gfx::fill(
                dc,
                rect(sep, save.top, sep + 1., save.bottom),
                c("border-subtle"),
            )?;
        }
        Ok(())
    }

    fn draw_control(
        &self,
        dc: &ID2D1DeviceContext,
        target: Target,
        layer: Layer,
        (w, h): (f32, f32),
    ) -> Result<()> {
        if let Target::Prop(prop) = target {
            return self.draw_prop(dc, prop, layer, (w, h));
        }
        let t = &self.t;
        let g = &self.g;
        let c = |name: &str| t.color(name);
        let all = rect(0., 0., w, h);
        let (s3, s4, s5) = (t.n("s-3"), t.n("s-4"), t.n("s-5"));
        let r_md = t.n("r-md");
        let lit = layer == Layer::Lit;
        let radius = match target {
            Target::Tool(_) => t.n("r-lg"),
            Target::Fit | Target::ZoomOut | Target::ZoomIn | Target::Slider => 0.,
            Target::Layer(_) => t.n("r-lg"),
            Target::Eye(_) | Target::Lock(_) => t.n("r-sm"),
            _ => r_md,
        };
        if layer == Layer::Ring {
            // `outline: 2px solid var(--theme-accent); outline-offset: 2px`,
            // inset for the layer row's `box-shadow`.
            return match target {
                Target::Layer(_) => {
                    gfx::ring(dc, all, radius, -2., 0., t.color_alpha("theme-accent", 0.5))
                }
                Target::Fit | Target::ZoomOut | Target::ZoomIn | Target::Slider => {
                    gfx::ring(dc, all, 0., -2., 0., c("theme-accent"))
                }
                _ => gfx::ring(dc, all, radius, 2., 4., c("theme-accent")),
            };
        }
        let icon = |name: &str, side: f32, stroke: f32, color: D2D1_COLOR_F| {
            gfx::icon(
                dc,
                &g.factory,
                name,
                &[],
                ((w - side) / 2., (h - side) / 2., side),
                stroke,
                color,
            )
        };
        match target {
            Target::Dismiss => {
                if lit {
                    gfx::fill_round(dc, all, r_md, t.color_alpha("text", 0.08))?;
                }
                let text = g.text(t, Style::Button, header::DRAFT_DISMISS, 1e4)?;
                gfx::text(
                    dc,
                    &text,
                    (w - text.width) / 2.,
                    (h - text.height) / 2.,
                    c("caution-text"),
                )?;
            }
            Target::Trim => {
                if lit {
                    gfx::fill_round(dc, all, r_md, c("surface-hover"))?;
                }
                let ink = c(if lit { "text" } else { "text-muted" });
                gfx::icon(
                    dc,
                    &g.factory,
                    "trim",
                    &[],
                    (1. + s4, (h - 13.) / 2., 13.),
                    1.8,
                    ink,
                )?;
                let text = g.text(t, Style::Button, header::TRIM, 1e4)?;
                gfx::text(
                    dc,
                    &text,
                    1. + s4 + 13. + s3,
                    ((h - text.height) / 2.).round(),
                    ink,
                )?;
            }
            Target::Background => self.draw_bg_trigger(dc, lit, (w, h))?,
            Target::Undo | Target::Redo => {
                if lit {
                    gfx::fill_round(dc, all, r_md, c("surface-hover"))?;
                }
                let name = if target == Target::Undo {
                    "undo"
                } else {
                    "redo"
                };
                icon(name, 16., 1.8, c(if lit { "text" } else { "text-muted" }))?;
            }
            Target::Fit | Target::ZoomOut | Target::ZoomIn => {
                let active = target == Target::Fit && self.fit_mode();
                let (bg, ink) = match (lit, active) {
                    (true, _) => (Some("surface-hover"), "text"),
                    (false, true) => (
                        Some("surface-selected"),
                        if t.appearance == "dark" {
                            "theme-accent-text"
                        } else {
                            "theme-accent-readable"
                        },
                    ),
                    (false, false) => (None, "text-muted"),
                };
                if let Some(bg) = bg {
                    gfx::fill(dc, all, c(bg))?;
                }
                let name = match target {
                    Target::Fit => "fit",
                    Target::ZoomOut => "minus",
                    _ => "plus",
                };
                icon(name, 14., 1.8, c(ink))?;
            }
            Target::Slider => {
                if !lit {
                    let track = rect(s4, (h - 3.) / 2., w - s4, (h + 3.) / 2.);
                    gfx::fill_round(dc, track, 1.5, c("n-6"))?;
                }
            }
            Target::Tool(i) => {
                let active = self.tool_active(i);
                let ink = if active {
                    gfx::fill_round(dc, all, radius, c("theme-accent"))?;
                    c("theme-accent-ink")
                } else if lit {
                    gfx::fill_round(dc, all, radius, c("surface-hover"))?;
                    c("text")
                } else {
                    c("text-muted")
                };
                if lit && active {
                    return Ok(());
                }
                let name = if i == shapes_index() && SHAPE_TOOLS.iter().any(|s| s.key == self.tool)
                {
                    SHAPE_TOOLS[self.last_shape].icon
                } else {
                    RAIL_TOOLS[i].icon
                };
                icon(name, 18., 1.75, ink)?;
                if i == shapes_index() {
                    // The corner caret (`::after`).
                    let caret = D2D1_COLOR_F {
                        a: ink.a * 0.72,
                        ..ink
                    };
                    let (x, y) = (w - 5., h - 5.);
                    unsafe {
                        let path = g.factory.CreatePathGeometry()?;
                        let sink = path.Open()?;
                        sink.BeginFigure(Vector2 { X: x, Y: y - 7. }, D2D1_FIGURE_BEGIN_FILLED);
                        sink.AddLines(&[Vector2 { X: x, Y: y }, Vector2 { X: x - 7., Y: y }]);
                        sink.EndFigure(D2D1_FIGURE_END_CLOSED);
                        sink.Close()?;
                        dc.FillGeometry(&path, &gfx::brush(dc, caret)?, None);
                    }
                }
            }
            Target::Shape(i) => {
                let active = i == self.last_shape;
                let ink = if active {
                    gfx::fill_round(dc, all, r_md, c("theme-accent"))?;
                    c("theme-accent-ink")
                } else if lit {
                    gfx::fill_round(dc, all, r_md, c("surface-hover"))?;
                    c("text")
                } else {
                    c("text-muted")
                };
                if lit && active {
                    return Ok(());
                }
                icon(SHAPE_TOOLS[i].icon, 22., 1.75, ink)?;
            }
            Target::Divider => {
                if lit {
                    let x = (w - t.n("s-1")) / 2.;
                    gfx::fill(dc, rect(x, 0., x + t.n("s-1"), h), c("theme-accent"))?;
                }
            }
            Target::Layer(i) => {
                let Some(layer) = self.layer(i) else {
                    return Ok(());
                };
                let selected = self.selected.as_deref() == Some(layer.id.as_str());
                if lit {
                    if !selected {
                        gfx::fill_round(dc, all, radius, c("surface-hover"))?;
                    }
                    return Ok(());
                }
                if selected {
                    gfx::fill_round(dc, all, radius, c("surface-selected"))?;
                    gfx::border(dc, all, radius, 1., t.color_alpha("theme-accent", 0.4))?;
                }
                // Grip, preview, copy; then the dimmed layer menu (stub).
                let grip = if layer.locked { 0.35 } else { 0.5 };
                gfx::icon(
                    dc,
                    &g.factory,
                    "grip",
                    &[],
                    (1. + s3, (h - 18.) / 2., 18.),
                    1.6,
                    t.color_alpha("text-subtle", grip),
                )?;
                let px = 1. + s3 + 12. + s4;
                let preview = rect(px, (h - 32.) / 2., px + 44., (h + 32.) / 2.);
                let fade = if layer.visible { 1. } else { 0.42 };
                gfx::fill_round(
                    dc,
                    preview,
                    t.n("r-sm"),
                    t.color_alpha("canvas-checker-b", fade),
                )?;
                gfx::border(dc, preview, t.n("r-sm"), 1., t.color_alpha("border", fade))?;
                gfx::icon(
                    dc,
                    &g.factory,
                    layer.icon,
                    &[],
                    (px + 13., (h - 18.) / 2., 18.),
                    1.7,
                    t.color_alpha("text-muted", fade),
                )?;
                let cx = px + 44. + s4;
                let room = (w - cx - s3 - 3. * QUICK.0 - 4. - s3).max(1.);
                let name = g.text(t, Style::Button, &layer.name, 1e4)?;
                g.ellipsis(&name, room)?;
                let kind = g.text(t, Style::Small, layer.kind, 1e4)?;
                g.ellipsis(&kind, room)?;
                let block = name.height + 2. + kind.height;
                let y = ((h - block) / 2.).round();
                gfx::text(dc, &name, cx, y, t.color_alpha("text", fade))?;
                gfx::text(
                    dc,
                    &kind,
                    cx,
                    y + name.height + 2.,
                    t.color_alpha("text-subtle", fade),
                )?;
                let mx = w - s3 - QUICK.0;
                gfx::icon(
                    dc,
                    &g.factory,
                    "more",
                    &[],
                    (mx + (QUICK.0 - 15.) / 2., (h - 15.) / 2., 15.),
                    1.8,
                    t.color_alpha("text-subtle", 0.25),
                )?;
            }
            Target::Eye(i) | Target::Lock(i) => {
                let Some(layer) = self.layer(i) else {
                    return Ok(());
                };
                let on = match target {
                    Target::Eye(_) => !layer.visible,
                    _ => layer.locked,
                };
                if lit {
                    gfx::fill_round(dc, all, radius, c("surface-hover"))?;
                    return Ok(());
                }
                // `.screenshot-layer-quick-actions > button.active`: hidden
                // or locked.
                let ink = if on {
                    gfx::fill_round(dc, all, radius, c("surface-selected"))?;
                    c(if t.appearance == "dark" {
                        "theme-accent-text"
                    } else {
                        "theme-accent-readable"
                    })
                } else {
                    t.color_alpha("text-subtle", if on { 1. } else { 0.5 })
                };
                let name = match (target, layer.visible, layer.locked) {
                    (Target::Eye(_), true, _) => "eye",
                    (Target::Eye(_), false, _) => "eye-off",
                    (_, _, true) => "lock",
                    _ => "unlock",
                };
                icon(name, 15., 1.8, ink)?;
            }
            Target::Copy => {
                let (bg, edge) = if lit {
                    ("control-hover", "border-strong")
                } else {
                    gfx::shadow(dc, all, r_md, t.shadow("shadow-xs"))?;
                    ("control", "control-border")
                };
                gfx::fill_round(dc, all, r_md, c(bg))?;
                gfx::border(dc, all, r_md, 1., c(edge))?;
                let text = g.text(t, Style::Button, "Copy image", 1e4)?;
                let x = (w - 15. - s3 - text.width) / 2.;
                gfx::icon(
                    dc,
                    &g.factory,
                    "copy",
                    &[],
                    (x, (h - 15.) / 2., 15.),
                    1.7,
                    c("text"),
                )?;
                gfx::text(dc, &text, x + 15. + s3, (h - text.height) / 2., c("text"))?;
            }
            Target::SaveAsNew => {
                let on = self.pressed(target).unwrap_or(false);
                let x = t.n("s-2");
                let y = ((h - SWITCH.1) / 2.).round();
                let track = rect(x, y, x + SWITCH.0, y + SWITCH.1);
                let pill = SWITCH.1 / 2.;
                if lit {
                    gfx::border(dc, track, pill, 1., c("border-strong"))?;
                    return Ok(());
                }
                if on {
                    gfx::fill_round(dc, track, pill, c("theme-accent"))?;
                } else {
                    gfx::fill_round(dc, track, pill, c("n-4"))?;
                    gfx::border(dc, track, pill, 1., c("border"))?;
                }
                let knob = SWITCH.1 - 6.;
                let kx = if on { track.right - 3. - knob } else { x + 3. };
                let k = rect(kx, y + 3., kx + knob, y + 3. + knob);
                gfx::fill_round(
                    dc,
                    k,
                    knob / 2.,
                    if on {
                        c("theme-accent-ink")
                    } else {
                        c("text-subtle")
                    },
                )?;
            }
            Target::Save => {
                if !lit {
                    gfx::shadow(dc, all, r_md, t.shadow("shadow-sm"))?;
                }
                let bg = if lit {
                    "theme-accent-hover"
                } else {
                    "theme-accent"
                };
                gfx::fill_round(dc, all, r_md, c(bg))?;
                let text = g.text(t, Style::Strong, "Save", 1e4)?;
                let x = (w - 15. - s3 - text.width) / 2.;
                let ink = c("theme-accent-ink");
                gfx::icon(
                    dc,
                    &g.factory,
                    "save",
                    &[],
                    (x, (h - 15.) / 2., 15.),
                    1.7,
                    ink,
                )?;
                gfx::text(dc, &text, x + 15. + s3, (h - text.height) / 2., ink)?;
                let _ = s5;
            }
            Target::Prop(_) => {}
        }
        Ok(())
    }

    /// Paint every visible node into `dc` (the `--screenshot` renderer).
    pub fn paint(&self, dc: &ID2D1DeviceContext, scale: f32) -> Result<()> {
        self.paint_node(dc, scale, 0, (0., 0.))
    }

    fn paint_node(
        &self,
        dc: &ID2D1DeviceContext,
        scale: f32,
        node: usize,
        origin: (f32, f32),
    ) -> Result<()> {
        let n = &self.nodes[node];
        if !n.visible || n.opacity <= 0. {
            return Ok(());
        }
        let (x, y) = (origin.0 + n.rect.x, origin.1 + n.rect.y);
        let layer = n.opacity < 1.;
        unsafe {
            dc.SetTransform(&Matrix3x2::identity());
            if layer {
                dc.PushLayer(
                    &D2D1_LAYER_PARAMETERS1 {
                        contentBounds: rect(-1e6, -1e6, 1e6, 1e6),
                        opacity: n.opacity,
                        maskTransform: Matrix3x2::identity(),
                        ..Default::default()
                    },
                    None::<&ID2D1Layer>,
                );
            }
            if n.clip {
                dc.PushAxisAlignedClip(
                    &rect(
                        x * scale,
                        y * scale,
                        (x + n.rect.w) * scale,
                        (y + n.rect.h) * scale,
                    ),
                    D2D1_ANTIALIAS_MODE_ALIASED,
                );
            }
            if n.kind == Kind::Image {
                self.paint_image(dc, scale, (x, y))?;
            } else if n.kind != Kind::Group {
                dc.SetTransform(&Matrix3x2 {
                    M11: scale,
                    M12: 0.,
                    M21: 0.,
                    M22: scale,
                    M31: (x * scale).round(),
                    M32: (y * scale).round(),
                });
                self.draw(dc, node)?;
            }
            for &child in &self.children[node] {
                self.paint_node(dc, scale, child, (x, y))?;
            }
            dc.SetTransform(&Matrix3x2::identity());
            if n.clip {
                dc.PopAxisAlignedClip();
            }
            if layer {
                dc.PopLayer();
            }
        }
        Ok(())
    }

    /// The image scaled into its rect, as the compositor would show it.
    fn paint_image(&self, dc: &ID2D1DeviceContext, scale: f32, (x, y): (f32, f32)) -> Result<()> {
        let (Some(pixels), Some(r)) = (&self.image, self.image_rect()) else {
            return Ok(());
        };
        unsafe {
            let bitmap = bitmap(dc, pixels, (0, 0, pixels.width, pixels.height))?;
            let target: &ID2D1RenderTarget = dc;
            let dest = rect(
                (x * scale).round(),
                (y * scale).round(),
                (x * scale).round() + r.width as f32 * scale,
                (y * scale).round() + r.height as f32 * scale,
            );
            target.DrawBitmap(
                &bitmap,
                Some(&dest),
                1.,
                D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
                None,
            );
        }
        Ok(())
    }

    // ---- composition ----

    fn client_dips(&self) -> (f32, f32) {
        let mut rc = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut rc);
        }
        (rc.right as f32 / self.scale, rc.bottom as f32 / self.scale)
    }

    pub fn prepare(&mut self) -> Result<()> {
        if std::mem::take(&mut self.relayout) {
            let (w, h) = if self.hwnd.is_invalid() {
                (self.w, self.h)
            } else {
                self.client_dips()
            };
            self.layout(w, h)?;
            for node in &mut self.nodes {
                node.props = true;
            }
            self.refresh(false);
        }
        Ok(())
    }

    fn frame(&mut self) -> Result<()> {
        self.prepare()?;
        let Some(dcomp) = self.dcomp.clone() else {
            return Ok(());
        };
        let s = self.scale;
        let count = self.nodes.len();
        let mut abs = vec![(0f32, 0f32); count];
        let mut origin = vec![(0f32, 0f32); count];
        let mut snapped = vec![(0f32, 0f32); count];
        let mut shown = vec![true; count];
        for i in 0..count {
            let node = &self.nodes[i];
            let base = node.parent.map_or((0., 0.), |p| abs[p]);
            shown[i] = node.visible && node.parent.is_none_or(|p| shown[p]);
            abs[i] = (base.0 + node.rect.x, base.1 + node.rect.y);
            let bleed = (node.bleed * s).round();
            let (left, top) = ((abs[i].0 * s).round(), (abs[i].1 * s).round());
            snapped[i] = (
                ((abs[i].0 + node.rect.w) * s).round() - left,
                ((abs[i].1 + node.rect.h) * s).round() - top,
            );
            origin[i] = (left - bleed, top - bleed);
        }
        let mut draws = Vec::new();
        unsafe {
            for i in 0..count {
                if self.visuals[i].is_none() {
                    let visual: IDCompositionVisual3 = dcomp.CreateVisual()?.cast()?;
                    match self.nodes[i].parent {
                        // Nodes are created in paint order; with no reference,
                        // `insertAbove = false` puts each above its siblings.
                        Some(parent) => self.visuals[parent]
                            .as_ref()
                            .expect("parent visual")
                            .AddVisual(&visual, false, None::<&IDCompositionVisual>)?,
                        None => self._target.as_ref().expect("target").SetRoot(&visual)?,
                    }
                    if self.nodes[i].kind == Kind::Image {
                        visual.SetBitmapInterpolationMode(
                            DCOMPOSITION_BITMAP_INTERPOLATION_MODE_LINEAR,
                        )?;
                    }
                    self.visuals[i] = Some(visual);
                }
                let visual = self.visuals[i].as_ref().expect("visual").clone();
                let node = &mut self.nodes[i];
                if node.props {
                    node.props = false;
                    let parent = node.parent.map_or((0., 0.), |p| origin[p]);
                    if node.kind == Kind::Image {
                        // Zoom and pan: a transform on the document-sized surface.
                        if let Some(pixels) = &self.image {
                            let (sx, sy) = (
                                snapped[i].0 / pixels.width as f32,
                                snapped[i].1 / pixels.height as f32,
                            );
                            visual.SetOffsetX2(0.)?;
                            visual.SetOffsetY2(0.)?;
                            // IDCompositionVisual's 2D overload, not Visual3's 4×4 one.
                            let flat: &IDCompositionVisual = &visual;
                            flat.SetTransform2(&Matrix3x2 {
                                M11: sx,
                                M12: 0.,
                                M21: 0.,
                                M22: sy,
                                M31: origin[i].0 - parent.0,
                                M32: origin[i].1 - parent.1,
                            })?;
                        }
                    } else {
                        let offset = (origin[i].0 - parent.0, origin[i].1 - parent.1);
                        self.offsets[i] = offset;
                        visual.SetOffsetX2(offset.0)?;
                        visual.SetOffsetY2(offset.1)?;
                    }
                    // The caret's opacity belongs to its blink animation: a
                    // static value here would replace it after any relayout.
                    if !matches!(node.kind, Kind::Caret | Kind::FieldCaret) {
                        visual.SetOpacity2(node.opacity)?;
                    }
                    visual.SetVisible(node.visible)?;
                    if node.clip {
                        let bleed = (node.bleed * s).round();
                        visual.SetClip2(&rect(
                            bleed,
                            bleed,
                            bleed + snapped[i].0,
                            bleed + snapped[i].1,
                        ))?;
                    }
                }
                if matches!(node.kind, Kind::Group | Kind::List) || !shown[i] {
                    continue;
                }
                if node.kind == Kind::Image {
                    continue;
                }
                let bleed = 2. * (node.bleed * s).round();
                let size = (
                    (snapped[i].0 + bleed).max(1.) as u32,
                    (snapped[i].1 + bleed).max(1.) as u32,
                );
                if self.surfaces[i]
                    .as_ref()
                    .is_none_or(|(_, have)| *have != size)
                {
                    let surface = self.g.surface(size.0, size.1)?;
                    visual.SetContent(&surface)?;
                    self.surfaces[i] = Some((surface, size));
                    node.dirty = true;
                    if node.kind == Kind::Overlay {
                        self.overlay_drawn = (None, None);
                        self.overlay_fresh = true;
                    }
                }
                if node.dirty {
                    node.dirty = false;
                    draws.push(i);
                }
            }
        }
        self.upload_image()?;
        // A failed draw keeps its node dirty and does not skip the others or
        // the commit, so one bad surface cannot freeze the rest of the window.
        let mut failed = None;
        for i in draws {
            let (surface, size) = self.surfaces[i].as_ref().expect("surface").clone();
            let node = &self.nodes[i];
            let bleed = (node.bleed * s).round();
            let axis = |px: f32, dips: f32| if dips > 0. { px / dips } else { s };
            let scale = (
                axis(snapped[i].0, node.rect.w),
                axis(snapped[i].1, node.rect.h),
            );
            let drawn = if node.kind == Kind::Overlay {
                self.draw_overlay_region(&surface, size, scale)
            } else {
                gfx::draw_surface(&surface, size, scale, bleed, |dc| self.draw(dc, i))
            };
            if let Err(error) = drawn {
                self.nodes[i].dirty = true;
                if self.nodes[i].kind == Kind::Overlay {
                    self.overlay_fresh = true;
                }
                failed.get_or_insert(error);
            }
        }
        self.start_motions(&dcomp)?;
        if std::mem::take(&mut self.caret_blink) {
            self.blink_caret(&dcomp)?;
        }
        unsafe { dcomp.Commit()? };
        failed.map_or(Ok(()), Err)
    }

    /// Redraw only the union of the overlay's old and new content.
    fn draw_overlay_region(
        &mut self,
        surface: &IDCompositionSurface,
        size: (u32, u32),
        scale: (f32, f32),
    ) -> Result<()> {
        let now = self.overlay_parts();
        let fresh = std::mem::take(&mut self.overlay_fresh);
        let either = |a: Option<Rect>, b: Option<Rect>| match (a, b) {
            (Some(a), Some(b)) => Some(cover(a, b)),
            (a, b) => a.or(b),
        };
        let was = self.overlay_drawn;
        let region = if fresh {
            // A new surface is drawn whole once, transparent where empty.
            Some(Rect::new(
                0.,
                0.,
                size.0 as f32 / scale.0,
                size.1 as f32 / scale.1,
            ))
        } else if was.0 == now.0 {
            // Only the brush ring moved (or nothing did).
            either(was.1, now.1)
        } else {
            either(either(was.0, was.1), either(now.0, now.1))
        };
        self.overlay_drawn = now;
        let Some(region) = region else {
            return Ok(());
        };
        let px = RECT {
            left: ((region.x * scale.0).floor() as i32).clamp(0, size.0 as i32),
            top: ((region.y * scale.1).floor() as i32).clamp(0, size.1 as i32),
            right: (((region.x + region.w) * scale.0).ceil() as i32).clamp(0, size.0 as i32),
            bottom: (((region.y + region.h) * scale.1).ceil() as i32).clamp(0, size.1 as i32),
        };
        if px.right <= px.left || px.bottom <= px.top {
            return Ok(());
        }
        unsafe {
            let mut offset = POINT::default();
            let dc: ID2D1DeviceContext = surface.BeginDraw(Some(&px), &mut offset)?;
            dc.SetDpi(96., 96.);
            let (ox, oy) = ((offset.x - px.left) as f32, (offset.y - px.top) as f32);
            dc.SetTransform(&Matrix3x2::translation(ox, oy));
            dc.PushAxisAlignedClip(
                &rect(
                    px.left as f32,
                    px.top as f32,
                    px.right as f32,
                    px.bottom as f32,
                ),
                D2D1_ANTIALIAS_MODE_ALIASED,
            );
            dc.Clear(Some(&D2D1_COLOR_F::default()));
            dc.SetTransform(&Matrix3x2 {
                M11: scale.0,
                M12: 0.,
                M21: 0.,
                M22: scale.1,
                M31: ox,
                M32: oy,
            });
            let result = self.draw_overlay(&dc);
            dc.PopAxisAlignedClip();
            surface.EndDraw()?;
            result
        }
    }

    /// Upload new pixels: a new surface when the size changed, else only
    /// the changed rectangle.
    fn upload_image(&mut self) -> Result<()> {
        let (Some(pending), Some(pixels)) = (self.upload.take(), &self.image) else {
            return Ok(());
        };
        let i = self.image_node;
        let Some(visual) = self.visuals[i].clone() else {
            self.upload = Some(pending);
            return Ok(());
        };
        let size = (pixels.width, pixels.height);
        let fresh = self.surfaces[i]
            .as_ref()
            .is_none_or(|(_, have)| *have != size);
        if fresh {
            // A plain surface: partial updates to a tiled virtual surface left
            // a stale 1 px seam along old update edges on hardware (pass 14).
            let surface = self.g.surface(size.0, size.1)?;
            unsafe { visual.SetContent(&surface)? };
            self.surfaces[i] = Some((surface, size));
            self.nodes[i].props = true;
        }
        let (x, y, w, h) = if fresh {
            (0, 0, size.0, size.1)
        } else {
            pending
        };
        if w == 0 || h == 0 {
            return Ok(());
        }
        let surface = self.surfaces[i].as_ref().expect("surface").0.clone();
        unsafe {
            let update = RECT {
                left: x as i32,
                top: y as i32,
                right: (x + w) as i32,
                bottom: (y + h) as i32,
            };
            let mut offset = POINT::default();
            let dc: ID2D1DeviceContext = surface.BeginDraw(Some(&update), &mut offset)?;
            dc.SetDpi(96., 96.);
            dc.SetTransform(&Matrix3x2::identity());
            let (ox, oy) = (offset.x as f32, offset.y as f32);
            let area = rect(ox, oy, ox + w as f32, oy + h as f32);
            dc.PushAxisAlignedClip(&area, D2D1_ANTIALIAS_MODE_ALIASED);
            dc.Clear(Some(&D2D1_COLOR_F::default()));
            let result = bitmap(&dc, pixels, (x, y, w, h)).map(|bitmap| {
                let target: &ID2D1RenderTarget = &dc;
                target.DrawBitmap(
                    &bitmap,
                    Some(&area),
                    1.,
                    D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
                    None,
                );
            });
            dc.PopAxisAlignedClip();
            surface.EndDraw()?;
            result
        }
    }

    fn start_motions(&mut self, dcomp: &IDCompositionDesktopDevice) -> Result<()> {
        let mut any = false;
        for motion in self.motions.iter_mut().filter(|m| !m.applied) {
            motion.applied = true;
            any = true;
            if motion.fresh {
                motion.fresh = false;
                motion.start = Instant::now();
            }
            let elapsed = motion.start.elapsed().as_secs_f32().min(motion.seconds);
            let remaining = (motion.seconds - elapsed).max(0.001);
            let start = elapsed / motion.seconds;
            let Some(visual) = &self.visuals[motion.node] else {
                continue;
            };
            unsafe {
                let animation = dcomp.CreateAnimation()?;
                let step = remaining / SEGMENTS as f32;
                for k in 0..SEGMENTS {
                    let (p0, p1) = (
                        start + (1. - start) * k as f32 / SEGMENTS as f32,
                        start + (1. - start) * (k + 1) as f32 / SEGMENTS as f32,
                    );
                    let (v0, v1) = (motion.value_at(p0), motion.value_at(p1));
                    animation.AddCubic(f64::from(step * k as f32), v0, (v1 - v0) / step, 0., 0.)?;
                }
                animation.End(f64::from(remaining), motion.value_at(1.))?;
                visual.SetOpacity(&animation)?;
            }
        }
        if any && !self.timer {
            // Only to retire finished motions.
            unsafe { SetTimer(Some(self.hwnd), TIMER_MOTION, 16, None) };
            self.timer = true;
        }
        Ok(())
    }

    /// Restart the caret's blink, solid first, as a repeating compositor
    /// animation at the system rate (no timer, no redraw); the IME windows
    /// follow the caret.
    fn blink_caret(&mut self, dcomp: &IDCompositionDesktopDevice) -> Result<()> {
        // The label's caret, or a focused property input's.
        let node = if self.field.is_some() {
            self.field_caret_node()
        } else {
            self.caret
        };
        let Some(visual) = self.visuals[node].clone() else {
            return Ok(());
        };
        unsafe {
            let blink = GetCaretBlinkTime();
            if blink == 0 || blink == u32::MAX {
                visual.SetOpacity2(1.)?;
            } else {
                let on = f64::from(blink) / 1000.;
                let animation = dcomp.CreateAnimation()?;
                animation.AddCubic(0., 1., 0., 0., 0.)?;
                animation.AddCubic(on, 0., 0., 0., 0.)?;
                animation.AddRepeat(2. * on, 2. * on)?;
                visual.SetOpacity(&animation)?;
            }
        }
        if let Some(rect) = self.field_ime_rect().or_else(|| self.ime_rect()) {
            crate::textfield::ime_position(self.hwnd, rect);
        }
        Ok(())
    }

    /// Draw and commit now, waiting for the compositor (resize).
    fn present(&mut self) {
        self.need = false;
        if let Err(error) = self.frame() {
            eprintln!("editor frame failed: {error:?}");
        }
        if let Some(dcomp) = &self.dcomp {
            unsafe {
                let _ = dcomp.WaitForCommitCompletion();
            }
        }
    }

    /// One frame if due; the run loop calls this after clearing `woken`.
    pub fn service(&mut self) -> (bool, Vec<Event>) {
        // The IME is on only while a label is open or a property input has
        // focus, like a focused textarea.
        let text = self.inline.is_some() || self.field.is_some();
        if text != self.ime && !self.hwnd.is_invalid() {
            crate::textfield::ime_enable(self.hwnd, text);
            self.ime = text;
        }
        if self.need {
            self.need = false;
            if let Err(error) = self.frame() {
                eprintln!("editor frame failed: {error:?}");
            }
        }
        (self.need, std::mem::take(&mut self.events))
    }

    fn retheme(&mut self) {
        let settings = captures_settings::load(&self.settings_file).unwrap_or_default();
        let tokens = appearance::tokens(self.appearance.as_deref(), &settings);
        if tokens.appearance != self.t.appearance {
            appearance::title_bar(self.hwnd, &tokens);
            self.t = tokens;
            for node in &mut self.nodes {
                node.dirty = true;
            }
            self.page_key = (f32::NAN, 0., 0., 0.);
            self.relayout = true;
            self.need = true;
        }
    }

    fn handle(&mut self, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
        let s = self.scale;
        let x = (lp.0 & 0xffff) as i16 as f32 / s;
        let y = ((lp.0 >> 16) & 0xffff) as i16 as f32 / s;
        let key_down = |vk: VIRTUAL_KEY| unsafe { GetKeyState(vk.0 as i32) } < 0;
        match msg {
            WM_SIZE => {
                if lp.0 & 0xffff != 0 && (lp.0 >> 16) & 0xffff != 0 {
                    self.relayout = true;
                    self.present();
                }
            }
            // Only the run loop clears `woken`.
            WM_APP_WAKE => {}
            WM_APP_OUTCOME => {
                let outcomes: Vec<Outcome> = self.outcomes.try_iter().collect();
                for outcome in outcomes {
                    self.receive(outcome);
                }
            }
            WM_TIMER if wp.0 == TIMER_CLOSE => {
                unsafe {
                    let _ = KillTimer(Some(self.hwnd), TIMER_CLOSE);
                }
                self.close_ready = true;
            }
            WM_TIMER if wp.0 == TIMER_AUTOSAVE => {
                unsafe {
                    let _ = KillTimer(Some(self.hwnd), TIMER_AUTOSAVE);
                }
                // The session refuses drafts while a label is open; closing
                // it arms the save again (`DraftAutosave::take_due`).
                if self.inline.is_none() {
                    if self.autosave.take_due(Instant::now()) {
                        let _ = self.jobs.send(Job::Autosave);
                    } else {
                        self.arm_autosave();
                    }
                }
            }
            WM_TIMER if wp.0 == TIMER_PREVIEW => {
                unsafe {
                    let _ = KillTimer(Some(self.hwnd), TIMER_PREVIEW);
                }
                self.preview_timer = false;
                if self.preview_dirty {
                    self.preview_gesture();
                }
            }
            WM_TIMER if wp.0 == TIMER_MOTION => {
                let now = Instant::now();
                self.motions.retain(|m| m.progress(now) < 1.);
                if self.motions.is_empty() {
                    unsafe {
                        let _ = KillTimer(Some(self.hwnd), TIMER_MOTION);
                    }
                    self.timer = false;
                }
            }
            WM_SETTINGCHANGE => {
                self.reduced_motion = appearance::reduced_motion();
                self.retheme();
            }
            WM_KILLFOCUS => {
                self.pressed = None;
                if let Some((range, value)) = self.range_drag {
                    self.set_range(range, value, false);
                }
                self.divider_drag = None;
                self.slider_drag = false;
                self.text_drag = false;
                self.cancel_gesture();
                self.refresh(true);
                return None;
            }
            WM_MOUSEMOVE => {
                if !self.tracking {
                    let mut track = TRACKMOUSEEVENT {
                        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: self.hwnd,
                        dwHoverTime: 0,
                    };
                    unsafe {
                        let _ = TrackMouseEvent(&mut track);
                    }
                    self.tracking = true;
                }
                if let Some((y0, from)) = self.thumb_drag {
                    self.drag_props_thumb(y - y0, from);
                } else if let Some((x0, w0)) = self.divider_drag {
                    self.resize_sidebar(Some(w0 + x0 - x));
                } else if let Some((range, _)) = self.range_drag {
                    let value = self.range_at(range, x);
                    self.set_range(range, value, true);
                } else if self.slider_drag {
                    self.slide_to(x);
                } else if self.text_drag && self.field.is_some() {
                    self.field_point(x, y, false, true);
                } else if self.text_drag {
                    self.text_drag_to(x, y);
                } else if self.gesture.is_some() {
                    self.update(x, y);
                }
                if self.menu.is_some_and(|m| m.rect.contains(x, y)) {
                    let row = self.menu_item_at(x, y);
                    if row.is_some() {
                        self.set_menu_active(row);
                    }
                }
                self.set_pointer(Some((x, y)));
                self.hover_props_thumb(Some((x, y)));
                // The brush ring follows the pointer in the overlay.
                if self.brush_mode().is_some() {
                    self.overlay_changed();
                }
            }
            WM_MOUSELEAVE => {
                self.tracking = false;
                self.set_pointer(None);
                self.hover_props_thumb(None);
                if self.tool == "b" {
                    self.overlay_changed();
                }
            }
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK | WM_MBUTTONDOWN => {
                unsafe { SetCapture(self.hwnd) };
                let hit = self.hit(x, y);
                if self.flyout
                    && !self.l.flyout.contains(x, y)
                    && hit != Some(Target::Tool(shapes_index()))
                {
                    self.set_flyout(false);
                }
                // `closeOutside`: a press outside the card and its trigger.
                if self.bg_open
                    && !self.bg_menu_rect().contains(x, y)
                    && hit != Some(Target::Background)
                {
                    self.set_bg_menu(false);
                }
                // An open popup takes presses inside it; one outside closes
                // it (its trigger toggles it instead).
                if let Some(menu) = self.menu {
                    if menu.rect.contains(x, y) {
                        self.menu_press = self.menu_item_at(x, y);
                        return Some(LRESULT(0));
                    }
                    if hit != Some(Target::Prop(menu.kind.trigger())) {
                        self.close_menu();
                    }
                }
                if msg != WM_MBUTTONDOWN && self.press_props_scrollbar(x, y) {
                    return Some(LRESULT(0));
                }
                if msg == WM_MBUTTONDOWN {
                    if self.l.well.contains(x, y) {
                        self.gesture = Some(Gesture::Pan {
                            from: self.viewport,
                            x,
                            y,
                        });
                    }
                    return Some(LRESULT(0));
                }
                // A control takes focus from an open label: commit it.
                if self.inline.is_some() && hit.is_some() {
                    self.finish_text();
                }
                match hit {
                    Some(Target::Divider) => {
                        if msg == WM_LBUTTONDBLCLK {
                            self.resize_sidebar(None);
                        } else {
                            // Pointer resizing keeps focus where it is.
                            self.divider_drag = Some((x, self.sidebar_width()));
                            self.refresh(true);
                        }
                    }
                    Some(Target::Slider) => {
                        self.set_focus(Some(Target::Slider), false);
                        self.slider_drag = true;
                        self.slide_to(x);
                    }
                    Some(target @ Target::Prop(Prop::Range(range))) => {
                        self.set_focus(Some(target), false);
                        // The caption is part of the label; only the slider drags.
                        if y >= self.target_rect(target).y + props::SLIDER_TOP {
                            let value = self.range_at(range, x);
                            self.set_range(range, value, true);
                        }
                    }
                    Some(target @ Target::Prop(prop))
                        if Self::is_field(prop) && self.enabled(target) =>
                    {
                        self.set_focus(Some(target), false);
                        self.field_point(x, y, msg == WM_LBUTTONDBLCLK, key_down(VK_SHIFT));
                    }
                    // Steppers (`tabIndex=-1`) leave focus where it is.
                    Some(target @ Target::Prop(Prop::Step(..))) if self.enabled(target) => {
                        self.pressed = Some(target);
                        self.refresh(true);
                    }
                    Some(target) if self.enabled(target) => {
                        self.pressed = Some(target);
                        self.set_focus(Some(target), false);
                    }
                    Some(_) => {}
                    None if self.l.well.contains(x, y) && !self.flyout => {
                        self.set_focus(None, false);
                        self.begin(x, y, msg == WM_LBUTTONDBLCLK);
                    }
                    None => {}
                }
                self.need = true;
            }
            WM_LBUTTONUP | WM_MBUTTONUP => {
                self.text_drag = false;
                unsafe {
                    let _ = ReleaseCapture();
                }
                if let Some(row) = self.menu_press.take() {
                    if self.menu_item_at(x, y) == Some(row) {
                        self.choose_menu(row);
                    }
                    self.need = true;
                    return Some(LRESULT(0));
                }
                if self.divider_drag.take().is_some() {
                    self.refresh(true);
                }
                if self.thumb_drag.take().is_some() {
                    self.nodes[self.props_thumb].dirty = true;
                    self.hover_props_thumb(Some((x, y)));
                }
                self.slider_drag = false;
                if let Some((range, value)) = self.range_drag {
                    self.set_range(range, value, false);
                }
                if self.gesture.is_some() {
                    self.finish();
                }
                let hit = self.hit(x, y);
                if let Some(pressed) = self.pressed.take()
                    && hit == Some(pressed)
                {
                    self.activate(pressed);
                }
                self.need = true;
            }
            WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
                let delta = ((wp.0 >> 16) & 0xffff) as i16 as f32 / 120.;
                let mut point = POINT {
                    x: (lp.0 & 0xffff) as i16 as i32,
                    y: ((lp.0 >> 16) & 0xffff) as i16 as i32,
                };
                unsafe {
                    let _ = windows::Win32::Graphics::Gdi::ScreenToClient(self.hwnd, &mut point);
                }
                let (px, py) = (point.x as f32 / s, point.y as f32 / s);
                let pixels = delta * appearance::wheel_lines() * 100. / 3.;
                if self.menu.is_some_and(|m| m.rect.contains(px, py)) {
                    return Some(LRESULT(0));
                }
                if self.l.list.contains(px, py) && msg == WM_MOUSEWHEEL {
                    self.scroll_list(-pixels);
                } else if self.props_view().contains(px, py) && msg == WM_MOUSEWHEEL {
                    self.scroll_props(-pixels);
                } else if self.l.well.contains(px, py) {
                    if msg == WM_MOUSEWHEEL && key_down(VK_CONTROL) {
                        if let (Some(factor), Some(percent)) = (
                            editor_viewport::wheel_zoom_factor(f64::from(-pixels)),
                            self.zoom_percent(),
                        ) {
                            self.zoom_to(percent * factor, Some((px, py)));
                        }
                    } else {
                        let mut viewport = self.viewport;
                        let horizontal = msg == WM_MOUSEHWHEEL || key_down(VK_SHIFT);
                        if horizontal {
                            // WM_MOUSEHWHEEL is positive to the right.
                            let sign = if msg == WM_MOUSEHWHEEL { -1. } else { 1. };
                            viewport.pan_x += f64::from(sign * pixels);
                        } else {
                            viewport.pan_y += f64::from(pixels);
                        }
                        self.set_viewport(viewport);
                    }
                }
                self.set_pointer(Some((px, py)));
            }
            WM_SETCURSOR => {
                if (lp.0 & 0xffff) as u32 != HTCLIENT {
                    return None;
                }
                if let Some((x, y)) = self.pointer
                    && self.tool == "b"
                    && self.divider_drag.is_none()
                    && self.l.well.contains(x, y)
                    && self.hit(x, y).is_none()
                {
                    // `None` hides the system cursor under the brush ring.
                    let shape = self.eraser_cursor(x, y);
                    unsafe {
                        SetCursor(shape.and_then(|c| LoadCursorW(None, c).ok()));
                    }
                    return Some(LRESULT(1));
                }
                let cursor = match self.pointer {
                    _ if self.divider_drag.is_some() => IDC_SIZEWE,
                    Some((x, y)) if self.menu.is_some_and(|m| m.rect.contains(x, y)) => IDC_HAND,
                    Some((x, y)) => match self.hit(x, y) {
                        Some(Target::Divider) => IDC_SIZEWE,
                        Some(Target::Prop(prop)) if Self::is_field(prop) => IDC_IBEAM,
                        Some(target) if self.enabled(target) => IDC_HAND,
                        Some(_) => IDC_ARROW,
                        None if self.in_text(x, y) => IDC_IBEAM,
                        None if self.l.well.contains(x, y) => {
                            match self.to_document(x, y).and_then(|p| self.handle_at(p)) {
                                Some(ResizeHandle::N | ResizeHandle::S) => IDC_SIZENS,
                                Some(ResizeHandle::E | ResizeHandle::W) => IDC_SIZEWE,
                                Some(ResizeHandle::Nw | ResizeHandle::Se) => IDC_SIZENWSE,
                                Some(_) => IDC_SIZENESW,
                                None if self.tool == "t" => IDC_IBEAM,
                                None if self.tool != "v" => IDC_CROSS,
                                None => IDC_ARROW,
                            }
                        }
                        None => IDC_ARROW,
                    },
                    None => IDC_ARROW,
                };
                unsafe {
                    SetCursor(LoadCursorW(None, cursor).ok());
                }
                return Some(LRESULT(1));
            }
            WM_KEYDOWN => {
                let vk = VIRTUAL_KEY(wp.0 as u16);
                let (ctrl, shift) = (key_down(VK_CONTROL), key_down(VK_SHIFT));
                if self.menu_key(vk) {
                    self.need = true;
                    return Some(LRESULT(0));
                }
                // Shipping skips its editor shortcuts while an input,
                // textarea or select has focus: only Tab and the Ctrl zoom
                // keys pass.
                if self.field.is_some()
                    || matches!(self.focus, Some(Target::Prop(Prop::Font | Prop::Aspect)))
                {
                    let zoom = ctrl
                        && matches!(
                            vk,
                            VK_OEM_PLUS | VK_ADD | VK_OEM_MINUS | VK_SUBTRACT | VK_0 | VK_NUMPAD0
                        );
                    let consumed = self.field.is_some() && self.field_key(vk, ctrl, shift);
                    if !consumed && !((vk == VK_TAB || zoom) && self.key(vk, ctrl, shift)) {
                        return None;
                    }
                    self.need = true;
                    return Some(LRESULT(0));
                }
                let consumed = self.inline.is_some() && self.text_key(vk, ctrl, shift);
                if !consumed && !self.key(vk, ctrl, shift) {
                    return None;
                }
                self.need = true;
            }
            WM_CHAR if self.field.is_some() => self.field_char(wp.0 as u16),
            WM_IME_STARTCOMPOSITION if self.field.is_some() => {
                if let Some(rect) = self.field_ime_rect() {
                    crate::textfield::ime_position(self.hwnd, rect);
                }
            }
            WM_IME_COMPOSITION if self.field.is_some() => self.field_ime(lp.0 as u32),
            WM_IME_ENDCOMPOSITION if self.field.is_some() => self.field_ime_end(),
            WM_CHAR if self.inline.is_some() => self.text_char(wp.0 as u16),
            WM_IME_STARTCOMPOSITION if self.inline.is_some() => {
                // The composition draws inline; no system composition window.
                if let Some(rect) = self.ime_rect() {
                    crate::textfield::ime_position(self.hwnd, rect);
                }
            }
            WM_IME_COMPOSITION if self.inline.is_some() => {
                self.ime_composition(self.hwnd, lp.0 as u32);
            }
            WM_IME_ENDCOMPOSITION if self.inline.is_some() => self.ime_end(),
            WM_GETMINMAXINFO => {
                let size = crate::host::window_size(self.hwnd, MIN_SIZE.0, MIN_SIZE.1);
                let info = unsafe { &mut *(lp.0 as *mut MINMAXINFO) };
                info.ptMinTrackSize = POINT {
                    x: size.0,
                    y: size.1,
                };
                return Some(LRESULT(0));
            }
            _ => return None,
        }
        Some(LRESULT(0))
    }

    /// Move the zoom thumb to a client x.
    fn slide_to(&mut self, x: f32) {
        let slider = self.target_rect(Target::Slider);
        let s4 = self.t.n("s-4");
        let track = (slider.w - 2. * s4 - 12.).max(1.);
        let position = (x - slider.x - s4 - 6.) / track;
        self.set_slider(f64::from(position.clamp(0., 1.)));
    }
}

/// `Layout::buttons`: twelve fixed controls, the rail, and a spare.
const SLOTS: usize = 13 + RAIL_TOOLS.len() + 1;

/// Where each button's rect lives in `Layout::buttons`.
fn button_slot(target: Target) -> usize {
    match target {
        Target::Dismiss => 0,
        Target::Undo => 1,
        Target::Redo => 2,
        Target::Fit => 3,
        Target::ZoomOut => 4,
        Target::Slider => 5,
        Target::ZoomIn => 6,
        Target::Divider => 7,
        Target::Copy => 8,
        Target::SaveAsNew => 9,
        Target::Save => 10,
        Target::Trim => 11,
        // One slot per rail tool.
        Target::Tool(i) => 12 + i,
        Target::Background => 12 + RAIL_TOOLS.len(),
        // Laid out from their own rects, never through `Layout::buttons`.
        Target::Shape(_)
        | Target::Layer(_)
        | Target::Eye(_)
        | Target::Lock(_)
        | Target::Prop(_) => SLOTS - 1,
    }
}

/// A D2D bitmap of `pixels` inside `(x, y, w, h)`.
/// Patch published pixels into the UI's copy and return the rectangle that
/// changed (all of it when the image is new or resized).
fn patch(image: &mut Option<Image>, pixels: Pixels) -> PixelRect {
    if let (Some(image), Some(damage)) = (image.as_mut(), pixels.damage)
        && (image.width, image.height) == (pixels.width, pixels.height)
    {
        let (x, y, w, _) = damage;
        let row = w as usize * 4;
        for (r, from) in pixels.bgra.chunks_exact(row).enumerate() {
            let to = ((y as usize + r) * image.width as usize + x as usize) * 4;
            image.bgra[to..to + row].copy_from_slice(from);
        }
        return damage;
    }
    let full = (0, 0, pixels.width, pixels.height);
    *image = Some(Image {
        width: pixels.width,
        height: pixels.height,
        bgra: pixels.bgra,
    });
    full
}

/// The rectangle covering both.
fn cover(a: Rect, b: Rect) -> Rect {
    let (x0, y0) = (a.x.min(b.x), a.y.min(b.y));
    let (x1, y1) = ((a.x + a.w).max(b.x + b.w), (a.y + a.h).max(b.y + b.h));
    Rect::new(x0, y0, x1 - x0, y1 - y0)
}

fn bitmap(
    dc: &ID2D1DeviceContext,
    pixels: &Image,
    (x, y, w, h): (u32, u32, u32, u32),
) -> Result<ID2D1Bitmap1> {
    let pitch = pixels.width * 4;
    let start = (y * pitch + x * 4) as usize;
    unsafe {
        dc.CreateBitmap(
            D2D_SIZE_U {
                width: w,
                height: h,
            },
            Some(pixels.bgra[start..].as_ptr() as *const _),
            pitch,
            &D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 96.,
                dpiY: 96.,
                bitmapOptions: D2D1_BITMAP_OPTIONS_NONE,
                colorContext: std::mem::ManuallyDrop::new(None),
            },
        )
    }
}

/// A round-capped, round-joined polyline.
fn polyline(
    dc: &ID2D1DeviceContext,
    g: &Gfx,
    points: &[(f32, f32)],
    close: bool,
    color: D2D1_COLOR_F,
    width: f32,
) -> Result<()> {
    if points.len() < 2 {
        return Ok(());
    }
    unsafe {
        let geometry = g.factory.CreatePathGeometry()?;
        let sink = geometry.Open()?;
        let v = |(x, y): (f32, f32)| Vector2 { X: x, Y: y };
        sink.BeginFigure(v(points[0]), D2D1_FIGURE_BEGIN_HOLLOW);
        let rest: Vec<Vector2> = points[1..].iter().map(|p| v(*p)).collect();
        sink.AddLines(&rest);
        sink.EndFigure(if close {
            D2D1_FIGURE_END_CLOSED
        } else {
            D2D1_FIGURE_END_OPEN
        });
        sink.Close()?;
        let style = g.factory.CreateStrokeStyle(
            &D2D1_STROKE_STYLE_PROPERTIES1 {
                startCap: D2D1_CAP_STYLE_ROUND,
                endCap: D2D1_CAP_STYLE_ROUND,
                dashCap: D2D1_CAP_STYLE_ROUND,
                lineJoin: D2D1_LINE_JOIN_ROUND,
                miterLimit: 10.,
                ..Default::default()
            },
            None,
        )?;
        dc.DrawGeometry(&geometry, &gfx::brush(dc, color)?, width, &style);
    }
    Ok(())
}

/// A `#rrggbb` document color (shipping's `#f7f7f5` when unreadable).
fn hex_color(value: &str) -> D2D1_COLOR_F {
    let hex = value.trim_start_matches('#');
    let channel = |i: usize| {
        hex.get(i..i + 2)
            .and_then(|v| u8::from_str_radix(v, 16).ok())
            .map_or(0.97, |v| f32::from(v) / 255.)
    };
    if hex.len() < 6 {
        return D2D1_COLOR_F {
            r: 0.969,
            g: 0.969,
            b: 0.961,
            a: 1.,
        };
    }
    let alpha = if hex.len() >= 8 { channel(6) } else { 1. };
    D2D1_COLOR_F {
        r: channel(0),
        g: channel(2),
        b: channel(4),
        a: alpha,
    }
}

/// Put the image on the clipboard as `PNG` and `CF_DIB`.
fn set_clipboard(hwnd: HWND, png: &[u8], dib: &[u8]) -> Result<()> {
    use windows::Win32::System::DataExchange::*;
    use windows::Win32::System::Memory::*;
    const CF_DIB: u32 = 8;
    unsafe {
        OpenClipboard(Some(hwnd))?;
        let result = (|| {
            EmptyClipboard()?;
            let png_format = RegisterClipboardFormatW(w!("PNG"));
            for (format, bytes) in [(png_format, png), (CF_DIB, dib)] {
                let memory = GlobalAlloc(GMEM_MOVEABLE, bytes.len())?;
                let target = GlobalLock(memory) as *mut u8;
                if target.is_null() {
                    let _ = GlobalFree(Some(memory));
                    return Err(Error::from_thread());
                }
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), target, bytes.len());
                let _ = GlobalUnlock(memory);
                if let Err(error) = SetClipboardData(format, Some(HANDLE(memory.0))) {
                    let _ = GlobalFree(Some(memory));
                    return Err(error);
                }
            }
            Ok(())
        })();
        let _ = CloseClipboard();
        result
    }
}

fn find_entry(
    root: &std::path::Path,
    id: &str,
) -> std::result::Result<captures_history::HistoryEntry, String> {
    captures_history::load(root, chrono::Utc::now())
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|entry| entry.id == id && !entry.kind.is_recording())
        .ok_or_else(|| format!("No screenshot {id} in Capture History"))
}

fn root_of(options: &Options) -> PathBuf {
    options
        .history_root
        .clone()
        .unwrap_or_else(captures_app::default_history_root)
}

/// Open the editor for a History screenshot, or bring its window forward.
pub fn open(options: &Options, artifact_id: &str) -> Result<()> {
    let existing = EDITORS.with(|e| {
        e.try_borrow().ok().and_then(|e| {
            e.iter()
                .find(|x| x.artifact_id == artifact_id && !x.closing)
                .map(|x| x.hwnd)
        })
    });
    if let Some(hwnd) = existing {
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
            let _ = SetForegroundWindow(hwnd);
        }
        return Ok(());
    }
    let root = root_of(options);
    let entry = find_entry(&root, artifact_id).map_err(|e| Error::new(E_FAIL, e))?;
    let settings = captures_settings::load(&options.settings_file).unwrap_or_default();
    let tokens = appearance::tokens(options.appearance.as_deref(), &settings);
    let g = match crate::host::with_host(|h| h.g.clone())
        .or_else(|| crate::history::with_history(|h| h.g.clone()))
    {
        Some(g) => g,
        None => Gfx::new(&tokens, true)?,
    };
    unsafe {
        let instance = windows::Win32::System::LibraryLoader::GetModuleHandleW(None)?;
        let hwnd = CreateWindowExW(
            WS_EX_NOREDIRECTIONBITMAP,
            w!("CapturesEditor"),
            &HSTRING::from(captures_app::app_windows::SCREENSHOT_EDITOR_TITLE),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            SIZE.0 as i32,
            SIZE.1 as i32,
            None,
            None,
            Some(instance.into()),
            None,
        )?;
        let (w, h) = options
            .size
            .filter(|_| options.scene == crate::Scene::Editor)
            .map_or(SIZE, |(w, h)| (w as f32, h as f32));
        let (ww, wh) = crate::host::window_size(hwnd, w, h);
        let _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            ww,
            wh,
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
        let (jobs, inbox) = channel();
        let (results, outcomes) = channel();
        let previous = WORKERS.with(|w| {
            let mut w = w.borrow_mut();
            w.retain(|(_, worker)| !worker.is_finished());
            let (same, others): (Vec<_>, Vec<_>) = std::mem::take(&mut *w)
                .into_iter()
                .partition(|(id, _)| id == artifact_id);
            *w = others;
            same.into_iter().map(|(_, worker)| worker).collect()
        });
        let worker = spawn_worker(
            root,
            artifact_id.to_owned(),
            inbox,
            results,
            hwnd.0 as isize,
            previous,
        );
        WORKERS.with(|w| w.borrow_mut().push((artifact_id.to_owned(), worker)));
        let mode = entry.mode.unwrap_or(CaptureMode::Region);
        let mut editor = Editor::new(
            hwnd,
            g,
            tokens,
            options,
            artifact_id.to_owned(),
            &entry.created_at,
            mode,
            Some((jobs, outcomes)),
        );
        editor.scale = GetDpiForWindow(hwnd) as f32 / 96.;
        editor.reduced_motion = appearance::reduced_motion();
        let dcomp = editor.g.dcomp.clone().ok_or_else(Error::empty)?;
        editor._target = Some(dcomp.CreateTargetForHwnd(hwnd, true)?);
        editor.dcomp = Some(dcomp);
        // No IME until a label is open, like Chromium.
        crate::textfield::ime_enable(hwnd, false);
        editor.ime = false;
        appearance::title_bar(hwnd, &editor.t);
        if options.scene == crate::Scene::Editor {
            editor.start_tool(options);
        }
        EDITORS.with(|e| e.borrow_mut().push(editor));
        let _ = ShowWindow(hwnd, SW_SHOW);
    }
    Ok(())
}

pub fn register(instance: HINSTANCE) -> Result<()> {
    unsafe {
        let class = WNDCLASSW {
            style: CS_DBLCLKS,
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: w!("CapturesEditor"),
            ..Default::default()
        };
        RegisterClassW(&class);
    }
    Ok(())
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    use windows::Win32::UI::Accessibility::{UiaReturnRawElementProvider, UiaRootObjectId};
    unsafe {
        match msg {
            WM_GETOBJECT => {
                if lp.0 as i32 == UiaRootObjectId {
                    return UiaReturnRawElementProvider(
                        hwnd,
                        wp,
                        lp,
                        &crate::editor_uia::node(hwnd, crate::editor_uia::Id::Root),
                    );
                }
                return DefWindowProcW(hwnd, msg, wp, lp);
            }
            WM_DESTROY => {
                let _ = UiaReturnRawElementProvider(hwnd, WPARAM(0), LPARAM(0), None);
                let closed = EDITORS.with(|e| {
                    let mut e = e.try_borrow_mut().ok()?;
                    let index = e.iter().position(|editor| editor.hwnd == hwnd)?;
                    Some(e.remove(index))
                });
                // Any other teardown still commits the label and writes the
                // draft; the worker finishes its queue after this window.
                if let Some(mut editor) = closed {
                    editor.queue_close();
                }
                if !crate::host::exists() && !crate::history::exists() && !exists() {
                    PostQuitMessage(0);
                }
                return LRESULT(0);
            }
            WM_CLOSE => {
                // Commit an open composition with the editor released, so
                // its result string reaches the label before it is finished.
                if with_editor(hwnd, |e| e.composing()) == Some(true) {
                    crate::textfield::ime_complete(hwnd);
                }
                with_editor(hwnd, |e| e.request_close());
                return LRESULT(0);
            }
            WM_ENDSESSION if wp.0 != 0 => {
                if with_editor(hwnd, |e| e.composing()) == Some(true) {
                    crate::textfield::ime_complete(hwnd);
                }
                with_editor(hwnd, |e| e.flush_now());
                return LRESULT(0);
            }
            WM_ERASEBKGND => return LRESULT(1),
            WM_IME_SETCONTEXT => {
                // Labels draw the composition inline.
                let lp = LPARAM(
                    lp.0 & !(windows::Win32::UI::Input::Ime::ISC_SHOWUICOMPOSITIONWINDOW as isize),
                );
                return DefWindowProcW(hwnd, msg, wp, lp);
            }
            WM_PAINT => {
                let _ = windows::Win32::Graphics::Gdi::ValidateRect(Some(hwnd), None);
                return LRESULT(0);
            }
            WM_DPICHANGED => {
                let dpi = (wp.0 & 0xffff) as f32;
                with_editor(hwnd, |e| {
                    e.scale = dpi / 96.;
                    e.relayout = true;
                    for node in &mut e.nodes {
                        node.dirty = true;
                    }
                    e.need = true;
                });
                let r = &*(lp.0 as *const RECT);
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                return LRESULT(0);
            }
            _ => {}
        }
        let handled = with_editor(hwnd, |e| e.handle(msg, wp, lp));
        // The custom color dialog is modal: ask with the window released.
        if let Some(Some((field, seed))) =
            with_editor(hwnd, |e| e.pick_color.take().map(|f| (f, e.color_seed(f))))
        {
            match crate::dialog::pick_color(hwnd, &seed) {
                Ok(Some(hex)) => {
                    with_editor(hwnd, |e| e.set_color(field, hex));
                }
                Ok(None) => {}
                Err(error) => eprintln!("color picker failed: {error}"),
            }
        }
        // Destroyed outside `with_editor`, so `WM_DESTROY` can remove it.
        // Taken, not read: DestroyWindow sends messages back into this
        // wndproc (WM_UAHDESTROYWINDOW), which must not destroy again or the
        // editor's WM_DESTROY never runs and the process never quits. A late
        // flush outcome must not re-arm it either, so `closing` ends too.
        let ready = with_editor(hwnd, |e| {
            let ready = std::mem::take(&mut e.close_ready);
            if ready {
                e.closing = false;
            }
            ready
        });
        if ready == Some(true) {
            let _ = DestroyWindow(hwnd);
            return LRESULT(0);
        }
        if let Some(Some(result)) = handled {
            return result;
        }
        DefWindowProcW(hwnd, msg, wp, lp)
    }
}

/// `--screenshot --scene editor`: open the session on this thread and paint
/// one frame without a window.
pub fn render_offscreen(
    options: &Options,
    g: Gfx,
    tokens: Tokens,
    (w, h): (f32, f32),
) -> Result<Editor> {
    let root = root_of(options);
    let id = options.artifact.clone().unwrap_or_default();
    let fail = |e: String| Error::new(E_FAIL, e);
    let entry = find_entry(&root, &id).map_err(fail)?;
    let session = EditorSession::open_with_fonts(
        open_request(&root, &id),
        Some(captures_app::editor_fonts::bundled()),
    )
    .map_err(fail)?;
    let presented = present(&session, &id, &mut Published::default());
    let mode = entry.mode.unwrap_or(CaptureMode::Region);
    let mut editor = Editor::new(
        HWND::default(),
        g,
        tokens,
        options,
        id,
        &entry.created_at,
        mode,
        None,
    );
    editor.w = w;
    editor.h = h;
    editor.receive(Outcome::Presented(Box::new(presented)));
    editor.start_tool(options);
    if let Some([x, y, width, height]) = options.crop {
        editor.select_tool("c");
        editor.set_crop(Some(DocRect {
            x,
            y,
            width,
            height,
        }));
    }
    editor.prepare()?;
    Ok(editor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use captures_app::editor_image_background::BrushMode;
    use image::Rgba;

    /// Hardware pass 14: after two Erase strokes and two undos a row at the
    /// edge of stroke 1's damage stayed on screen. Drive the worker's
    /// `publish` and the UI's `patch` exactly as the editor does and check the
    /// UI's copy against the session after every step.
    #[test]
    fn ui_pixels_follow_brush_previews_commits_and_undos() {
        let data = std::env::temp_dir().join(format!("captures-win-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&data);
        let root = data.join("history");
        let source = RgbaImage::from_fn(640, 360, |x, y| {
            Rgba([(x * 3 % 256) as u8, (y * 5 % 256) as u8, 90, 255])
        });
        let capture =
            captures_app::persist_screenshot(&root, &source, CaptureMode::Region).unwrap();
        let mut session = EditorSession::open_with_fonts(
            open_request(&root, &capture.entry.id),
            Some(captures_app::editor_fonts::bundled()),
        )
        .unwrap();
        let mut published = Published::default();
        let mut ui: Option<Image> = None;
        let check = |session: &EditorSession, ui: &Option<Image>, step: &str| {
            let want = convert(&session.pixels(), (0, 0, 640, 360));
            let have = &ui.as_ref().unwrap().bgra;
            if let Some(i) = (0..want.len()).find(|&i| want[i] != have[i]) {
                let px = i / 4;
                panic!(
                    "{step}: UI pixel ({}, {}) is {:?}, session has {:?}",
                    px % 640,
                    px / 640,
                    &have[px * 4..px * 4 + 4],
                    &want[px * 4..px * 4 + 4]
                );
            }
        };
        let show = |pixels: Option<Pixels>, ui: &mut Option<Image>| {
            if let Some(pixels) = pixels {
                patch(ui, pixels);
            }
        };
        show(publish(session.pixels(), None, &mut published), &mut ui);
        check(&session, &ui, "open");
        let strokes: [Vec<Point>; 2] = [
            (0..40)
                .map(|i| Point {
                    x: 150. + f64::from(i) * 9.,
                    y: 120. + f64::from(i % 5) * 3.,
                })
                .collect(),
            (0..40)
                .map(|i| Point {
                    x: 160. + f64::from(i) * 9.,
                    y: 150. + f64::from(i % 7) * 2.5,
                })
                .collect(),
        ];
        let brush = |points: &[Point]| Request::PaintImageBackground {
            points: points.to_vec(),
            size: 28.,
            softness: 18.,
            mode: BrushMode::Erase,
        };
        for (s, stroke) in strokes.iter().enumerate() {
            for n in 1..=stroke.len() {
                let frame = session
                    .preview_drawing_with_geometry(brush(&stroke[..n]))
                    .unwrap();
                show(
                    publish(frame.pixels.clone(), frame.damage, &mut published),
                    &mut ui,
                );
                let want = convert(&frame.pixels, (0, 0, 640, 360));
                assert!(
                    want == ui.as_ref().unwrap().bgra,
                    "stroke {s} preview {n} differs from the UI"
                );
            }
            session.execute(brush(stroke)).unwrap();
            show(publish(session.pixels(), None, &mut published), &mut ui);
            check(&session, &ui, &format!("stroke {s} committed"));
        }
        for u in 1..=2 {
            session.execute(Request::Undo).unwrap();
            show(publish(session.pixels(), None, &mut published), &mut ui);
            check(&session, &ui, &format!("undo {u}"));
        }
        let _ = std::fs::remove_dir_all(&data);
    }
}
