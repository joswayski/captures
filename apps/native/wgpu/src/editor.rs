//! First connected screenshot editor: one serialized worker per open artifact.
//! Only snapshots and retained pixels cross to the UI; disk/render work does not.
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::{Duration, Instant},
};

use captures_app::{
    compression_compare as compare,
    editor::{
        ARROW_MIN_DRAW_LENGTH, AlignmentGuide, AnnotationStylePatch, ClosedShapeCreate,
        ClosedShapeKind, CropDrag, DEFAULT_ROTATION_SNAP_DEGREES, Document, DropShadowStyle,
        DropShadowStylePatch, Element, ElementBase, ElementStyle, FreehandPathCreate,
        GuideOrientation, ImageTransform, LayerEdit, LayerPlacement, MoveDrag, OpenShapeCreate,
        OpenShapeKind, OptionalNullable, Point, Rect, ResizeDrag, ResizeHandle, ShapeElement,
        TextElement, arrow_fill_polygon, preview_rotation, rotation_angle, rotation_handle,
        smooth_path_centerline,
    },
    editor_chrome::colors,
    editor_export::{
        self as export, EstimateState, ExportBarView, ExportEstimate, ExportSource, ExportTarget,
        FileSizeUnit, SavePlan,
    },
    editor_image_background::BrushMode,
    editor_output::SavedExport,
    editor_session::{
        DraftAutosave, EditorSession, ExportFormat, ExportOptions, ExportQuality, ExportSize,
        ImportImage, OpenRequest, PngOptions, Request, TextCreate, TextPatch,
    },
    editor_text::{TextStylePreset, font_family_options, shadow_style},
    editor_viewport::{Viewport, wheel_zoom_factor, zoom_from_slider, zoom_slider_position},
};
use captures_capture::CaptureMode;
use eframe::egui::{self, RichText};
use image::RgbaImage;

#[cfg(test)]
use captures_app::editor_render::MAX_RENDER_DIMENSION;
#[cfg(test)]
use image::ImageFormat;
#[cfg(test)]
use std::{fs::File, io::Cursor};

use crate::tokens::Tokens;

mod canvas;
mod chrome;
mod drawing_preview;
mod inspector;
mod layers;
mod pickers;
mod text_input;

type CompareReply = (u64, Result<(RgbaImage, u64), String>);
/// Shows a saved file in the file manager (`crate::reveal::reveal` in
/// editor windows; tests record the request instead).
type RevealFile = Arc<dyn Fn(&Path) -> std::io::Result<()> + Send + Sync>;

enum Job {
    Apply(Request),
    DrawingPreview {
        request: Request,
        epoch: u64,
        reply: Sender<drawing_preview::Reply>,
    },
    Import {
        path: PathBuf,
        selected_id: Option<String>,
        /// Document-space drop sample; `None` uses the default placement.
        point: Option<Point>,
    },
    Copy,
    Save {
        plan: SavePlan,
        options: ExportOptions,
    },
    Flush {
        input: Option<text_input::FlushInput>,
        reply: Sender<Result<(), String>>,
    },
    /// Shipping's debounced draft autosave. It runs behind accepted edits
    /// without blocking the editor and replies whether a draft now exists.
    Autosave {
        reply: Sender<Result<bool, String>>,
    },
    Shutdown,
}

struct Presented {
    document: Arc<Document>,
    /// Live Layers-row previews by layer ID (`captures_app::editor_layers`).
    thumbnails: BTreeMap<String, Arc<RgbaImage>>,
    /// Natural pixels of each visible image layer by source, shared with the
    /// session, so the Wand loupe samples on the UI thread.
    image_assets: BTreeMap<String, Arc<RgbaImage>>,
    pixels: Arc<RgbaImage>,
    original_export_path: Option<PathBuf>,
    initial_text_size: f64,
    font_families: BTreeMap<String, String>,
    text_style_presets: Vec<TextStylePreset>,
    saved: Option<(SavePlan, SavedExport)>,
    copied: bool,
    copied_layer: bool,
    pasted_layer: bool,
    created_layer: Option<String>,
    can_undo: bool,
    can_redo: bool,
    can_paste_layer: bool,
    merge_down_ids: Vec<String>,
    can_merge_visible: bool,
    can_flatten: bool,
    active_text_input: Option<(String, String)>,
    unsaved: bool,
    has_draft: bool,
}

impl Presented {
    /// After an edit: refresh the session's row thumbnails, then present.
    fn after_edit(session: &mut EditorSession) -> Self {
        session.refresh_layer_thumbnails();
        Self::from_session(session)
    }

    fn from_session(session: &EditorSession) -> Self {
        let snapshot = session.snapshot();
        Self {
            thumbnails: snapshot
                .document
                .elements
                .iter()
                .filter_map(|element| {
                    let id = &element.base().id;
                    session.layer_thumbnail(id).map(|image| (id.clone(), image))
                })
                .collect(),
            document: Arc::new(snapshot.document.clone()),
            image_assets: session.visible_image_assets(),
            pixels: session.pixels(),
            original_export_path: snapshot.original_export_path.map(Path::to_owned),
            initial_text_size: snapshot.initial_text_size,
            font_families: snapshot.font_families.cloned().unwrap_or_default(),
            text_style_presets: snapshot.text_style_presets,
            saved: None,
            copied: false,
            copied_layer: false,
            pasted_layer: false,
            created_layer: None,
            can_undo: snapshot.can_undo,
            can_redo: snapshot.can_redo,
            can_paste_layer: snapshot.can_paste_layer,
            merge_down_ids: snapshot
                .merge_down_ids
                .into_iter()
                .map(str::to_owned)
                .collect(),
            can_merge_visible: snapshot.can_merge_visible,
            can_flatten: snapshot.can_flatten,
            active_text_input: snapshot
                .active_text_input
                .map(|input| (input.input_id.to_owned(), input.layer_id.to_owned())),
            unsaved: snapshot.unsaved_changes,
            has_draft: snapshot.has_draft,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Section {
    Geometry,
    Layers,
    Draw,
}

/// Shipping debounce before the export size estimate re-encodes.
const ESTIMATE_DEBOUNCE: Duration = Duration::from_millis(220);
/// Shipping duration of the Copied / Saved confirmations.
const EXPORT_CONFIRMATION: Duration = Duration::from_secs(4);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DrawShape {
    Text,
    Rectangle,
    Ellipse,
    Line,
    Arrow,
    Freehand,
    Wand,
    Erase,
    Restore,
    Triangle,
    Diamond,
    Star,
}

impl DrawShape {
    /// `editor_chrome::draw_preview::stroke` tool key.
    fn preview_key(self) -> &'static str {
        match self {
            Self::Rectangle => "rectangle",
            Self::Ellipse => "ellipse",
            Self::Line => "line",
            Self::Arrow => "arrow",
            Self::Triangle => "triangle",
            Self::Diamond => "diamond",
            Self::Star => "star",
            Self::Freehand | Self::Text | Self::Wand | Self::Erase | Self::Restore => "pen",
        }
    }

    fn is_grouped(self) -> bool {
        self.closed_kind().is_some() || self == Self::Line
    }

    fn closed_kind(self) -> Option<ClosedShapeKind> {
        match self {
            Self::Rectangle => Some(ClosedShapeKind::Rectangle),
            Self::Ellipse => Some(ClosedShapeKind::Ellipse),
            Self::Triangle => Some(ClosedShapeKind::Triangle),
            Self::Diamond => Some(ClosedShapeKind::Diamond),
            Self::Star => Some(ClosedShapeKind::Star),
            _ => None,
        }
    }

    fn request(
        self,
        start: Point,
        end: Point,
        display_scale: f64,
        style: &ElementStyle,
        opacity: f64,
    ) -> Option<Request> {
        match self {
            Self::Rectangle | Self::Ellipse | Self::Triangle | Self::Diamond | Self::Star
                if start.x != end.x && start.y != end.y =>
            {
                Some(Request::CreateClosedShape {
                    create: ClosedShapeCreate {
                        shape: self.closed_kind().expect("closed shape"),
                        start,
                        end,
                        style: style.clone(),
                        opacity,
                    },
                })
            }
            Self::Line | Self::Arrow
                if self == Self::Line
                    || (end.x - start.x).hypot(end.y - start.y)
                        >= ARROW_MIN_DRAW_LENGTH.max(3. / display_scale.max(0.01)) =>
            {
                Some(Request::CreateOpenShape {
                    create: OpenShapeCreate {
                        shape: if self == Self::Line {
                            OpenShapeKind::Line
                        } else {
                            OpenShapeKind::Arrow
                        },
                        start,
                        end,
                        style: style.clone(),
                        opacity,
                    },
                })
            }
            Self::Text | Self::Wand | Self::Erase | Self::Restore => None,
            _ => None,
        }
    }
}

const CROP_ASPECTS: [(&str, Option<f64>); 5] = [
    ("Free", None),
    ("1 : 1", Some(1.)),
    ("4 : 3", Some(4. / 3.)),
    ("3 : 2", Some(3. / 2.)),
    ("16 : 9", Some(16. / 9.)),
];

fn output_preset(options: &ExportOptions) -> Option<&'static str> {
    if options.format == ExportFormat::Png && options.png.max_colors.is_some() {
        return None;
    }
    export::quality_preset(options.quality_value).map(|preset| preset.label)
}

#[derive(Clone, PartialEq)]
struct AnnotationFields {
    style: ElementStyle,
    shadow: DropShadowStyle,
}

#[derive(Clone, Debug, PartialEq)]
struct TextFields {
    id: String,
    accepted: TextValues,
    staged: TextValues,
    /// The staged values last sent as a live edit; unchanged fields are not
    /// resent while that edit is in flight.
    sent: Option<TextValues>,
}

#[derive(Clone, Debug, PartialEq)]
struct TextValues {
    text: String,
    font_size: f64,
    font_family: String,
    bold: bool,
    italic: bool,
    align: String,
    color: String,
    background: Option<String>,
    rounded_background: bool,
    drop_shadow: bool,
    shadow: DropShadowStyle,
    outlined: bool,
}

impl TextValues {
    fn from_element(text: &TextElement) -> Self {
        Self {
            text: text.text.clone(),
            font_size: text.font_size,
            font_family: text.font_family.clone(),
            bold: text.bold,
            italic: text.italic,
            align: text.align.clone(),
            color: text.color.clone(),
            background: text.background.clone(),
            rounded_background: text.rounded_background,
            drop_shadow: text.has_drop_shadow(),
            shadow: shadow_style(text, text.font_size).resolved_drop_shadow_style(),
            outlined: text.outlined,
        }
    }

    fn apply_preset(&mut self, preset: &TextStylePreset) {
        self.font_family = preset.font_family.into();
        self.background = preset
            .background
            .map(|color| self.background.clone().unwrap_or_else(|| color.into()));
        self.outlined = preset.outlined;
        self.rounded_background = preset.rounded_background;
    }

    fn patch(&self, accepted: &Self) -> TextPatch {
        let shadow = DropShadowStylePatch {
            color: (self.shadow.color != accepted.shadow.color).then(|| self.shadow.color.clone()),
            opacity: (self.shadow.opacity != accepted.shadow.opacity)
                .then_some(self.shadow.opacity),
            blur: (self.shadow.blur != accepted.shadow.blur).then_some(self.shadow.blur),
            offset_x: (self.shadow.offset_x != accepted.shadow.offset_x)
                .then_some(self.shadow.offset_x),
            offset_y: (self.shadow.offset_y != accepted.shadow.offset_y)
                .then_some(self.shadow.offset_y),
        };
        TextPatch {
            text: (self.text != accepted.text).then(|| self.text.clone()),
            font_size: (self.font_size != accepted.font_size).then_some(self.font_size),
            font_family: (self.font_family != accepted.font_family)
                .then(|| self.font_family.clone()),
            bold: (self.bold != accepted.bold).then_some(self.bold),
            italic: (self.italic != accepted.italic).then_some(self.italic),
            align: (self.align != accepted.align).then(|| self.align.clone()),
            color: (self.color != accepted.color).then(|| self.color.clone()),
            background: if self.background == accepted.background {
                OptionalNullable::Missing
            } else {
                self.background
                    .clone()
                    .map_or(OptionalNullable::Null, OptionalNullable::Value)
            },
            rounded_background: (self.rounded_background != accepted.rounded_background)
                .then_some(self.rounded_background),
            drop_shadow: (self.drop_shadow != accepted.drop_shadow).then_some(self.drop_shadow),
            drop_shadow_style: (self.drop_shadow && shadow != DropShadowStylePatch::default())
                .then_some(shadow),
            outlined: (self.outlined != accepted.outlined).then_some(self.outlined),
        }
    }
}

impl AnnotationFields {
    /// The live-undo field a change belongs to: a color or number burst folds
    /// into one undo step; a toggle (`None`) is its own step.
    fn changed_field(&self, before: &Self) -> Option<&'static str> {
        let (style, old) = (&self.style, &before.style);
        if style.has_stroke() != old.has_stroke()
            || style.fill.is_some() != old.fill.is_some()
            || style.has_drop_shadow() != old.has_drop_shadow()
        {
            return None;
        }
        let (shadow, previous) = (&self.shadow, &before.shadow);
        [
            (style.color != old.color, "stroke-color"),
            (style.stroke_width != old.stroke_width, "stroke-width"),
            (style.fill != old.fill, "fill-color"),
            (shadow.color != previous.color, "shadow-color"),
            (shadow.opacity != previous.opacity, "shadow-opacity"),
            (shadow.blur != previous.blur, "shadow-blur"),
            (shadow.offset_x != previous.offset_x, "shadow-x"),
            (shadow.offset_y != previous.offset_y, "shadow-y"),
        ]
        .into_iter()
        .find_map(|(changed, field)| changed.then_some(field))
    }

    fn new(style: &ElementStyle) -> Self {
        Self {
            style: style.clone(),
            shadow: style.resolved_drop_shadow_style(),
        }
    }

    fn patch(&self, original: &ElementStyle) -> AnnotationStylePatch {
        let previous_shadow = original.resolved_drop_shadow_style();
        let shadow = DropShadowStylePatch {
            color: (self.shadow.color != previous_shadow.color).then(|| self.shadow.color.clone()),
            opacity: (self.shadow.opacity != previous_shadow.opacity)
                .then_some(self.shadow.opacity),
            blur: (self.shadow.blur != previous_shadow.blur).then_some(self.shadow.blur),
            offset_x: (self.shadow.offset_x != previous_shadow.offset_x)
                .then_some(self.shadow.offset_x),
            offset_y: (self.shadow.offset_y != previous_shadow.offset_y)
                .then_some(self.shadow.offset_y),
        };
        AnnotationStylePatch {
            color: (self.style.color != original.color).then(|| self.style.color.clone()),
            fill: if self.style.fill == original.fill {
                OptionalNullable::Missing
            } else {
                self.style
                    .fill
                    .clone()
                    .map_or(OptionalNullable::Null, OptionalNullable::Value)
            },
            stroke_width: (self.style.stroke_width != original.stroke_width)
                .then_some(self.style.stroke_width),
            stroke_enabled: (self.style.has_stroke() != original.has_stroke())
                .then_some(self.style.has_stroke()),
            drop_shadow: (self.style.has_drop_shadow() != original.has_drop_shadow())
                .then_some(self.style.has_drop_shadow()),
            // Hidden controls must not accidentally re-enable a disabled shadow.
            drop_shadow_style: (self.style.has_drop_shadow()
                && shadow != DropShadowStylePatch::default())
            .then_some(shadow),
        }
    }
}

struct View {
    presented: Option<Presented>,
    texture: Option<egui::TextureHandle>,
    drawing_preview: Option<drawing_preview::State>,
    crop: [f64; 4],
    crop_previous: Option<[f64; 4]>,
    /// The rail's Crop tool is active: like shipping, a new selection can be
    /// dragged again after Apply crop, Clear or Escape.
    crop_tool: bool,
    crop_drag: Option<CropDrag>,
    crop_aspect: usize,
    draw_shape: DrawShape,
    last_background_tool: DrawShape,
    last_grouped_shape: DrawShape,
    rotation_snap_degrees: f64,
    new_text_preset: Option<String>,
    new_text_size: f64,
    new_annotation_style: ElementStyle,
    new_annotation_opacity: f64,
    wand_tolerance: f64,
    wand_contiguous: bool,
    brush_size: f64,
    brush_softness: f64,
    brush_points: Vec<Point>,
    shape_drag: Option<(Point, Point)>,
    /// Canvas rect at press. A live drawing preview can grow the canvas and
    /// re-center the viewport mid-gesture; pointer positions must keep mapping
    /// into the committed document the gesture started on.
    shape_drag_frame: egui::Rect,
    freehand_points: Vec<Point>,
    canvas: [f64; 2],
    /// Header Canvas W/H text while focused; otherwise the published size.
    canvas_text: [String; 2],
    /// Shipping's "Restored unsaved edits" banner for a draft found at open.
    draft_restored: bool,
    /// Shipping `lastSolid`: restored when Solid background is turned back on.
    last_solid_background: String,
    /// A live background change made while another job runs; the latest one
    /// is applied when the worker is free (one undo step).
    background_queued: Option<Option<String>>,
    section: Section,
    export_options: ExportOptions,
    custom_export_size: [u32; 2],
    export_aspect_locked: bool,
    /// The automatic before/after comparison's encoded After side and its
    /// byte length, for the current pixels and export options.
    output: Option<(egui::TextureHandle, u64)>,
    compare_key: Option<(u64, ExportOptions)>,
    compare_due: Option<Instant>,
    compare_generation: u64,
    compare_rx: Option<Receiver<CompareReply>>,
    compare_pending: bool,
    compare_error: Option<String>,
    /// Hide was chosen; Show before / after or a new quality mode returns it.
    compare_dismissed: bool,
    compare_split: f64,
    /// Maximum file size as typed, in `max_size_unit` (shipping's value + unit).
    max_size_text: String,
    max_size_unit: FileSizeUnit,
    artifact_id: String,
    default_directory: PathBuf,
    default_stem: String,
    export_target: Option<ExportTarget>,
    filename: String,
    export_settings_open: bool,
    estimate: EstimateState,
    estimate_key: Option<(u64, ExportOptions, Option<u64>)>,
    estimate_due: Option<Instant>,
    estimate_generation: u64,
    estimate_rx: Option<Receiver<(u64, Result<ExportEstimate, String>)>>,
    pixels_revision: u64,
    original_bytes: Option<u64>,
    last_saved: Option<PathBuf>,
    /// Save reveals the saved file's folder, as shipping does after every
    /// Save; a detached view (unit tests) without one never opens anything.
    reveal_file: Option<RevealFile>,
    /// The after-Save reveal in flight: the saved path and whether it opened.
    reveal_rx: Option<Receiver<(PathBuf, bool)>>,
    notice_until: Option<Instant>,
    copied_until: Option<Instant>,
    export_error: Option<String>,
    export_job: bool,
    folder_picker: Option<Receiver<Option<PathBuf>>>,
    output_notice: Option<String>,
    import_picker: Option<Receiver<Option<Vec<PathBuf>>>>,
    /// Image files dragged over or dropped on the canvas.
    drop: canvas::DropState,
    /// When Trim edges gained hover or keyboard focus while it can trim
    /// (egui seconds); the canvas previews the cut while set.
    trim_hover_since: Option<f64>,
    /// The Wand loupe's rendered magnifier, keyed by sampled source/pixel.
    wand_loupe: Option<canvas::LoupeTexture>,
    /// Curve slider value while dragging, committed once on release.
    curve_bend: Option<(String, f64)>,
    history_changed: bool,
    original_replaced: bool,
    selected_layer: Option<String>,
    layer_gesture: Option<LayerGesture>,
    pending_layer_selection: Option<String>,
    combine_pending: bool,
    layer_opacity: f64,
    /// Image Width, Height, X and Y as shown by the live number fields.
    layer_geometry: [f64; 4],
    /// Live inspector edits waiting for the worker: the newest per key, in
    /// order (see `View::live_edit`).
    live_queue: Vec<(String, Request)>,
    live_serial: u64,
    /// Sidebar Layers rows: drag, inline rename, ⋯ menu and thumbnails.
    layers: layers::State,
    annotation: Option<AnnotationFields>,
    text: Option<TextFields>,
    text_apply_pending: bool,
    inline: Option<text_input::InlineText>,
    pending: bool,
    closed: bool,
    /// Close was asked for; the draft flushes before the window closes.
    close_requested: bool,
    /// The close flush is in flight; its reply closes the window.
    close_after_save: bool,
    /// Shipping's 700 ms draft autosave timer.
    autosave: DraftAutosave,
    /// Editor windows autosave; a detached view (unit tests) never arms it.
    autosaves: bool,
    /// A background autosave in flight: whether a draft exists afterwards.
    autosave_rx: Option<Receiver<Result<bool, String>>>,
    /// When the pending autosave wake-up fires; one timer at a time.
    autosave_wake: Option<Instant>,
    error: Option<String>,
    viewport: Viewport,
    viewport_pan: Option<(egui::PointerButton, egui::Pos2)>,
    viewport_area: Option<egui::Rect>,
    viewport_image_size: Option<egui::Vec2>,
    viewport_intercepted: bool,
}

impl Default for View {
    fn default() -> Self {
        Self {
            presented: None,
            texture: None,
            drawing_preview: None,
            crop: [0., 0., 1., 1.],
            crop_previous: None,
            crop_tool: false,
            crop_drag: None,
            crop_aspect: 0,
            draw_shape: DrawShape::Rectangle,
            last_background_tool: DrawShape::Wand,
            last_grouped_shape: DrawShape::Rectangle,
            rotation_snap_degrees: DEFAULT_ROTATION_SNAP_DEGREES,
            new_text_preset: None,
            new_text_size: 24.,
            new_annotation_style: ElementStyle::default(),
            new_annotation_opacity: 100.,
            wand_tolerance: 36.,
            wand_contiguous: true,
            brush_size: 28.,
            brush_softness: 18.,
            brush_points: Vec::new(),
            shape_drag: None,
            shape_drag_frame: egui::Rect::NOTHING,
            freehand_points: Vec::new(),
            canvas: [1., 1.],
            canvas_text: [String::new(), String::new()],
            draft_restored: false,
            last_solid_background: colors::DEFAULT_CANVAS_BACKGROUND.into(),
            background_queued: None,
            section: Section::Layers,
            export_options: ExportOptions {
                format: ExportFormat::Png,
                quality: ExportQuality::Preserve,
                quality_value: 80,
                max_size_bytes: None,
                png: PngOptions::default(),
                size: ExportSize::Original,
            },
            custom_export_size: [1, 1],
            export_aspect_locked: true,
            output: None,
            compare_key: None,
            compare_due: None,
            compare_generation: 0,
            compare_rx: None,
            compare_pending: false,
            compare_error: None,
            compare_dismissed: false,
            compare_split: compare::DEFAULT_SPLIT,
            max_size_text: FileSizeUnit::Mb.value(export::DEFAULT_MAX_SIZE_BYTES),
            max_size_unit: FileSizeUnit::Mb,
            artifact_id: String::new(),
            default_directory: PathBuf::new(),
            default_stem: String::new(),
            export_target: None,
            filename: String::new(),
            export_settings_open: false,
            estimate: EstimateState::default(),
            estimate_key: None,
            estimate_due: None,
            estimate_generation: 0,
            estimate_rx: None,
            pixels_revision: 0,
            original_bytes: None,
            last_saved: None,
            reveal_file: None,
            reveal_rx: None,
            notice_until: None,
            copied_until: None,
            export_error: None,
            export_job: false,
            folder_picker: None,
            output_notice: None,
            import_picker: None,
            drop: canvas::DropState::default(),
            trim_hover_since: None,
            wand_loupe: None,
            curve_bend: None,
            history_changed: false,
            original_replaced: false,
            selected_layer: None,
            layer_gesture: None,
            pending_layer_selection: None,
            combine_pending: false,
            layer_opacity: 100.,
            layer_geometry: [1., 1., 0., 0.],
            live_queue: Vec::new(),
            live_serial: 0,
            layers: layers::State::default(),
            annotation: None,
            text: None,
            text_apply_pending: false,
            inline: None,
            pending: true,
            closed: false,
            close_requested: false,
            close_after_save: false,
            autosave: DraftAutosave::default(),
            autosaves: false,
            autosave_rx: None,
            autosave_wake: None,
            error: None,
            viewport: Viewport::default(),
            viewport_pan: None,
            viewport_area: None,
            viewport_image_size: None,
            viewport_intercepted: false,
        }
    }
}

impl View {
    fn tool_shows_transform_chrome(&self) -> bool {
        self.section == Section::Layers
            || (self.section == Section::Draw
                && (self.draw_shape.is_grouped() || self.draw_shape == DrawShape::Arrow))
    }

    fn properties_section(&self) -> Section {
        if self.section == Section::Draw && self.selected_layer.is_some() {
            Section::Layers
        } else {
            self.section
        }
    }

    fn activate_tool(&mut self, section: Section, shape: Option<DrawShape>) {
        // Shipping clears selection even when reactivating the current tool.
        if section != Section::Layers {
            self.select_layer_exact(None);
        }
        let active = self.section == section
            && match shape {
                Some(shape) => self.draw_shape == shape,
                None => section == Section::Layers || self.crop_previous.is_some(),
            };
        if active {
            return;
        }
        self.cancel_edit_gestures();
        self.cancel_crop();
        self.viewport_pan = None;
        self.section = section;
        self.crop_tool = section == Section::Geometry && shape.is_none();
        if let Some(shape) = shape {
            self.draw_shape = shape;
            if shape.is_grouped() {
                self.last_grouped_shape = shape;
            }
        } else if section == Section::Geometry {
            self.crop_previous = Some(self.crop);
        }
    }

    fn cancel_crop(&mut self) {
        if let Some(previous) = self.crop_previous.take() {
            self.crop = previous;
        }
        self.crop_drag = None;
    }

    fn cancel_drawing(&mut self) {
        self.shape_drag = None;
        self.freehand_points.clear();
        self.brush_points.clear();
        if let Some(preview) = &mut self.drawing_preview {
            preview.cancel();
        }
    }

    fn cancel_layer_gesture(&mut self) {
        self.layer_gesture = None;
    }

    fn cancel_edit_gestures(&mut self) {
        self.crop_drag = None;
        self.cancel_drawing();
        self.cancel_layer_gesture();
    }

    fn reset_viewport(&mut self) {
        self.cancel_edit_gestures();
        self.viewport = Viewport::default();
        self.viewport_pan = None;
    }

    /// Shipping's window title. The " — Working…" suffix while a job runs is
    /// a native addition that the X11 exercises wait on.
    fn title(&self) -> &'static str {
        if self.pending {
            "Captures Screenshot Editor — Working…"
        } else {
            captures_app::app_windows::SCREENSHOT_EDITOR_TITLE
        }
    }

    fn unsaved(&self) -> bool {
        self.presented.as_ref().is_some_and(|value| value.unsaved)
    }

    fn request_close(&mut self) {
        if self.close_inline() {
            return;
        }
        self.cancel_drawing();
        self.cancel_layer_gesture();
        self.pending_layer_selection = None;
        if self
            .text
            .as_ref()
            .is_some_and(|fields| fields.staged.patch(&fields.accepted) != TextPatch::default())
        {
            self.error = Some("Apply or cancel pending text before closing.".into());
            self.section = Section::Layers;
            return;
        }
        // Shipping closes without asking and flushes the draft (see
        // `View::drive_close`).
        self.close_requested = true;
    }

    /// Close once nothing is in flight: flush a pending or unsaved draft on
    /// the worker first (the reply closes the window), else close now.
    fn drive_close(&mut self, tx: &Sender<Job>) {
        if !self.close_requested || self.close_after_save || self.pending {
            return;
        }
        if self.inline.is_some() || !self.live_queue.is_empty() {
            return;
        }
        if self.presented.is_some() && self.autosave.take_flush(self.unsaved()) {
            self.close_after_save = true;
            self.submit(tx, Self::autosave_request());
            if !self.pending {
                // The worker is gone; the last saved draft stays on disk.
                self.closed = true;
            }
        } else {
            self.closed = true;
        }
    }

    fn autosave_request() -> Request {
        Request::AutosaveDraft {
            updated_at_ms: chrono::Utc::now().timestamp_millis().max(0) as u64,
        }
    }

    /// Start a due autosave behind any accepted edits, and keep the frame
    /// loop awake until the next one is due.
    fn drive_autosave(&mut self, ctx: &egui::Context, tx: &Sender<Job>) {
        let now = Instant::now();
        if !self.pending
            && self.inline.is_none()
            && self.autosave_rx.is_none()
            && !self.close_requested
            && self.presented.is_some()
            && self.autosave.take_due(now)
        {
            let (reply, rx) = mpsc::channel();
            if tx.send(Job::Autosave { reply }).is_ok() {
                self.autosave_rx = Some(rx);
            }
        }
        // A repaint request alone does not wake an idle deferred viewport, so a
        // timer thread wakes it like a worker reply does. A pending timer that
        // fires before a later deadline just schedules the next one.
        if let Some(due) = self.autosave.deadline()
            && self.autosave_wake.is_none_or(|wake_at| wake_at <= now)
        {
            self.autosave_wake = Some(due);
            let ctx = ctx.clone();
            let viewport = ctx.viewport_id();
            thread::spawn(move || {
                thread::sleep(due.saturating_duration_since(Instant::now()));
                wake(&ctx, viewport);
            });
        }
    }

    /// Apply a finished background autosave. Its reply always precedes the
    /// replies of edits queued after it, so it is read first.
    fn receive_autosave(&mut self) -> bool {
        let Some(result) = self.autosave_rx.as_ref().map(Receiver::try_recv) else {
            return false;
        };
        let result = match result {
            Err(mpsc::TryRecvError::Empty) => return false,
            Err(mpsc::TryRecvError::Disconnected) => Err(String::new()),
            Ok(result) => result,
        };
        self.autosave_rx = None;
        // A failed autosave stays quiet, as in shipping: the next edit or
        // the close flush retries.
        if let (Ok(has_draft), Some(presented)) = (result, &mut self.presented) {
            presented.unsaved = false;
            presented.has_draft = has_draft;
            if !has_draft {
                self.draft_restored = false;
            }
        }
        true
    }

    /// The restored-draft notice's Discard: back to the original capture.
    fn discard_draft(&mut self, tx: &Sender<Job>) {
        self.draft_restored = false;
        self.autosave.cancel();
        self.submit(tx, Request::DiscardDraft);
    }

    fn receive(&mut self, ctx: &egui::Context, result: Result<Presented, String>) {
        if self.closed {
            return;
        }
        self.pending = false;
        let export_job = std::mem::take(&mut self.export_job);
        match result {
            Ok(mut presented) => {
                if self.presented.is_none() {
                    // A draft present at open was restored, as in shipping.
                    self.draft_restored = presented.has_draft;
                    self.new_text_size = presented.initial_text_size;
                    self.new_text_preset = presented
                        .text_style_presets
                        .iter()
                        .find(|preset| preset.id == "rounded-box")
                        .or_else(|| {
                            presented
                                .text_style_presets
                                .iter()
                                .find(|preset| preset.id == "standard")
                        })
                        .map(|preset| preset.id.to_owned());
                }
                let changed = self
                    .presented
                    .as_ref()
                    .is_none_or(|old| !Arc::ptr_eq(&old.pixels, &presented.pixels));
                if changed {
                    self.invalidate_output();
                    self.output_notice = None;
                    self.pixels_revision += 1;
                    self.cancel_crop();
                    self.cancel_drawing();
                    self.viewport_pan = None;
                    let image = &presented.pixels;
                    self.texture = Some(ctx.load_texture(
                        "edited-screenshot",
                        egui::ColorImage::from_rgba_unmultiplied(
                            [image.width() as usize, image.height() as usize],
                            image.as_raw(),
                        ),
                        egui::TextureOptions::LINEAR,
                    ));
                    self.canvas = [f64::from(image.width()), f64::from(image.height())];
                    self.crop = [0., 0., self.canvas[0], self.canvas[1]];
                }
                if let Some((plan, saved)) = presented.saved.take() {
                    self.output_notice = Some(export::saved_notice(&plan, &saved));
                    self.notice_until = Some(Instant::now() + EXPORT_CONFIRMATION);
                    let (SavedExport::Saved { path, .. }
                    | SavedExport::SavedWithoutHistory { path, .. }) = &saved;
                    self.last_saved = Some(path.clone());
                    self.reveal_after_save(ctx, path.clone());
                    if let SavedExport::Saved { artifact, .. } = &saved {
                        self.history_changed = true;
                        // The saved file becomes the original, as in the shipping app.
                        self.original_bytes =
                            Some(artifact.entry.size_bytes).filter(|bytes| *bytes > 0);
                    }
                    if let SavePlan::Overwrite { artifact_id, .. } = &plan {
                        self.history_changed = true;
                        self.original_replaced |= *artifact_id == self.artifact_id;
                    }
                    if let Some(target) = &mut self.export_target {
                        target.record_saved(&saved);
                        self.filename.clone_from(&target.stem);
                    }
                }
                if presented.copied {
                    self.copied_until = Some(Instant::now() + EXPORT_CONFIRMATION);
                }
                if export_job {
                    self.export_error = None;
                }
                let copied_layer = presented.copied_layer;
                let pasted_layer = presented.pasted_layer;
                let combined_layers = std::mem::take(&mut self.combine_pending);
                let selected = self.pending_layer_selection.take().or_else(|| {
                    presented
                        .created_layer
                        .take()
                        .or(self.selected_layer.clone())
                });
                if !presented.has_draft {
                    self.draft_restored = false;
                }
                // Shipping autosaves 700 ms after each change to the document.
                if self.autosaves
                    && presented.unsaved
                    && self
                        .presented
                        .as_ref()
                        .is_none_or(|old| !old.unsaved || old.document != presented.document)
                {
                    self.autosave.edited(Instant::now());
                } else if !presented.unsaved {
                    self.autosave.cancel();
                }
                self.presented = Some(presented);
                self.ensure_export_target();
                if !copied_layer {
                    self.select_layer_exact(selected);
                }
                self.text_apply_pending = false;
                if pasted_layer || combined_layers {
                    self.activate_tool(Section::Layers, None);
                }
                if !copied_layer {
                    self.reset_background_fields();
                }
                self.error = None;
                self.received_inline();
            }
            Err(error) => {
                self.pending_layer_selection = None;
                self.combine_pending = false;
                if export_job {
                    // Save and copy failures belong to the export bar's status line.
                    self.export_error = Some(error);
                } else {
                    self.error = Some(error);
                }
                self.inline_failed();
                self.live_queue.clear();
                // A rejected explicit text Apply keeps the user's staged composition.
                if !self.text_apply_pending {
                    self.select_layer_exact(self.selected_layer.clone());
                }
                self.text_apply_pending = false;
                self.reset_background_fields();
            }
        }
        // The close flush is best-effort, as in shipping: the window closes
        // even when the draft could not be written.
        if std::mem::take(&mut self.close_after_save) {
            self.closed = true;
        }
    }

    fn submit(&mut self, tx: &Sender<Job>, request: Request) {
        self.submit_job(tx, Job::Apply(request));
    }

    fn reset_background_fields(&mut self) {
        if let Some(color) = self
            .presented
            .as_ref()
            .and_then(|presented| presented.document.background.as_ref())
        {
            self.last_solid_background.clone_from(color);
        }
    }

    /// The background the card shows: a queued live change, else the document's.
    fn shown_background(&self) -> Option<String> {
        self.background_queued.clone().unwrap_or_else(|| {
            self.presented
                .as_ref()
                .and_then(|presented| presented.document.background.clone())
        })
    }

    /// Shipping applies each background change at once as its own undo step;
    /// an unchanged value adds none (the session skips identical commits).
    fn set_background(&mut self, tx: &Sender<Job>, color: Option<String>) {
        if let Some(color) = &color {
            self.last_solid_background.clone_from(color);
        }
        if self.pending {
            self.background_queued = Some(color);
        } else {
            self.background_queued = None;
            self.submit(tx, Request::SetBackground { color });
        }
    }

    fn flush_background(&mut self, tx: &Sender<Job>) {
        if !self.pending
            && self.inline.is_none()
            && let Some(color) = self.background_queued.take()
        {
            self.submit(tx, Request::SetBackground { color });
        }
    }

    /// The After side no longer matches the pixels or options; the
    /// comparison re-encodes on its own schedule while it is shown.
    fn invalidate_output(&mut self) {
        self.output = None;
    }

    /// Build the export target once the session reports its saved original.
    fn ensure_export_target(&mut self) {
        if self.export_target.is_some() {
            return;
        }
        let Some(presented) = &self.presented else {
            return;
        };
        let source = presented
            .original_export_path
            .clone()
            .map(|path| ExportSource {
                artifact_id: self.artifact_id.clone(),
                path,
            });
        let target = ExportTarget::new(source, &self.default_directory, &self.default_stem);
        self.filename.clone_from(&target.stem);
        self.export_target = Some(target);
    }

    fn export_view(&self) -> Option<ExportBarView> {
        let target = self.export_target.as_ref()?;
        let presented = self.presented.as_ref()?;
        Some(export::present(
            target,
            self.export_options,
            presented.pixels.dimensions(),
            presented.document.background.is_none(),
            self.estimate,
        ))
    }

    /// A different file, folder or encoding is no longer the saved result.
    fn export_target_changed(&mut self) {
        self.last_saved = None;
        self.output_notice = None;
        self.notice_until = None;
        self.export_error = None;
    }

    fn update_export_target(&mut self, update: impl FnOnce(&mut ExportTarget, ExportFormat)) {
        let format = self.export_options.format;
        if let Some(target) = &mut self.export_target {
            update(target, format);
            self.filename.clone_from(&target.stem);
        }
        self.export_target_changed();
    }

    fn save(&mut self, tx: &Sender<Job>) {
        let Some(bar) = self.export_view() else {
            return;
        };
        let (Some(plan), None) = (bar.plan, bar.error.clone()) else {
            self.export_error = bar.error;
            return;
        };
        self.output_notice = None;
        self.notice_until = None;
        self.export_error = None;
        self.export_job = self.submit_job(
            tx,
            Job::Save {
                plan,
                options: self.export_options,
            },
        );
    }

    fn copy(&mut self, tx: &Sender<Job>) {
        self.export_error = None;
        self.export_job = self.submit_job(tx, Job::Copy);
    }

    fn reveal_saved(&mut self) {
        let Some(path) = self.last_saved.clone() else {
            return;
        };
        if !path.is_file() {
            self.export_error = Some(format!(
                "Couldn’t show the saved file: saved file no longer exists: {}",
                path.display()
            ));
            return;
        }
        // Asking the file manager over D-Bus can wait for a service to start;
        // keep that off the UI thread like the preview card's Show in Folder.
        std::thread::spawn(move || {
            if let Err(error) = crate::reveal::reveal(&path) {
                eprintln!("Couldn’t show the saved file: {error}");
            }
        });
    }

    /// Shipping `saveEditedImage` reveals the saved file after every Save
    /// (overwrite or new file); the file manager handoff stays off the UI
    /// thread like Show in Folder.
    fn reveal_after_save(&mut self, ctx: &egui::Context, path: PathBuf) {
        let Some(reveal) = self.reveal_file.clone() else {
            return;
        };
        let (tx, rx) = mpsc::channel();
        self.reveal_rx = Some(rx);
        let ctx = ctx.clone();
        let viewport = ctx.viewport_id();
        thread::spawn(move || {
            let revealed = reveal(&path).is_ok();
            let _ = tx.send((path, revealed));
            ctx.request_repaint_of(viewport);
        });
    }

    /// The file is on disk either way; only a failed handoff changes the
    /// notice, and only while it still describes that save.
    fn drive_reveal(&mut self) {
        let Some(rx) = &self.reveal_rx else {
            return;
        };
        match rx.try_recv() {
            Ok((path, revealed)) => {
                self.reveal_rx = None;
                if !revealed && self.last_saved.as_ref() == Some(&path) {
                    self.output_notice = Some(export::reveal_failed_notice(&path));
                    self.notice_until = Some(Instant::now() + EXPORT_CONFIRMATION);
                }
            }
            Err(mpsc::TryRecvError::Disconnected) => self.reveal_rx = None,
            Err(mpsc::TryRecvError::Empty) => {}
        }
    }

    /// Re-encode the export in the background after edits or option changes
    /// settle, exactly as Save would, without occupying the session worker.
    fn drive_estimate(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.estimate_rx {
            match rx.try_recv() {
                Ok((generation, result)) => {
                    self.estimate_rx = None;
                    if generation == self.estimate_generation {
                        self.estimate = match result {
                            Ok(estimate) => EstimateState {
                                bytes: Some(estimate.bytes),
                                baseline_bytes: estimate.baseline_bytes,
                                pending: false,
                            },
                            Err(_) => EstimateState::default(),
                        };
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => self.estimate_rx = None,
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        let Some(presented) = &self.presented else {
            return;
        };
        let pixels = presented.pixels.clone();
        let key = (
            self.pixels_revision,
            self.export_options,
            self.original_bytes,
        );
        if self.estimate_key != Some(key) {
            self.estimate_key = Some(key);
            self.estimate_generation += 1;
            self.estimate.pending = true;
            self.estimate_due = Some(Instant::now() + ESTIMATE_DEBOUNCE);
        }
        let Some(due) = self.estimate_due else {
            return;
        };
        let now = Instant::now();
        if now < due {
            ctx.request_repaint_after(due - now);
            return;
        }
        if self.estimate_rx.is_some() {
            return; // A superseded encode finishes first; its result is dropped.
        }
        self.estimate_due = None;
        let (width, height) = pixels.dimensions();
        if export::validate_options(self.export_options, width, height).is_err() {
            self.estimate = EstimateState::default();
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.estimate_rx = Some(rx);
        let (generation, options, original) = (
            self.estimate_generation,
            self.export_options,
            self.original_bytes,
        );
        let ctx = ctx.clone();
        let viewport = ctx.viewport_id();
        thread::spawn(move || {
            let _ = tx.send((
                generation,
                export::estimate_export(&pixels, options, original),
            ));
            ctx.request_repaint_of(viewport);
        });
    }

    /// Shipping's automatic comparison: Compress or Maximum with Export
    /// settings open, until Hide.
    fn compare_visible(&self) -> bool {
        self.presented.is_some()
            && compare::screenshot_visible(
                self.export_options.quality != ExportQuality::Preserve,
                self.export_settings_open,
                self.compare_dismissed,
            )
    }

    /// A canvas gesture or inline text fades the comparison away so the live
    /// canvas can be edited underneath (shipping `suppressed`).
    fn compare_suppressed(&self) -> bool {
        self.inline.is_some()
            || self.layer_gesture.is_some()
            || self.shape_drag.is_some()
            || !self.freehand_points.is_empty()
            || !self.brush_points.is_empty()
            || self.crop_drag.is_some()
    }

    /// Encode the After side off the session worker once edits or options
    /// settle (shipping's 280 ms refresh), exactly as Save would.
    fn drive_compare(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.compare_rx {
            match rx.try_recv() {
                Ok((generation, result)) => {
                    self.compare_rx = None;
                    if generation == self.compare_generation {
                        self.compare_pending = false;
                        match result {
                            Ok((image, bytes)) => {
                                self.output = Some((
                                    ctx.load_texture(
                                        "encoded-screenshot",
                                        egui::ColorImage::from_rgba_unmultiplied(
                                            [image.width() as usize, image.height() as usize],
                                            image.as_raw(),
                                        ),
                                        egui::TextureOptions::LINEAR,
                                    ),
                                    bytes,
                                ));
                                self.compare_error = None;
                            }
                            Err(error) => {
                                self.output = None;
                                self.compare_error = Some(error);
                            }
                        }
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => self.compare_rx = None,
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if !self.compare_visible() {
            return;
        }
        let Some(presented) = &self.presented else {
            return;
        };
        let pixels = presented.pixels.clone();
        let key = (self.pixels_revision, self.export_options);
        let now = Instant::now();
        if self.compare_key != Some(key) {
            self.compare_key = Some(key);
            self.compare_generation += 1;
            self.compare_pending = true;
            self.compare_error = None;
            self.compare_due =
                Some(now + Duration::from_millis(compare::SCREENSHOT_REFRESH_DELAY_MS));
        }
        let Some(due) = self.compare_due else {
            return;
        };
        if now < due {
            ctx.request_repaint_after(due - now);
            return;
        }
        if self.compare_rx.is_some() {
            return; // A superseded encode finishes first; its result is dropped.
        }
        self.compare_due = None;
        let options = self.export_options;
        let (width, height) = pixels.dimensions();
        if let Err(error) = export::validate_options(options, width, height) {
            self.compare_pending = false;
            self.output = None;
            self.compare_error = Some(error);
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.compare_rx = Some(rx);
        let generation = self.compare_generation;
        let ctx = ctx.clone();
        let viewport = ctx.viewport_id();
        thread::spawn(move || {
            let _ = tx.send((generation, compare::encode_after(&pixels, options)));
            ctx.request_repaint_of(viewport);
        });
    }

    /// Before is the lossless flattened edit (the estimate's baseline while
    /// compressing), After the encoded file.
    fn compare_badges(&self) -> compare::Badges {
        compare::badges(
            self.estimate.baseline_bytes,
            self.output.as_ref().map(|(_, bytes)| *bytes),
            self.compare_pending,
        )
    }

    /// The Maximum file size field in bytes; unparseable text is an invalid
    /// cap that the export bar explains, never an unbounded export.
    fn max_size_bytes(&self) -> u64 {
        self.max_size_unit.bytes(&self.max_size_text).unwrap_or(0)
    }

    fn set_max_size_unit(&mut self, unit: FileSizeUnit) {
        if let Some(bytes) = self.max_size_unit.bytes(&self.max_size_text) {
            self.max_size_text = unit.value(bytes);
        }
        self.max_size_unit = unit;
    }

    fn choose_folder(&mut self, ctx: &egui::Context) {
        if self.folder_picker.is_some() {
            return;
        }
        let directory = self
            .export_target
            .as_ref()
            .map(|target| target.directory.clone())
            .unwrap_or_default();
        let (tx, rx) = mpsc::channel();
        self.folder_picker = Some(rx);
        let ctx = ctx.clone();
        let viewport = ctx.viewport_id();
        // A native dialog must not occupy the session worker: closing/quit still
        // drains edits and saves drafts without waiting for a folder selection.
        thread::spawn(move || {
            let selected = rfd::FileDialog::new()
                .set_title("Choose save location")
                .set_directory(directory)
                .pick_folder();
            let _ = tx.send(selected);
            wake(&ctx, viewport);
        });
    }

    fn choose_image(&mut self, ctx: &egui::Context) {
        if self.import_picker.is_some() {
            return;
        }
        self.cancel_layer_gesture();
        let (tx, rx) = mpsc::channel();
        self.import_picker = Some(rx);
        let ctx = ctx.clone();
        let viewport = ctx.viewport_id();
        // Picking is independent of the session worker: quit and draft saves
        // never wait for a dialog. Decode and import happen on the worker later.
        thread::spawn(move || {
            // Shipping's `<input type="file" multiple>`: every chosen image
            // becomes a layer through the canvas-drop import queue.
            let selected = rfd::FileDialog::new()
                .set_title("Import images")
                .add_filter(
                    "Images (PNG, JPEG, WebP, TIFF)",
                    &["png", "jpg", "jpeg", "webp", "tif", "tiff"],
                )
                .pick_files();
            let _ = tx.send(selected);
            wake(&ctx, viewport);
        });
    }

    fn receive_folder(&mut self) -> bool {
        let Some(result) = self.folder_picker.as_ref().map(Receiver::try_recv) else {
            return false;
        };
        if matches!(result, Err(mpsc::TryRecvError::Empty)) {
            return false;
        }
        self.folder_picker = None;
        if self.closed {
            return false;
        }
        match result {
            Ok(Some(directory)) => {
                self.update_export_target(|target, _| target.set_directory(directory));
                self.error = None;
            }
            Ok(None) => {} // Cancellation preserves the path and current session.
            Err(_) => self.error = Some("Save location could not be changed. Try again.".into()),
        }
        true
    }

    fn receive_import(&mut self, tx: &Sender<Job>) -> bool {
        if self.closed || self.close_requested {
            self.import_picker = None;
            return false;
        }
        // Preserve the one-in-flight edit contract. A selected file waits until
        // an accepted edit or a discard confirmation has finished.
        if self.pending || self.inline.is_some() {
            return false;
        }
        let Some(result) = self.import_picker.as_ref().map(Receiver::try_recv) else {
            return false;
        };
        if matches!(result, Err(mpsc::TryRecvError::Empty)) {
            return false;
        }
        self.import_picker = None;
        match result {
            Ok(Some(paths)) => {
                // Like a canvas drop without a pointer: the first image takes
                // shipping's default placement and later ones stack below the
                // layer the previous one created, one edit at a time.
                let paths: Vec<PathBuf> = paths
                    .into_iter()
                    .filter(|path| captures_app::editor_session::is_supported_image_path(path))
                    .collect();
                if paths.is_empty() {
                    self.error = Some(captures_app::editor_session::DROP_UNSUPPORTED.into());
                } else {
                    self.drop
                        .queue
                        .extend(paths.into_iter().map(|path| (path, None)));
                    canvas::drain_drops(self, tx);
                }
            }
            Ok(None) => {}
            Err(_) => self.error = Some("Image selection failed. Try again.".into()),
        }
        true
    }

    fn submit_job(&mut self, tx: &Sender<Job>, job: Job) -> bool {
        if self.inline.is_some()
            && !matches!(
                &job,
                Job::Apply(
                    Request::BeginTextInput { .. }
                        | Request::UpdateTextInput { .. }
                        | Request::FinishTextInput { .. }
                )
            )
        {
            self.error = Some("Finish or cancel text input before another editor action.".into());
            return false;
        }
        self.cancel_layer_gesture();
        if let Some(preview) = &mut self.drawing_preview {
            preview.cancel();
        }
        match tx.send(job) {
            Ok(()) => {
                self.pending = true;
                self.error = None;
                true
            }
            Err(_) => {
                self.pending_layer_selection = None;
                self.error =
                    Some("The editor worker stopped. Your last saved draft is preserved.".into());
                false
            }
        }
    }

    fn select_layer(&mut self, id: Option<String>) {
        let previous_text = self.text.clone();
        let previous_layer = self.selected_layer.clone();
        let previous_annotation = self.annotation.take();
        let elements = self
            .presented
            .as_ref()
            .map(|value| &value.document.elements);
        let layer = elements.and_then(|elements| {
            elements
                .iter()
                .find(|element| Some(&element.base().id) == id.as_ref())
                .or_else(|| elements.last())
        });
        self.selected_layer = layer.map(|element| element.base().id.clone());
        // Live style edits still queued keep the fields the user is changing.
        let keep = !self.live_queue.is_empty() && previous_layer == self.selected_layer;
        self.annotation = layer.and_then(|element| match element {
            Element::Shape(shape) => Some(AnnotationFields::new(&shape.style)),
            Element::Path(path) => Some(AnnotationFields::new(&path.style)),
            _ => None,
        });
        if keep && self.annotation.is_some() && previous_annotation.is_some() {
            self.annotation = previous_annotation;
        }
        self.text = layer.and_then(|element| match element {
            Element::Text(text) => {
                let accepted = TextValues::from_element(text);
                let previous = previous_text.filter(|fields| fields.id == text.base.id);
                let sent = previous.as_ref().and_then(|fields| fields.sent.clone());
                let staged = previous
                    .filter(|fields| {
                        fields.staged.patch(&fields.accepted) != TextPatch::default()
                            && !self.text_apply_pending
                    })
                    .map_or_else(|| accepted.clone(), |fields| fields.staged);
                Some(TextFields {
                    id: text.base.id.clone(),
                    accepted,
                    staged,
                    sent,
                })
            }
            _ => None,
        });
        if let Some(layer) = layer
            && self.live_queue.is_empty()
        {
            self.layer_opacity = layer.base().opacity;
            let base = layer.base();
            self.layer_geometry = match layer {
                Element::Image(image) => [image.width, image.height, base.x, base.y],
                _ => [1., 1., base.x, base.y],
            };
        }
    }

    fn select_layer_exact(&mut self, id: Option<String>) {
        if id.is_none() {
            self.selected_layer = None;
            self.annotation = None;
            self.text = None;
            self.layer_opacity = 100.;
            self.layer_geometry = [1., 1., 0., 0.];
        } else {
            self.select_layer(id);
        }
    }

    /// Shipping live inspector edits (typing, steppers, sliders) apply at once.
    /// Edits sharing `key` fold into one undo step in the session; while a
    /// job runs, the newest edit per key waits here in order.
    fn live_edit(&mut self, tx: &Sender<Job>, key: String, request: Request) {
        match self.live_queue.last_mut() {
            Some(last) if last.0 == key => last.1 = request,
            _ => self.live_queue.push((key, request)),
        }
        self.flush_live(tx);
    }

    /// A key for one discrete live change (a toggle or menu choice): its own
    /// undo step, still ordered behind queued edits.
    fn live_once(&mut self, kind: &str) -> String {
        self.live_serial += 1;
        format!("{kind}:once:{}", self.live_serial)
    }

    fn flush_live(&mut self, tx: &Sender<Job>) {
        if self.pending || self.inline.is_some() || self.live_queue.is_empty() {
            return;
        }
        let (key, request) = self.live_queue.remove(0);
        self.submit(
            tx,
            Request::Live {
                key,
                request: Box::new(request),
            },
        );
    }
}

#[derive(Clone)]
enum LayerGestureKind {
    Move {
        id: Option<String>,
        drag: Option<Box<MoveDrag>>,
        outline: Option<[Point; 4]>,
        guides: Vec<AlignmentGuide>,
        display_scale: f64,
    },
    Rotate {
        id: String,
        outline: [Point; 4],
        initial_radians: f64,
        snap: bool,
    },
    Resize {
        id: String,
        handle: ResizeHandle,
        drag: Box<ResizeDrag>,
        outline: [Point; 4],
        guides: Vec<AlignmentGuide>,
        lock_aspect: bool,
        display_scale: f64,
    },
    /// Dragging a line/arrow endpoint, curve dot or starter dot.
    Curve {
        id: String,
        handle: captures_app::editor_canvas::CurveHandle,
        shape: Box<ShapeElement>,
    },
}

#[derive(Clone)]
struct LayerGesture {
    kind: LayerGestureKind,
    start: Point,
    current: Point,
    preview: egui::Rect,
}

pub struct Editor {
    viewport: egui::ViewportId,
    view: Arc<Mutex<View>>,
    tx: Sender<Job>,
    rx: Receiver<Result<Presented, String>>,
    worker: Option<thread::JoinHandle<()>>,
}

fn save_dirty(session: &mut EditorSession) -> Result<(), String> {
    if session.snapshot().active_text_input.is_some() {
        return Err("Finish or cancel text input before saving a draft.".into());
    }
    if session.snapshot().unsaved_changes {
        session.execute(View::autosave_request())?;
    }
    Ok(())
}

fn decode_import(path: &Path) -> Result<RgbaImage, String> {
    captures_app::editor_image_decode::decode_import(path)
}

fn wake(ctx: &egui::Context, viewport: egui::ViewportId) {
    ctx.send_viewport_cmd_to(
        egui::ViewportId::ROOT,
        egui::ViewportCommand::RequestPaintWhileHidden,
    );
    ctx.request_repaint_of(egui::ViewportId::ROOT);
    ctx.request_repaint_of(viewport);
}

impl Editor {
    pub fn open(
        ctx: &egui::Context,
        root: PathBuf,
        artifact_id: String,
        output_directory: PathBuf,
        mode: CaptureMode,
        copy: impl Fn(Arc<RgbaImage>) -> Result<(), String> + Send + 'static,
    ) -> Self {
        let viewport = egui::ViewportId::from_hash_of(("screenshot-editor", &artifact_id));
        let default_stem = format!(
            "Captures_{}_edited",
            chrono::Local::now().format("%Y-%m-%d_%H-%M-%S")
        );
        let original_bytes = export::original_size_bytes(&root, &artifact_id);
        let view_artifact_id = artifact_id.clone();
        let (tx, jobs) = mpsc::channel();
        let (out, rx) = mpsc::channel();
        let wake_ctx = ctx.clone();
        let worker = thread::spawn(move || {
            let opened = EditorSession::open_with_fonts(
                OpenRequest {
                    drafts_root: root.with_file_name("editor-drafts"),
                    history_root: root.clone(),
                    artifact_id,
                },
                Some(captures_app::editor_fonts::bundled()),
            );
            let mut session = match opened {
                Ok(mut session) => {
                    let _ = out.send(Ok(Presented::after_edit(&mut session)));
                    Some(session)
                }
                Err(error) => {
                    let _ = out.send(Err(error));
                    None
                }
            };
            wake(&wake_ctx, viewport);
            while let Ok(job) = jobs.recv() {
                let result = match job {
                    Job::Shutdown => break,
                    Job::DrawingPreview {
                        request,
                        epoch,
                        reply,
                    } => {
                        let pixels = session
                            .as_mut()
                            .ok_or_else(|| "Editor is unavailable.".to_owned())
                            .and_then(|session| session.preview_drawing(request));
                        let _ = reply.send(drawing_preview::Reply { epoch, pixels });
                        wake(&wake_ctx, viewport);
                        continue;
                    }
                    Job::Apply(request) => session
                        .as_mut()
                        .ok_or_else(|| "Editor is unavailable.".to_owned())
                        .and_then(|session| {
                            let creates_layer = matches!(
                                request,
                                Request::CreateClosedShape { .. }
                                    | Request::CreateOpenShape { .. }
                                    | Request::CreateFreehandPath { .. }
                                    | Request::CreateText { .. }
                            );
                            let copied_layer = matches!(request, Request::CopyLayer { .. });
                            let pasted_layer = matches!(request, Request::PasteLayer { .. });
                            session.execute(request)?;
                            let mut presented = Presented::after_edit(session);
                            presented.copied_layer = copied_layer;
                            presented.pasted_layer = pasted_layer;
                            if creates_layer {
                                presented.created_layer = presented
                                    .document
                                    .elements
                                    .last()
                                    .map(|element| element.base().id.clone());
                            }
                            Ok(presented)
                        }),
                    Job::Import {
                        path,
                        selected_id,
                        point,
                    } => session
                        .as_mut()
                        .ok_or_else(|| "Editor is unavailable.".to_owned())
                        .and_then(|session| {
                            let id = session.import_image(ImportImage {
                                pixels: decode_import(&path)?,
                                name: path
                                    .file_name()
                                    .unwrap_or_default()
                                    .to_string_lossy()
                                    .into_owned(),
                                selected_id,
                                point,
                            })?;
                            let mut presented = Presented::after_edit(session);
                            presented.created_layer = Some(id);
                            Ok(presented)
                        }),
                    Job::Copy => session
                        .as_ref()
                        .ok_or_else(|| "Editor is unavailable.".to_owned())
                        .and_then(|session| {
                            if session.snapshot().active_text_input.is_some() {
                                return Err(
                                    "Finish or cancel text input before copying pixels.".into()
                                );
                            }
                            copy(session.pixels())?;
                            let mut presented = Presented::from_session(session);
                            presented.copied = true;
                            Ok(presented)
                        }),
                    Job::Save { plan, options } => session
                        .as_ref()
                        .ok_or_else(|| "Editor is unavailable.".to_owned())
                        .and_then(|session| {
                            if session.snapshot().active_text_input.is_some() {
                                return Err(
                                    "Finish or cancel text input before saving pixels.".into()
                                );
                            }
                            let saved =
                                export::publish(&root, &session.pixels(), &plan, options, mode)?;
                            let mut presented = Presented::from_session(session);
                            presented.saved = Some((plan, saved));
                            Ok(presented)
                        }),
                    Job::Autosave { reply } => {
                        let result = session
                            .as_mut()
                            .ok_or_else(|| "Editor is unavailable.".to_owned())
                            .and_then(|session| {
                                session.execute(View::autosave_request())?;
                                Ok(session.snapshot().has_draft)
                            });
                        let _ = reply.send(result);
                        wake(&wake_ctx, viewport);
                        continue;
                    }
                    Job::Flush { input, reply } => {
                        let result = session.as_mut().map_or(Ok(()), |session| {
                            if let Some(input) = input {
                                if session.snapshot().active_text_input.is_some() {
                                    if input.commit {
                                        session.execute(Request::UpdateTextInput {
                                            input_id: input.input_id.clone(), text: input.text,
                                        })?;
                                    }
                                    session.execute(Request::FinishTextInput {
                                        input_id: input.input_id, commit: input.commit,
                                    })?;
                                } else if input.commit && !input.finishing {
                                    return Err("Text input could not be finished. Retry or cancel before quitting.".into());
                                }
                            }
                            save_dirty(session)
                        });
                        let _ = reply.send(result.clone());
                        match (result, session.as_mut()) {
                            (Ok(()), Some(session)) => Ok(Presented::after_edit(session)),
                            (Err(error), _) => Err(error),
                            _ => continue,
                        }
                    }
                };
                if out.send(result).is_err() {
                    break;
                }
                wake(&wake_ctx, viewport);
            }
        });
        Self {
            viewport,
            view: Arc::new(Mutex::new(View {
                artifact_id: view_artifact_id,
                default_directory: output_directory,
                default_stem,
                original_bytes,
                drawing_preview: Some(drawing_preview::State::new(ctx.clone(), viewport)),
                autosaves: true,
                // Unit tests open real editors; they never hand files to a
                // file manager unless a test installs its own recorder.
                reveal_file: (!cfg!(test))
                    .then(|| Arc::new(|path: &Path| crate::reveal::reveal(path)) as RevealFile),
                ..View::default()
            })),
            tx,
            rx,
            worker: Some(worker),
        }
    }

    pub fn focus(&self, ctx: &egui::Context) {
        ctx.send_viewport_cmd_to(self.viewport, egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd_to(self.viewport, egui::ViewportCommand::Focus);
        wake(ctx, self.viewport);
    }

    pub fn closed(&self) -> bool {
        self.view.lock().unwrap().closed
    }

    pub fn take_history_changed(&self) -> bool {
        std::mem::take(&mut self.view.lock().unwrap().history_changed)
    }

    pub fn take_original_replaced(&self) -> bool {
        std::mem::take(&mut self.view.lock().unwrap().original_replaced)
    }

    pub fn receive(&self, ctx: &egui::Context) {
        if self.view.lock().unwrap().receive_folder() {
            ctx.request_repaint_of(self.viewport);
        }
        if self.view.lock().unwrap().receive_autosave() {
            ctx.request_repaint_of(self.viewport);
        }
        while let Ok(result) = self.rx.try_recv() {
            self.view.lock().unwrap().receive(ctx, result);
            ctx.request_repaint_of(self.viewport);
        }
        if let Some(preview) = &mut self.view.lock().unwrap().drawing_preview {
            preview.receive(&self.tx);
        }
        self.view.lock().unwrap().drain_inline(&self.tx);
        if self.view.lock().unwrap().receive_import(&self.tx) {
            ctx.request_repaint_of(self.viewport);
        }
        if canvas::drain_drops(&mut self.view.lock().unwrap(), &self.tx) {
            ctx.request_repaint_of(self.viewport);
        }
    }

    /// Application quit drains accepted edits and saves their final draft on the
    /// worker. Failure cancels normal quit; the live session/window stays usable.
    pub fn flush(&self, ctx: &egui::Context) -> Result<(), String> {
        if self.closed() {
            return Ok(());
        }
        // A ready picker result is not an accepted edit yet. Do not let receive
        // enqueue a new import after the worker has finished its final save.
        self.view.lock().unwrap().import_picker = None;
        let (tx, rx) = mpsc::channel();
        let input = self.view.lock().unwrap().inline_for_flush();
        self.tx
            .send(Job::Flush { input, reply: tx })
            .map_err(|_| "Editor worker stopped.".to_owned())?;
        let result = rx.recv().map_err(|_| "Editor worker stopped.".to_owned())?;
        self.receive(ctx);
        if let Err(error) = &result {
            self.view.lock().unwrap().error =
                Some(format!("Could not save edits before quitting: {error}"));
            self.focus(ctx);
        }
        result
    }

    pub fn show(&self, ctx: &egui::Context, tokens: &Tokens) {
        let _span = crate::diagnostics::span("editor-register");
        if self.closed() {
            return;
        }
        let state = self.view.clone();
        let tx = self.tx.clone();
        let tokens = tokens.clone();
        let viewport = self.viewport;
        let title = self.view.lock().unwrap().title();
        ctx.show_viewport_deferred(
            viewport,
            egui::ViewportBuilder::default()
                .with_title(title)
                .with_inner_size([1000., 700.])
                .with_min_inner_size([760., 540.]),
            move |ui, _| {
                let _span = crate::diagnostics::span("editor-callback");
                let mut view = state.lock().unwrap();
                crate::diagnostics::event("editor-locked", || {
                    serde_json::json!({
                        "actualViewport":format!("{:?}", ui.ctx().viewport_id()),
                        "rootPass":ui.ctx().cumulative_pass_nr_for(egui::ViewportId::ROOT),
                    })
                });
                if ui.input(|input| input.viewport().close_requested()) {
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::CancelClose);
                    view.request_close();
                }
                view.flush_live(&tx);
                view.drive_close(&tx);
                if view.closed {
                    wake(ui.ctx(), viewport);
                    return;
                }
                ui.push_id(viewport, |ui| show(ui, &tokens, &mut view, &tx));
                if ui.input(|input| input.viewport().title.as_deref() != Some(view.title())) {
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::Title(view.title().into()));
                }
                if view.closed {
                    wake(ui.ctx(), viewport);
                }
            },
        );
    }
}

impl Drop for Editor {
    fn drop(&mut self) {
        // Never panic here: a panic while holding the view lock (for example a
        // failed assertion) poisons it, and a second panic during unwinding
        // aborts the process and hides the original failure.
        self.view
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .closed = true;
        let _ = self.tx.send(Job::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn show(ui: &mut egui::Ui, tokens: &Tokens, view: &mut View, tx: &Sender<Job>) {
    view.flush_background(tx);
    view.flush_live(tx);
    view.drive_autosave(ui.ctx(), tx);
    let previous_section = view.section;
    handle_viewport_shortcuts(ui.ctx(), view);
    handle_document_shortcuts(ui.ctx(), view, tx);
    if view.inline.is_none()
        && !ui.ctx().egui_wants_keyboard_input()
        && !egui::Popup::is_any_open(ui.ctx())
        && ui.input(|input| input.key_pressed(egui::Key::Escape))
    {
        view.cancel_crop();
        view.cancel_drawing();
        view.cancel_layer_gesture();
        view.layers.cancel();
    }
    egui::Panel::top("editor-actions")
        .resizable(false)
        .show_separator_line(false)
        .frame(egui::Frame::NONE.fill(tokens.color("surface-raised")))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
            chrome::show_banners(ui, tokens, view, tx);
            chrome::show_header(ui, tokens, view, tx);
        });
    if view.section != previous_section {
        view.viewport_pan = None;
    }
    if view.section != Section::Geometry {
        view.cancel_crop();
    }
    if view.section != Section::Draw || view.close_requested || !ui.input(|input| input.focused) {
        view.cancel_drawing();
    }
    if view.section != Section::Layers || view.close_requested {
        view.layers.menu = None;
        view.layers.rename = None;
    }
    if !view.tool_shows_transform_chrome()
        || view.pending
        || view.close_requested
        || !ui.input(|input| input.focused)
    {
        view.cancel_layer_gesture();
    }
    view.drive_reveal();
    view.drive_estimate(ui.ctx());
    view.drive_compare(ui.ctx());
    show_export_bar(ui, tokens, view, tx);
    egui::Panel::right("editor-geometry")
        .resizable(false)
        .exact_size(320.)
        .show(ui, |ui| {
            // Shipping `.screenshot-sidebar`: Layers above Properties, always.
            // Live fields stay enabled while a job runs; their edits queue.
            let enabled = view.inline.is_none() && view.presented.is_some();
            let layers_height = layers::section_height(ui.available_height());
            let top = ui.cursor().top();
            ui.add_enabled_ui(enabled, |ui| {
                layers::show(ui, tokens, view, tx, layers_height)
            });
            let used = ui.cursor().top() - top;
            ui.add_space((layers_height - used).max(0.));
            ui.painter().hline(
                ui.max_rect().expand2(egui::vec2(8., 0.)).x_range(),
                ui.cursor().top() - 0.5,
                egui::Stroke::new(1., tokens.color("border-subtle")),
            );
            // Shipping's sticky Properties title stays outside the fields'
            // scroll viewport, so it cannot cover a control scrolled into view.
            ui.spacing_mut().item_spacing.y = 0.;
            ui.add_enabled_ui(enabled, |ui| chrome::properties_heading(ui, tokens, view));
            crate::primitives::scroll_area(
                ui,
                tokens,
                egui::ScrollArea::vertical()
                    .id_salt(view.properties_section())
                    .auto_shrink([false, false]),
                |ui| {
                    ui.add_enabled_ui(enabled, |ui| {
                        // Sections space their own items (`inspector::section`).
                        ui.spacing_mut().item_spacing.y = 0.;
                        match view.properties_section() {
                            Section::Layers => show_layer_properties(ui, tokens, view, tx),
                            Section::Draw => show_draw_properties(ui, tokens, view),
                            Section::Geometry => show_crop_properties(ui, tokens, view, tx),
                        }
                    });
                },
            );
        });
    chrome::show_tool_rail(ui, tokens, view);
    egui::CentralPanel::default().show(ui, |ui| {
        let texture = view
            .drawing_preview
            .as_ref()
            .and_then(|preview| preview.texture.as_ref())
            .or(view.texture.as_ref());
        if let Some(texture) = texture {
            let texture = texture.clone();
            let available = ui.available_rect_before_wrap();
            if view.viewport_area != Some(available) {
                view.cancel_edit_gestures();
                view.viewport_pan = None;
                view.viewport_area = Some(available);
            }
            let size = texture.size_vec2();
            view.viewport_image_size = Some(size);
            let fit = fitted_image_rect(available, size);
            let comparison_id = egui::Id::unique("screenshot-compression-comparison");
            let intercepted = (view.compare_visible()
                && !view.compare_suppressed()
                && crate::compare_overlay::owns_pointer(ui.ctx(), comparison_id))
                || handle_viewport_input(ui, view, available);
            let preview = viewport_rect(view.viewport, fit, size).unwrap_or(fit);
            canvas::receive_drops(ui.ctx(), view, Some(preview));
            ui.allocate_rect(available, egui::Sense::hover());
            ui.painter()
                .with_clip_rect(available.intersect(ui.clip_rect()))
                .image(
                    texture.id(),
                    preview,
                    egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.)),
                    egui::Color32::WHITE,
                );
            let clip = available.intersect(ui.clip_rect());
            let comparing = view.compare_visible() && !view.compare_suppressed();
            if comparing {
                crate::compare_overlay::paint_after(
                    ui,
                    preview,
                    clip,
                    view.output.as_ref().map(|(texture, _)| texture),
                    view.compare_split,
                );
            }
            if view.crop_previous.is_some() && !view.pending && view.inline.is_none() {
                show_crop(ui, tokens, view, available, preview, intercepted);
            }
            let layer_owned = view.tool_shows_transform_chrome()
                && view.shape_drag.is_none()
                && view.inline.is_none()
                && !view.pending
                && !view.close_requested
                && show_layer_canvas(ui, tokens, view, tx, available, preview, intercepted);
            if view.section == Section::Draw
                && view.inline.is_none()
                && !view.pending
                && !view.close_requested
            {
                show_shape(
                    ui,
                    tokens,
                    view,
                    tx,
                    available,
                    preview,
                    intercepted || layer_owned,
                );
            } else {
                view.wand_loupe = None;
            }
            if view.section == Section::Layers
                && view.crop_previous.is_none()
                && !view.close_requested
                && !view.drop.hovering
            {
                canvas::show_expand(ui, tokens, view, tx, available, preview);
            }
            canvas::paint_trim_preview(ui, tokens, view, available, preview);
            canvas::paint_drop_guide(ui, tokens, view, available, preview);
            text_input::show(ui, tokens, view, available, preview);
            if comparing && view.compare_visible() {
                let badges = view.compare_badges();
                let drawing = view.section == Section::Draw;
                let error = view.compare_error.clone();
                let mut split = view.compare_split;
                let shown = crate::compare_overlay::show_chrome(
                    ui,
                    tokens,
                    crate::compare_overlay::Overlay {
                        id: comparison_id,
                        frame: preview,
                        clip,
                        after: view.output.as_ref().map(|(texture, _)| texture),
                        badges,
                        processing: view.compare_pending,
                        error: error.as_deref(),
                        range_enabled: !drawing && view.crop_previous.is_none(),
                        after_hint: drawing.then_some(compare::AFTER_HINT),
                    },
                    &mut split,
                );
                view.compare_split = split;
                if shown.dismissed {
                    view.compare_dismissed = true;
                }
            }
            chrome::recenter(ui, tokens, view, available, preview);
        } else if view.pending {
            canvas::receive_drops(ui.ctx(), view, None);
            ui.centered_and_justified(|ui| {
                ui.spinner();
            });
        } else {
            ui.centered_and_justified(|ui| {
                ui.label("Could not open this screenshot. See the error below.");
            });
        }
    });
    view.drain_inline(tx);
    if canvas::drain_drops(view, tx) {
        ui.ctx().request_repaint();
    }
}

/// Shipping Properties for drawing tools. The rail picks the tool; the
/// Eraser rail tool picks Wand, Erase or Restore here (`Eraser mode`).
fn show_draw_properties(ui: &mut egui::Ui, tokens: &Tokens, view: &mut View) {
    inspector::section(ui, tokens, |ui| draw_section(ui, tokens, view));
}

fn draw_section(ui: &mut egui::Ui, tokens: &Tokens, view: &mut View) {
    let previous_tool = view.draw_shape;
    if matches!(
        view.draw_shape,
        DrawShape::Wand | DrawShape::Erase | DrawShape::Restore
    ) {
        inspector::hint(ui, tokens, captures_app::editor_chrome::eraser::INTRO);
        // Shipping `.screenshot-format-buttons-3`: three equal toggle buttons.
        let gap = tokens.number("s-2");
        let (row, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), tokens.number("h-md")),
            egui::Sense::hover(),
        );
        let width = (row.width() - 2. * gap) / 3.;
        for (index, (shape, label)) in [
            (DrawShape::Wand, "Wand"),
            (DrawShape::Erase, "Erase"),
            (DrawShape::Restore, "Restore"),
        ]
        .into_iter()
        .enumerate()
        {
            let rect = egui::Rect::from_min_size(
                egui::pos2(row.left() + index as f32 * (width + gap), row.top()),
                egui::vec2(width, row.height()),
            );
            let response = ui.interact(
                rect,
                ui.scope_id().with(("eraser-mode", label)),
                egui::Sense::click(),
            );
            let active = view.draw_shape == shape;
            // `.screenshot-format-buttons button`, accent-filled when active.
            let (fill, border, ink) = if active {
                (
                    tokens.color("theme-accent"),
                    tokens.color("theme-accent"),
                    tokens.color("theme-accent-ink"),
                )
            } else {
                (
                    tokens.color(if response.hovered() {
                        "control-hover"
                    } else {
                        "control"
                    }),
                    tokens.color("border-subtle"),
                    tokens.color("text-muted"),
                )
            };
            ui.painter().rect(
                rect,
                tokens.number("r-sm"),
                fill,
                egui::Stroke::new(1., border),
                egui::StrokeKind::Inside,
            );
            if response.has_focus() {
                crate::primitives::focus_ring(ui, tokens, rect, tokens.number("r-sm"));
            }
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                label,
                egui::FontId::new(
                    tokens.number("text-sm"),
                    egui::FontFamily::Name(crate::ui_fonts::SEMIBOLD.into()),
                ),
                ink,
            );
            response.widget_info(|| {
                egui::WidgetInfo::selected(egui::WidgetType::Button, true, active, label)
            });
            if response.clicked() {
                view.draw_shape = shape;
            }
        }
    }
    if view.draw_shape != previous_tool {
        view.cancel_drawing();
    }
    if matches!(
        view.draw_shape,
        DrawShape::Wand | DrawShape::Erase | DrawShape::Restore
    ) {
        view.last_background_tool = view.draw_shape;
    }
    if view.draw_shape.is_grouped() {
        view.last_grouped_shape = view.draw_shape;
    }
    if view.draw_shape == DrawShape::Wand {
        use captures_app::editor_chrome::eraser as e;
        let marks =
            e::TOLERANCE_MARKS.map(|(value, label)| crate::primitives::RangeMark { value, label });
        // Slider values are the wand's 0–255 channel distance; shipping stops at 120.
        view.wand_tolerance = view
            .wand_tolerance
            .clamp(e::TOLERANCE_RANGE.0, e::TOLERANCE_RANGE.1);
        let text = e::tolerance_text(view.wand_tolerance);
        labelled_slider(
            ui,
            tokens,
            e::TOLERANCE,
            crate::primitives::RangeSlider::new(
                "wand-tolerance",
                e::TOLERANCE_LABEL,
                ui.available_width(),
                e::TOLERANCE_RANGE.0..=e::TOLERANCE_RANGE.1,
                text,
            )
            .marks(&marks),
            &mut view.wand_tolerance,
        );
        inspector::check_row(ui, tokens, &mut view.wand_contiguous, e::CONTIGUOUS);
        inspector::hint(ui, tokens, e::wand_hint(view.wand_contiguous));
    } else if matches!(view.draw_shape, DrawShape::Erase | DrawShape::Restore) {
        use captures_app::editor_chrome::eraser as e;
        chrome::draw_tool_preview(
            ui,
            tokens,
            &captures_app::editor_chrome::draw_preview::brush(view.brush_size, view.brush_softness),
            egui::Color32::WHITE,
            None,
            1.,
        );
        let size_marks =
            e::SIZE_MARKS.map(|(value, label)| crate::primitives::RangeMark { value, label });
        let text = e::size_text(view.brush_size);
        labelled_slider(
            ui,
            tokens,
            e::SIZE,
            crate::primitives::RangeSlider::new(
                "brush-size",
                e::SIZE_LABEL,
                ui.available_width(),
                e::SIZE_RANGE.0..=e::SIZE_RANGE.1,
                text,
            )
            .marks(&size_marks),
            &mut view.brush_size,
        );
        let softness_marks =
            e::SOFTNESS_MARKS.map(|(value, label)| crate::primitives::RangeMark { value, label });
        let text = e::softness_text(view.brush_softness);
        labelled_slider(
            ui,
            tokens,
            e::SOFTNESS,
            crate::primitives::RangeSlider::new(
                "brush-softness",
                e::SOFTNESS_LABEL,
                ui.available_width(),
                e::SOFTNESS_RANGE.0..=e::SOFTNESS_RANGE.1,
                text,
            )
            .marks(&softness_marks),
            &mut view.brush_softness,
        );
        inspector::hint(
            ui,
            tokens,
            if view.draw_shape == DrawShape::Erase {
                e::ERASE_HINT
            } else {
                e::RESTORE_HINT
            },
        );
    } else if view.draw_shape == DrawShape::Text {
        // Shipping: New text style, New text size, then the drawing defaults'
        // Drop shadow. New text takes the drawing Color; there is no Color row.
        if let Some(presented) = &view.presented {
            inspector::labelled(ui, tokens, "New text style", |ui| {
                pickers::text_style_picker(
                    ui,
                    tokens,
                    "New text style",
                    &presented.text_style_presets,
                    &mut view.new_text_preset,
                    true,
                );
            });
        }
        inspector::labelled(ui, tokens, "New text size", |ui| {
            crate::primitives::NumberInput::new(
                "new-text-size",
                "New text size",
                ui.available_width(),
            )
            .range(8. ..=512.)
            .show(ui, tokens, &mut view.new_text_size);
        });
        // Shipping shares the drawing defaults' shadow with new text, showing
        // defaults scaled from the new text size until customized.
        let reference = captures_app::editor_text::new_text_shadow_style(
            &view.new_annotation_style,
            view.new_text_size,
        );
        let style = &mut view.new_annotation_style;
        let mut enabled = style.has_drop_shadow();
        let mut shadow = reference.resolved_drop_shadow_style();
        let before = shadow.clone();
        if drop_shadow_fields(ui, tokens, "new-text", &mut enabled, &mut shadow) {
            style.drop_shadow = Some(enabled);
        }
        if shadow != before {
            style.drop_shadow_style = Some(shadow);
        }
    } else {
        let closed = view.draw_shape.closed_kind().is_some();
        if view.draw_shape.is_grouped() {
            chrome::shape_picker(ui, tokens, view);
        }
        let style = &view.new_annotation_style;
        chrome::draw_tool_preview(
            ui,
            tokens,
            &captures_app::editor_chrome::draw_preview::stroke(
                view.draw_shape.preview_key(),
                style.stroke_width,
                !closed || style.has_stroke(),
            ),
            egui::Color32::from_hex(&style.color).unwrap_or(egui::Color32::BLACK),
            closed
                .then(|| {
                    style
                        .fill
                        .as_deref()
                        .and_then(|fill| egui::Color32::from_hex(fill).ok())
                })
                .flatten(),
            (view.new_annotation_opacity / 100.).clamp(0., 1.) as f32,
        );
        let style = &mut view.new_annotation_style;
        if closed {
            let mut stroke = style.has_stroke();
            if inspector::check_row(ui, tokens, &mut stroke, "Stroke").changed() {
                style.stroke_enabled = Some(stroke);
            }
        }
        if !closed || style.has_stroke() {
            swatch_color(
                ui,
                tokens,
                if closed {
                    colors::STROKE_COLOR
                } else {
                    colors::COLOR
                },
                &mut style.color,
            );
            let text = format!("{} px", style.stroke_width.round());
            labelled_slider(
                ui,
                tokens,
                "Size",
                crate::primitives::RangeSlider::new(
                    "new-stroke-width",
                    "Stroke width",
                    ui.available_width(),
                    2. ..=40.,
                    text,
                ),
                &mut style.stroke_width,
            );
        }
        let text = format!("{}%", view.new_annotation_opacity.round());
        labelled_slider(
            ui,
            tokens,
            "Opacity",
            crate::primitives::RangeSlider::new(
                "new-opacity",
                "Opacity",
                ui.available_width(),
                0. ..=100.,
                text,
            ),
            &mut view.new_annotation_opacity,
        );
        let style = &mut view.new_annotation_style;
        if closed {
            let mut filled = style.fill.is_some();
            if inspector::check_row(ui, tokens, &mut filled, "Filled shape").changed() {
                style.fill = filled.then(|| style.color.clone());
            }
            if let Some(fill) = &mut style.fill {
                swatch_color(ui, tokens, colors::FILL_COLOR, fill);
            }
        }
        let mut enabled = style.has_drop_shadow();
        let mut shadow = style
            .drop_shadow_style
            .clone()
            .unwrap_or_else(|| style.resolved_drop_shadow_style());
        let before = shadow.clone();
        if drop_shadow_fields(ui, tokens, "new-drawing", &mut enabled, &mut shadow) {
            style.drop_shadow = Some(enabled);
        }
        if shadow != before {
            style.drop_shadow_style = Some(shadow);
        }
    }
}

/// Shipping Crop properties: the Aspect ratio select, then the staged
/// selection's size with Clear and Apply crop, or the drag hint.
fn show_crop_properties(ui: &mut egui::Ui, tokens: &Tokens, view: &mut View, tx: &Sender<Job>) {
    // Like shipping's Crop tool, a new selection can be dragged again.
    if view.crop_tool && view.crop_previous.is_none() && !view.pending {
        view.crop_previous = Some(view.crop);
    }
    inspector::section(ui, tokens, |ui| {
        inspector::labelled(ui, tokens, "Aspect ratio", |ui| {
            let options: Vec<_> = CROP_ASPECTS
                .iter()
                .enumerate()
                .map(|(index, (label, _))| crate::primitives::SelectOption::new(index, label))
                .collect();
            if let Some(index) =
                crate::primitives::Select::new("crop-aspect", "Aspect ratio", ui.available_width())
                    .show(ui, tokens, &options, &view.crop_aspect)
                    .chosen
            {
                view.crop_aspect = index;
            }
        });
        let staged = view
            .crop_previous
            .is_some_and(|previous| previous != view.crop);
        if staged {
            inspector::pair(ui, tokens, |ui, column, width| {
                let (label, name, value) = if column == 0 {
                    ("Width", "Crop width", view.crop[2])
                } else {
                    ("Height", "Crop height", view.crop[3])
                };
                inspector::labelled(ui, tokens, label, |ui| {
                    let mut value = value.round();
                    ui.add_enabled_ui(false, |ui| {
                        crate::primitives::NumberInput::new(("crop-size", column), name, width)
                            .show(ui, tokens, &mut value);
                    });
                });
            });
            let mut clear = false;
            let mut apply = false;
            inspector::columns(ui, tokens.number("s-3"), |ui, column, width| {
                if column == 0 {
                    clear = inspector::action_button(ui, tokens, "Clear", width, !view.pending)
                        .clicked();
                } else {
                    apply = chrome::primary_action(
                        ui,
                        tokens,
                        "Apply crop",
                        width,
                        !view.pending,
                        true,
                    )
                    .clicked();
                }
            });
            if clear {
                view.cancel_crop();
            }
            if apply {
                let [x, y, width, height] = view.crop;
                view.crop_previous = None;
                view.crop_drag = None;
                view.submit(
                    tx,
                    Request::Crop {
                        rect: Rect {
                            x,
                            y,
                            width,
                            height,
                        },
                    },
                );
            }
            inspector::hint(
                ui,
                tokens,
                "Hold Shift while dragging to keep this aspect ratio.",
            );
        } else {
            inspector::hint(
                ui,
                tokens,
                "Drag over the area you want to keep. Start from outside the canvas to crop to an edge. Hold Shift to lock the current aspect ratio.",
            );
        }
    });
}

fn fitted_image_rect(available: egui::Rect, image: egui::Vec2) -> egui::Rect {
    // Match Tauri's 2–100% Fit range; manual zoom has its own 5–800% range.
    let scale = (available.width() / image.x)
        .max(0.02)
        .min((available.height() / image.y).max(0.02))
        .min(1.);
    // Keep spare space balanced, matching Tauri and AppKit even when the
    // minimum Fit scale makes a very large image extend beyond the viewport.
    egui::Rect::from_center_size(available.center(), image * scale)
}

fn viewport_rect(viewport: Viewport, fit: egui::Rect, image: egui::Vec2) -> Option<egui::Rect> {
    viewport
        .rect(
            Rect {
                x: fit.left().into(),
                y: fit.top().into(),
                width: fit.width().into(),
                height: fit.height().into(),
            },
            image.x.into(),
            image.y.into(),
        )
        .map(|rect| {
            egui::Rect::from_min_size(
                egui::pos2(rect.x as f32, rect.y as f32),
                egui::vec2(rect.width as f32, rect.height as f32),
            )
        })
}

fn handle_viewport_shortcuts(ctx: &egui::Context, view: &mut View) {
    // Consume editor shortcuts before egui's end-of-pass global UI zoom.
    // Keep event order and repeats; several key presses may arrive in one pass.
    let keys = ctx.input_mut(|input| {
        let mut keys = Vec::new();
        input.events.retain(|event| {
            let egui::Event::Key {
                key,
                physical_key,
                pressed: true,
                modifiers,
                ..
            } = event
            else {
                return true;
            };
            let zoom_key = |key: &egui::Key| {
                matches!(
                    key,
                    egui::Key::Plus | egui::Key::Equals | egui::Key::Minus | egui::Key::Num0
                )
            };
            let key = if zoom_key(key) {
                Some(*key)
            } else {
                physical_key.filter(|key| matches!(key, egui::Key::Equals | egui::Key::Minus))
            };
            if let Some(key) = key.filter(|_| modifiers.command || modifiers.ctrl) {
                keys.push(key);
                false
            } else {
                true
            }
        });
        keys
    });
    if ctx.current_pass_index() != 0
        || !ctx.input(|input| input.focused)
        || view.close_requested
        || egui::Popup::is_any_open(ctx)
    {
        return;
    }
    // Shipping zoom shortcuts also work while a numeric/text field has focus.
    for key in keys {
        match key {
            egui::Key::Num0 => set_viewport_zoom(view, 100., None),
            egui::Key::Minus => change_viewport_zoom(view, 1. / 1.25, None),
            _ => change_viewport_zoom(view, 1.25, None),
        }
    }
}

fn handle_document_shortcuts(ctx: &egui::Context, view: &mut View, tx: &Sender<Job>) {
    if ctx.current_pass_index() != 0
        || !ctx.input(|input| input.focused)
        // Widgets have not processed this frame's click/focus change yet.
        || ctx.input(|input| input.pointer.any_pressed())
        || ctx.text_edit_focused()
        || egui::Popup::is_any_open(ctx)
        || view.inline.is_some()
        || view.closed
        || view.close_requested
    {
        return;
    }
    // Sliders and closed selectors also own arrows, not just text editors.
    let canvas_navigation = ctx.memory(|memory| memory.focused().is_none());
    let tools_enabled = canvas_navigation
        && view.presented.is_some()
        && !view.pending
        && view.import_picker.is_none()
        && view.folder_picker.is_none();
    // Keep tools and document actions in input order, including when a fast
    // tool-change + Delete arrives in one frame. Neither kind may jump ahead.
    enum Shortcut {
        Tool(Section, Option<DrawShape>),
        Document(egui::Key, bool),
    }
    let requests = ctx.input_mut(|input| {
        let mut requests = Vec::new();
        input.events.retain(|event| {
            // Winit supplies semantic clipboard events on native platforms.
            // Their text payload belongs to the OS clipboard, not our layer copy.
            match event {
                egui::Event::Copy => {
                    requests.push(Shortcut::Document(egui::Key::C, false));
                    return false;
                }
                egui::Event::Paste(_) => {
                    requests.push(Shortcut::Document(egui::Key::V, false));
                    return false;
                }
                _ => {}
            }
            if let egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } = event
                && tools_enabled
                && !modifiers.command
                && !modifiers.ctrl
                && !modifiers.mac_cmd
                && !modifiers.alt
            {
                let tool = match key {
                    egui::Key::V => Some((Section::Layers, None)),
                    egui::Key::C => Some((Section::Geometry, None)),
                    egui::Key::T => Some((Section::Draw, Some(DrawShape::Text))),
                    egui::Key::R => Some((Section::Draw, Some(DrawShape::Rectangle))),
                    egui::Key::O => Some((Section::Draw, Some(DrawShape::Ellipse))),
                    egui::Key::L => Some((Section::Draw, Some(DrawShape::Line))),
                    egui::Key::D => Some((Section::Draw, Some(DrawShape::Diamond))),
                    egui::Key::S => Some((Section::Draw, Some(DrawShape::Star))),
                    egui::Key::A => Some((Section::Draw, Some(DrawShape::Arrow))),
                    egui::Key::P => Some((Section::Draw, Some(DrawShape::Freehand))),
                    egui::Key::B => Some((Section::Draw, Some(view.last_background_tool))),
                    _ => None,
                };
                if let Some((section, shape)) = tool {
                    requests.push(Shortcut::Tool(section, shape));
                    return false;
                }
            }
            if let egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } = event
                && (matches!(key, egui::Key::Delete | egui::Key::Backspace)
                    || (canvas_navigation
                        && matches!(
                            key,
                            egui::Key::ArrowLeft
                                | egui::Key::ArrowRight
                                | egui::Key::ArrowUp
                                | egui::Key::ArrowDown
                        ))
                    || (matches!(
                        key,
                        egui::Key::Z | egui::Key::D | egui::Key::C | egui::Key::V
                    ) && (modifiers.command || modifiers.ctrl)))
            {
                requests.push(Shortcut::Document(*key, modifiers.shift));
                false
            } else {
                true
            }
        });
        requests
    });
    for shortcut in requests {
        if view.pending {
            continue;
        }
        let (key, shift) = match shortcut {
            Shortcut::Tool(section, shape) => {
                view.activate_tool(section, shape);
                continue;
            }
            Shortcut::Document(key, shift) => (key, shift),
        };
        let Some(presented) = &view.presented else {
            continue;
        };
        let layer = presented
            .document
            .elements
            .iter()
            .find(|element| Some(&element.base().id) == view.selected_layer.as_ref());
        let layer_action = match key {
            egui::Key::C => Some(LayerAction::Copy),
            egui::Key::V => Some(LayerAction::Paste),
            egui::Key::D => Some(LayerAction::Duplicate),
            egui::Key::Delete | egui::Key::Backspace => Some(LayerAction::Delete),
            _ => None,
        };
        if let Some(action) = layer_action {
            dispatch_layer_action(view, tx, action, view.selected_layer.clone());
            continue;
        }
        let request = match key {
            egui::Key::Z if shift && presented.can_redo => Some(Request::Redo),
            egui::Key::Z if !shift && presented.can_undo => Some(Request::Undo),
            egui::Key::ArrowLeft
            | egui::Key::ArrowRight
            | egui::Key::ArrowUp
            | egui::Key::ArrowDown => {
                layer
                    .filter(|element| !element.base().locked)
                    .map(|element| {
                        let distance = if shift { 10. } else { 1. };
                        let (delta_x, delta_y) = match key {
                            egui::Key::ArrowLeft => (-distance, 0.),
                            egui::Key::ArrowRight => (distance, 0.),
                            egui::Key::ArrowUp => (0., -distance),
                            _ => (0., distance),
                        };
                        Request::Layer {
                            id: element.base().id.clone(),
                            edit: LayerEdit::Translate { delta_x, delta_y },
                        }
                    })
            }
            _ => None,
        };
        if let Some(request) = request {
            view.cancel_edit_gestures();
            view.viewport_pan = None;
            view.submit(tx, request);
        }
    }
}

fn set_viewport_zoom(view: &mut View, percent: f64, anchor: Option<egui::Pos2>) {
    let (Some(area), Some(size)) = (view.viewport_area, view.viewport_image_size) else {
        return;
    };
    let fit = fitted_image_rect(area, size);
    let anchor = anchor.unwrap_or(area.center());
    if let Some(next) = view.viewport.zoom_at(
        Rect {
            x: fit.left().into(),
            y: fit.top().into(),
            width: fit.width().into(),
            height: fit.height().into(),
        },
        size.x.into(),
        size.y.into(),
        percent,
        Point {
            x: anchor.x.into(),
            y: anchor.y.into(),
        },
    ) {
        view.cancel_edit_gestures();
        view.viewport = next;
        view.viewport_pan = None;
    }
}

fn displayed_zoom(view: &View) -> Option<f64> {
    if view.viewport.zoom_percent == 0. {
        let area = view.viewport_area?;
        let size = view.viewport_image_size?;
        Some(f64::from(fitted_image_rect(area, size).width() / size.x) * 100.)
    } else {
        Some(view.viewport.zoom_percent)
    }
}

fn change_viewport_zoom(view: &mut View, factor: f64, anchor: Option<egui::Pos2>) {
    if let Some(current) = displayed_zoom(view) {
        set_viewport_zoom(view, current * factor, anchor);
    }
}

fn handle_viewport_input(ui: &egui::Ui, view: &mut View, available: egui::Rect) -> bool {
    if ui.ctx().current_pass_index() != 0 {
        return view.viewport_intercepted;
    }
    view.viewport_intercepted = false;
    let focused = ui.input(|input| input.focused);
    if !focused || view.close_requested || egui::Popup::is_any_open(ui.ctx()) {
        view.viewport_pan = None;
        view.cancel_edit_gestures();
        return false;
    }
    let events = ui.input(|input| input.events.clone());
    // Wheel zoom and pans start only over the canvas itself, not over a
    // popover or listbox that covers it.
    let anchor = ui.input(|input| input.pointer.hover_pos()).filter(|point| {
        available.contains(*point) && crate::primitives::pressed_on_layer(ui, *point)
    });
    let has_command_wheel = events.iter().any(|event| {
        matches!(event,
        egui::Event::MouseWheel { modifiers, .. } if (modifiers.command || modifiers.ctrl) && anchor.is_some())
    });
    let mut intercepted = view.viewport_pan.is_some();
    for event in events {
        match event {
            egui::Event::MouseWheel {
                unit,
                delta,
                modifiers,
                ..
            } if (modifiers.command || modifiers.ctrl) && anchor.is_some() => {
                let pixels = match unit {
                    egui::MouseWheelUnit::Point => f64::from(delta.y),
                    egui::MouseWheelUnit::Line => f64::from(delta.y) * 16.,
                    egui::MouseWheelUnit::Page => f64::from(delta.y * available.height()),
                };
                // egui reports content motion, opposite to browser wheel deltaY.
                if let Some(factor) = wheel_zoom_factor(-pixels) {
                    change_viewport_zoom(view, factor, anchor);
                    intercepted = true;
                }
            }
            egui::Event::Zoom(factor) if !has_command_wheel && anchor.is_some() => {
                change_viewport_zoom(view, f64::from(factor), anchor);
                intercepted = true;
            }
            egui::Event::PointerButton {
                pos,
                button,
                pressed: true,
                modifiers,
                ..
            } if available.contains(pos)
                && crate::primitives::pressed_on_layer(ui, pos)
                && (button == egui::PointerButton::Middle
                    || (button == egui::PointerButton::Primary
                        && (modifiers.command || modifiers.ctrl))) =>
            {
                view.cancel_edit_gestures();
                view.viewport_pan = Some((button, pos));
                intercepted = true;
            }
            egui::Event::PointerMoved(pos) => {
                if let Some((_, last)) = &mut view.viewport_pan {
                    view.viewport.pan_x += f64::from(pos.x - last.x);
                    view.viewport.pan_y += f64::from(pos.y - last.y);
                    *last = pos;
                    intercepted = true;
                }
            }
            egui::Event::PointerButton {
                pos,
                button,
                pressed: false,
                ..
            } if view
                .viewport_pan
                .is_some_and(|(active, _)| active == button) =>
            {
                if let Some((_, last)) = view.viewport_pan.take() {
                    view.viewport.pan_x += f64::from(pos.x - last.x);
                    view.viewport.pan_y += f64::from(pos.y - last.y);
                }
                intercepted = true;
            }
            egui::Event::PointerGone
            | egui::Event::Key {
                key: egui::Key::Escape,
                pressed: true,
                ..
            } => {
                view.viewport_pan = None;
            }
            _ => {}
        }
    }
    if view.viewport_pan.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    }
    view.viewport_intercepted = intercepted;
    intercepted
}

fn image_point(position: egui::Pos2, preview: egui::Rect, bounds: Rect) -> Point {
    Point {
        x: f64::from(position.x - preview.left()) / f64::from(preview.width()) * bounds.width,
        y: f64::from(position.y - preview.top()) / f64::from(preview.height()) * bounds.height,
    }
}

fn show_layer_canvas(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    view: &mut View,
    tx: &Sender<Job>,
    available: egui::Rect,
    preview: egui::Rect,
    viewport_intercepted: bool,
) -> bool {
    let Some(presented) = &view.presented else {
        return false;
    };
    // Keep ownership after a same-frame release/no-op/cancel; the draw path
    // must not replay a gesture whose state has already been cleared.
    let mut owned = view.layer_gesture.is_some();
    let document = presented.document.clone();
    let bounds = Rect {
        x: 0.,
        y: 0.,
        width: f64::from(presented.pixels.width()),
        height: f64::from(presented.pixels.height()),
    };
    let display_scale = f64::from(preview.width()) / bounds.width;
    if view
        .layer_gesture
        .as_ref()
        .is_some_and(|gesture| gesture.preview != preview)
    {
        view.cancel_layer_gesture();
    }
    let response = ui
        .interact(
            available,
            ui.scope_id().with("layer-canvas"),
            egui::Sense::click_and_drag(),
        )
        .on_hover_text("Click to select. Double-click text to edit. Drag the outline and release to move. Escape cancels.");
    let first_pass = ui.ctx().current_pass_index() == 0;
    let input_enabled = ui.input(|input| input.focused) && !egui::Popup::is_any_open(ui.ctx());
    if !input_enabled {
        view.cancel_layer_gesture();
    }
    if first_pass && input_enabled && !viewport_intercepted {
        // A recent toolbar click can make egui classify the second canvas
        // click as a triple-click. Both gestures enter the same text editor.
        if !view.pending
            && (response.double_clicked_by(egui::PointerButton::Primary)
                || response.triple_clicked_by(egui::PointerButton::Primary))
            && let Some(position) = response.interact_pointer_pos()
            && available.contains(position)
            && crate::primitives::pressed_on_layer(ui, position)
            && preview.contains(position)
        {
            let point = image_point(position, preview, bounds);
            if canvas::double_click(view, tx, &document, point, 10. / display_scale) {
                view.cancel_layer_gesture();
                return true;
            }
            match document.hit_test(point, 8. / display_scale) {
                Ok(Some(Element::Text(text))) if view.section == Section::Layers => {
                    view.begin_inline(
                        tx,
                        captures_app::editor_session::TextInputTarget::Existing {
                            id: text.base.id.clone(),
                        },
                    );
                    return true;
                }
                Err(error) => {
                    view.error = Some(error);
                    return true;
                }
                _ => {}
            }
        }
        if let Some(LayerGesture { kind, .. }) = &mut view.layer_gesture {
            let shift = ui.input(|input| input.modifiers.shift);
            match kind {
                LayerGestureKind::Rotate { snap, .. } => *snap = shift,
                LayerGestureKind::Resize { lock_aspect, .. } => *lock_aspect = shift,
                LayerGestureKind::Move { .. } | LayerGestureKind::Curve { .. } => {}
            }
        }
        let expand_button =
            canvas::expand_button_rect(ui, tokens, view, available, preview).map(|value| value.2);
        // Process in order: hover after release must not change the committed
        // delta, and a press/release in one frame must still be a plain click.
        for event in ui.input(|input| input.events.clone()) {
            match event {
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers,
                    ..
                } if !view.pending => {
                    view.cancel_layer_gesture();
                    // A press on a foreground area over the canvas (the layer
                    // settings popover, a select listbox) belongs to it.
                    if !available.contains(pos)
                        || !crate::primitives::pressed_on_layer(ui, pos)
                        || !preview.contains(pos)
                        || expand_button.is_some_and(|button| button.contains(pos))
                    {
                        continue;
                    }
                    let point = image_point(pos, preview, bounds);
                    let selected = view.selected_layer.as_ref().and_then(|id| {
                        document
                            .elements
                            .iter()
                            .find(|element| &element.base().id == id)
                    });
                    let rotation = selected.and_then(|element| {
                        let base = element.base();
                        if !base.visible || base.locked {
                            return None;
                        }
                        let outline = element.selection_outline().ok()?;
                        let handle = rotation_handle(
                            outline,
                            base.rotation(),
                            display_scale,
                            bounds.width,
                            bounds.height,
                        )?;
                        ((point.x - handle.handle.x).hypot(point.y - handle.handle.y)
                            <= handle.hit_radius)
                            .then_some((
                                base.id.clone(),
                                outline,
                                rotation_angle(base.rotation(), None)?,
                            ))
                    });
                    if let Some((id, outline, initial_radians)) = rotation {
                        owned = true;
                        view.layer_gesture = Some(LayerGesture {
                            kind: LayerGestureKind::Rotate {
                                id,
                                outline,
                                initial_radians,
                                snap: modifiers.shift,
                            },
                            start: point,
                            current: point,
                            preview,
                        });
                        view.error = None;
                        continue;
                    }
                    // Shipping priority: corner resize, then curve handles, then
                    // edge resize, so thin strokes keep their dots grabbable.
                    let curve = selected.and_then(|element| {
                        let Element::Shape(shape) = element else {
                            return None;
                        };
                        if !shape.base.visible || shape.base.locked {
                            return None;
                        }
                        let corner = element
                            .resize_handle_at(point, 8. / display_scale)
                            .ok()
                            .flatten()
                            .is_some_and(|handle| {
                                matches!(
                                    handle,
                                    ResizeHandle::Nw
                                        | ResizeHandle::Ne
                                        | ResizeHandle::Se
                                        | ResizeHandle::Sw
                                )
                            });
                        if corner {
                            return None;
                        }
                        captures_app::editor_canvas::hit_test_curve_handle(
                            shape,
                            point,
                            10. / display_scale,
                        )
                        .map(|handle| (shape.base.id.clone(), handle, Box::new(shape.clone())))
                    });
                    if let Some((id, handle, shape)) = curve {
                        owned = true;
                        view.layer_gesture = Some(LayerGesture {
                            kind: LayerGestureKind::Curve { id, handle, shape },
                            start: point,
                            current: point,
                            preview,
                        });
                        view.error = None;
                        continue;
                    }
                    let resize = (|| -> Result<_, String> {
                        let Some(element) = selected
                            .filter(|element| element.base().visible && !element.base().locked)
                        else {
                            return Ok(None);
                        };
                        let Some(handle) = element.resize_handle_at(point, 8. / display_scale)?
                        else {
                            return Ok(None);
                        };
                        let drag =
                            ResizeDrag::new(&document, &element.base().id, handle, display_scale)?;
                        let preview = drag.preview(point, modifiers.shift)?;
                        Ok(Some((element.base().id.clone(), handle, drag, preview)))
                    })();
                    let resize = match resize {
                        Ok(resize) => resize,
                        Err(error) => {
                            view.error = Some(error);
                            continue;
                        }
                    };
                    if let Some((id, handle, drag, resize)) = resize {
                        owned = true;
                        view.layer_gesture = Some(LayerGesture {
                            kind: LayerGestureKind::Resize {
                                id,
                                handle,
                                drag: Box::new(drag),
                                outline: resize.outline,
                                guides: resize.guides,
                                lock_aspect: modifiers.shift,
                                display_scale,
                            },
                            start: point,
                            current: point,
                            preview,
                        });
                        view.error = None;
                        continue;
                    }
                    let hit = if view.section == Section::Draw {
                        selected
                            .map(|element| {
                                captures_app::editor_canvas::selected_shape_body_hit(
                                    element,
                                    view.draw_shape.preview_key(),
                                    point,
                                    10. / display_scale,
                                )
                                .map(|hit| hit.then_some(element))
                            })
                            .transpose()
                            .map(Option::flatten)
                    } else {
                        document.hit_test(point, 8. / display_scale)
                    };
                    match hit {
                        Ok(hit) => {
                            if view.section == Section::Draw && hit.is_none() {
                                continue;
                            }
                            owned = true;
                            let move_state = hit
                                .map(|element| {
                                    let drag = MoveDrag::new(
                                        &document,
                                        &element.base().id,
                                        display_scale,
                                    )?;
                                    Ok::<_, String>((
                                        element.base().id.clone(),
                                        Box::new(drag),
                                        element.selection_outline()?,
                                    ))
                                })
                                .transpose();
                            let move_state = match move_state {
                                Ok(state) => state,
                                Err(error) => {
                                    view.error = Some(error);
                                    continue;
                                }
                            };
                            view.layer_gesture = Some(LayerGesture {
                                kind: LayerGestureKind::Move {
                                    id: move_state.as_ref().map(|state| state.0.clone()),
                                    drag: move_state.as_ref().map(|state| state.1.clone()),
                                    outline: move_state.as_ref().map(|state| state.2),
                                    guides: Vec::new(),
                                    display_scale,
                                },
                                start: point,
                                current: point,
                                preview,
                            });
                            view.error = None;
                        }
                        Err(error) => view.error = Some(error),
                    }
                }
                egui::Event::Key {
                    key: egui::Key::Escape,
                    pressed: true,
                    ..
                } => view.cancel_layer_gesture(),
                egui::Event::PointerMoved(pos) => {
                    if let Some(gesture) = &mut view.layer_gesture {
                        gesture.current = image_point(pos, preview, bounds);
                    }
                }
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers,
                    ..
                } => {
                    let Some(gesture) = view.layer_gesture.take() else {
                        continue;
                    };
                    let end = image_point(pos, preview, bounds);
                    match gesture.kind {
                        LayerGestureKind::Move {
                            id, display_scale, ..
                        } => {
                            let delta_x = end.x - gesture.start.x;
                            let delta_y = end.y - gesture.start.y;
                            let distance = (delta_x * f64::from(preview.width()) / bounds.width)
                                .hypot(delta_y * f64::from(preview.height()) / bounds.height);
                            if distance >= 3.
                                && let Some(id) = id
                            {
                                view.pending_layer_selection = Some(id.clone());
                                view.invalidate_output();
                                view.submit(
                                    tx,
                                    Request::Layer {
                                        id,
                                        edit: LayerEdit::DragMove {
                                            delta_x,
                                            delta_y,
                                            display_scale,
                                        },
                                    },
                                );
                            } else {
                                view.select_layer_exact(id);
                            }
                        }
                        LayerGestureKind::Rotate {
                            id,
                            outline,
                            initial_radians,
                            ..
                        } => {
                            if let Some(rotation) = preview_rotation(
                                outline,
                                initial_radians,
                                gesture.start,
                                end,
                                modifiers.shift.then_some(view.rotation_snap_degrees),
                            ) && rotation.radians != initial_radians
                            {
                                view.pending_layer_selection = Some(id.clone());
                                view.invalidate_output();
                                view.submit(
                                    tx,
                                    Request::Layer {
                                        id,
                                        edit: LayerEdit::Rotate {
                                            radians: rotation.radians,
                                        },
                                    },
                                );
                            }
                        }
                        LayerGestureKind::Curve { id, handle, .. } => {
                            let distance = ((end.x - gesture.start.x) * display_scale)
                                .hypot((end.y - gesture.start.y) * display_scale);
                            if distance >= 3. {
                                canvas::submit_curve(
                                    view,
                                    tx,
                                    id,
                                    captures_app::editor_canvas::CurveEdit::Move {
                                        handle,
                                        point: end,
                                    },
                                );
                            }
                        }
                        LayerGestureKind::Resize {
                            id,
                            handle,
                            display_scale,
                            ..
                        } => {
                            let distance = ((end.x - gesture.start.x) * display_scale)
                                .hypot((end.y - gesture.start.y) * display_scale);
                            if distance >= 3. {
                                view.pending_layer_selection = Some(id.clone());
                                view.invalidate_output();
                                view.submit(
                                    tx,
                                    Request::Layer {
                                        id,
                                        edit: LayerEdit::Resize {
                                            handle,
                                            current: end,
                                            display_scale,
                                            lock_aspect: modifiers.shift,
                                        },
                                    },
                                );
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    if let Some(LayerGesture {
        kind:
            LayerGestureKind::Resize {
                drag,
                outline,
                guides,
                lock_aspect,
                ..
            },
        current,
        ..
    }) = &mut view.layer_gesture
        && let Ok(resize) = drag.preview(*current, *lock_aspect)
    {
        *outline = resize.outline;
        *guides = resize.guides;
    }
    let move_preview = view.layer_gesture.as_ref().and_then(|gesture| {
        let LayerGestureKind::Move {
            drag: Some(drag),
            id: Some(id),
            ..
        } = &gesture.kind
        else {
            return None;
        };
        let delta = Point {
            x: gesture.current.x - gesture.start.x,
            y: gesture.current.y - gesture.start.y,
        };
        let moving = (delta.x * f64::from(preview.width()) / bounds.width)
            .hypot(delta.y * f64::from(preview.height()) / bounds.height)
            >= 3.;
        Some(if moving {
            drag.preview(delta)
                .map(|preview| (preview.outline, preview.guides))
        } else {
            document
                .elements
                .iter()
                .find(|element| &element.base().id == id)?
                .selection_outline()
                .map(|outline| (outline, Vec::new()))
        })
    });
    match move_preview {
        Some(Ok((move_outline, move_guides))) => {
            if let Some(LayerGesture {
                kind:
                    LayerGestureKind::Move {
                        outline, guides, ..
                    },
                ..
            }) = &mut view.layer_gesture
            {
                *outline = Some(move_outline);
                *guides = move_guides;
            }
        }
        Some(Err(error)) => {
            view.cancel_layer_gesture();
            view.error = Some(error);
        }
        _ => {}
    }
    if response.hovered() || view.layer_gesture.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    }
    if view.section == Section::Draw
        // Idle handle/body hover keeps the transform cursor. A drawing press
        // that ends over the old selection still belongs to drawing.
        && !ui.input(|input| input.pointer.any_down() || input.events.iter().any(|event| {
            matches!(event, egui::Event::PointerButton { button: egui::PointerButton::Primary, .. })
        }))
        && let Some(position) = ui.input(|input| input.pointer.hover_pos())
            .filter(|pos| available.contains(*pos) && preview.contains(*pos) && crate::primitives::pressed_on_layer(ui, *pos))
        && let Some(element) = view.selected_layer.as_ref().and_then(|id| {
            document.elements.iter().find(|element| &element.base().id == id)
        }).filter(|element| element.base().visible && !element.base().locked)
    {
        let point = image_point(position, preview, bounds);
        owned |= element
            .resize_handle_at(point, 8. / display_scale)
            .ok()
            .flatten()
            .is_some()
            || element
                .selection_outline()
                .ok()
                .and_then(|outline| {
                    rotation_handle(
                        outline,
                        element.base().rotation(),
                        display_scale,
                        bounds.width,
                        bounds.height,
                    )
                })
                .is_some_and(|handle| {
                    (point.x - handle.handle.x).hypot(point.y - handle.handle.y)
                        <= handle.hit_radius
                })
            || matches!(element, Element::Shape(shape) if captures_app::editor_canvas::hit_test_curve_handle(shape, point, 10. / display_scale).is_some())
            || captures_app::editor_canvas::selected_shape_body_hit(
                element,
                view.draw_shape.preview_key(),
                point,
                10. / display_scale,
            )
            .unwrap_or(false);
    }
    if owned && input_enabled && !viewport_intercepted {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    }
    let (outline, delta, active_rotation, guides, show_grips) =
        if let Some(gesture) = &view.layer_gesture {
            match &gesture.kind {
                LayerGestureKind::Move {
                    outline, guides, ..
                } => (
                    *outline,
                    Point { x: 0., y: 0. },
                    None,
                    guides.as_slice(),
                    false,
                ),
                LayerGestureKind::Rotate {
                    outline,
                    initial_radians,
                    snap,
                    ..
                } => {
                    let rotation = preview_rotation(
                        *outline,
                        *initial_radians,
                        gesture.start,
                        gesture.current,
                        snap.then_some(view.rotation_snap_degrees),
                    );
                    (
                        rotation.map(|value| value.outline).or(Some(*outline)),
                        Point { x: 0., y: 0. },
                        rotation
                            .map(|value| value.radians)
                            .or(Some(*initial_radians)),
                        &[][..],
                        false,
                    )
                }
                LayerGestureKind::Resize {
                    outline, guides, ..
                } => (
                    Some(*outline),
                    Point { x: 0., y: 0. },
                    None,
                    guides.as_slice(),
                    true,
                ),
                LayerGestureKind::Curve { shape, handle, .. } => {
                    let mut next = (**shape).clone();
                    let _ = captures_app::editor_canvas::apply_curve_edit(
                        &mut next,
                        captures_app::editor_canvas::CurveEdit::Move {
                            handle: *handle,
                            point: gesture.current,
                        },
                    );
                    (
                        Element::Shape(next).selection_outline().ok(),
                        Point { x: 0., y: 0. },
                        None,
                        &[][..],
                        false,
                    )
                }
            }
        } else {
            let selected = view.selected_layer.as_ref().and_then(|id| {
                document
                    .elements
                    .iter()
                    .find(|element| &element.base().id == id)
            });
            (
                selected.and_then(|element| element.selection_outline().ok()),
                Point { x: 0., y: 0. },
                selected
                    .filter(|element| element.base().visible && !element.base().locked)
                    .map(|element| element.base().rotation()),
                &[][..],
                selected.is_some_and(|element| element.base().visible && !element.base().locked),
            )
        };
    if let Some(outline) = outline {
        let project = |point: Point| {
            egui::pos2(
                preview.left() + ((point.x + delta.x) / bounds.width) as f32 * preview.width(),
                preview.top() + ((point.y + delta.y) / bounds.height) as f32 * preview.height(),
            )
        };
        let mut points: Vec<_> = outline.into_iter().map(project).collect();
        points.push(points[0]);
        let painter = ui
            .painter()
            .with_clip_rect(available.intersect(ui.clip_rect()));
        painter.add(egui::Shape::line(
            points,
            egui::Stroke::new(2., tokens.color("theme-accent")),
        ));
        for guide in guides {
            let (start, end) = match guide.orientation {
                GuideOrientation::Vertical => {
                    let x = project(Point {
                        x: guide.position,
                        y: 0.,
                    })
                    .x;
                    (
                        egui::pos2(x, preview.top()),
                        egui::pos2(x, preview.bottom()),
                    )
                }
                GuideOrientation::Horizontal => {
                    let y = project(Point {
                        x: 0.,
                        y: guide.position,
                    })
                    .y;
                    (
                        egui::pos2(preview.left(), y),
                        egui::pos2(preview.right(), y),
                    )
                }
            };
            painter.line_segment(
                [start, end],
                egui::Stroke::new(1., tokens.color("theme-accent")),
            );
        }
        // Lines/arrows keep corner grips only so curve dots stay easy to grab.
        let curve = canvas::selected_curve(view);
        if show_grips {
            for (index, point) in [
                outline[0],
                Point {
                    x: (outline[0].x + outline[1].x) / 2.,
                    y: (outline[0].y + outline[1].y) / 2.,
                },
                outline[1],
                Point {
                    x: (outline[1].x + outline[2].x) / 2.,
                    y: (outline[1].y + outline[2].y) / 2.,
                },
                outline[2],
                Point {
                    x: (outline[2].x + outline[3].x) / 2.,
                    y: (outline[2].y + outline[3].y) / 2.,
                },
                outline[3],
                Point {
                    x: (outline[3].x + outline[0].x) / 2.,
                    y: (outline[3].y + outline[0].y) / 2.,
                },
            ]
            .into_iter()
            .enumerate()
            {
                if curve.is_some() && index % 2 == 1 {
                    continue;
                }
                painter.rect(
                    egui::Rect::from_center_size(project(point), egui::vec2(8., 8.)),
                    1.,
                    tokens.color("surface-raised"),
                    egui::Stroke::new(2., tokens.color("theme-accent")),
                    egui::StrokeKind::Middle,
                );
            }
        }
        if let Some(radians) = active_rotation
            && let Some(handle) =
                rotation_handle(outline, radians, display_scale, bounds.width, bounds.height)
        {
            let anchor = project(handle.anchor);
            let grip = project(handle.handle);
            let stroke = egui::Stroke::new(2., tokens.color("theme-accent"));
            painter.line_segment([anchor, grip], stroke);
            painter.circle(grip, 4.5, tokens.color("surface-raised"), stroke);
        }
    }
    let painter = ui
        .painter()
        .with_clip_rect(available.intersect(ui.clip_rect()));
    let project = |point: Point| {
        egui::pos2(
            preview.left() + (point.x / bounds.width) as f32 * preview.width(),
            preview.top() + (point.y / bounds.height) as f32 * preview.height(),
        )
    };
    match &view.layer_gesture {
        Some(LayerGesture {
            kind: LayerGestureKind::Curve { shape, handle, .. },
            current,
            ..
        }) => {
            if let Some(handles) = canvas::curve_drag_preview(shape, *handle, *current) {
                canvas::paint_curve_handles(&painter, tokens, &handles, project, true);
            }
        }
        None => {
            if let Some((_, handles)) = canvas::selected_curve(view) {
                canvas::paint_curve_handles(&painter, tokens, &handles, project, false);
            }
            if let Some(pointer) = ui.input(|input| input.pointer.hover_pos()).filter(|pos| {
                available.contains(*pos) && crate::primitives::pressed_on_layer(ui, *pos)
            }) && preview.contains(pointer)
                && let Some(hint) = canvas::hover_hint(
                    view,
                    &document,
                    image_point(pointer, preview, bounds),
                    10. / display_scale,
                )
            {
                canvas::paint_hover_tip(ui, tokens, pointer, hint);
            }
        }
        Some(_) => {}
    }
    owned
}

/// One shared token table for UI tests that drive canvas painters directly.
#[cfg(test)]
fn test_tokens() -> &'static Tokens {
    static TOKENS: std::sync::OnceLock<Tokens> = std::sync::OnceLock::new();
    TOKENS.get_or_init(|| crate::tokens::load().into_values().next().unwrap())
}

fn show_shape(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    view: &mut View,
    tx: &Sender<Job>,
    available: egui::Rect,
    preview: egui::Rect,
    viewport_intercepted: bool,
) {
    let Some(presented) = &view.presented else {
        return;
    };
    let bounds = Rect {
        x: 0.,
        y: 0.,
        width: f64::from(presented.pixels.width()),
        height: f64::from(presented.pixels.height()),
    };
    let response = ui.interact(
        available,
        ui.scope_id().with("shape-canvas"),
        if matches!(view.draw_shape, DrawShape::Text | DrawShape::Wand) {
            egui::Sense::click()
        } else {
            egui::Sense::drag()
        },
    );
    let first_pass = ui.ctx().current_pass_index() == 0;
    if view.draw_shape == DrawShape::Text {
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
        }
        if first_pass
            && !view.pending
            && !viewport_intercepted
            && response.clicked_by(egui::PointerButton::Primary)
            && let Some(position) = response.interact_pointer_pos()
            && preview.contains(position)
        {
            let point = image_point(position, preview, bounds);
            let hit = match presented
                .document
                .hit_test(point, 8. * bounds.width / f64::from(preview.width()))
            {
                Ok(hit) => hit,
                Err(error) => {
                    view.error = Some(error);
                    return;
                }
            };
            let target = if let Some(Element::Text(element)) = hit {
                captures_app::editor_session::TextInputTarget::Existing {
                    id: element.base.id.clone(),
                }
            } else {
                captures_app::editor_session::TextInputTarget::New {
                    create: TextCreate {
                        point,
                        text: String::new(),
                        font_size: view.new_text_size,
                        font_family: view
                            .presented
                            .as_ref()
                            .and_then(|presented| {
                                presented
                                    .font_families
                                    .get_key_value("sans")
                                    .or_else(|| presented.font_families.first_key_value())
                                    .map(|(key, _)| key.clone())
                            })
                            .unwrap_or_else(|| "sans".into()),
                        // Shipping places new text in the drawing Color.
                        color: view.new_annotation_style.color.clone(),
                        style_preset: view.new_text_preset.clone(),
                        drop_shadow: view.new_annotation_style.drop_shadow,
                        drop_shadow_style: view.new_annotation_style.drop_shadow_style.clone(),
                    },
                }
            };
            view.begin_inline(tx, target);
        }
        return;
    }
    if view.draw_shape != DrawShape::Wand {
        view.wand_loupe = None;
    }
    if view.draw_shape == DrawShape::Wand {
        // Shipping's crosshair plus the colour loupe over an image, and
        // `not-allowed` elsewhere; panning hides both.
        let pointer = response
            .hover_pos()
            .filter(|pointer| preview.contains(*pointer) && available.contains(*pointer));
        match pointer {
            Some(pointer) if view.viewport_pan.is_none() && !view.pending => {
                let over_image = canvas::show_wand_loupe(ui, tokens, view, preview, pointer);
                ui.ctx().set_cursor_icon(if over_image {
                    egui::CursorIcon::Crosshair
                } else {
                    egui::CursorIcon::NotAllowed
                });
            }
            _ => {
                view.wand_loupe = None;
                if response.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
                }
            }
        }
        if first_pass
            && !view.pending
            && !viewport_intercepted
            && response.clicked_by(egui::PointerButton::Primary)
            && let Some(position) = response.interact_pointer_pos()
            && preview.contains(position)
        {
            view.submit(
                tx,
                Request::RemoveImageBackground {
                    point: image_point(position, preview, bounds),
                    tolerance: view.wand_tolerance,
                    contiguous: view.wand_contiguous,
                },
            );
        }
        return;
    }
    if matches!(view.draw_shape, DrawShape::Erase | DrawShape::Restore) {
        use captures_app::editor_chrome::brush_cursor as brush_ring;
        let clipped_image = preview.intersect(available).intersect(ui.clip_rect());
        let previous_samples = view.brush_points.len();
        let mode = if view.draw_shape == DrawShape::Erase {
            BrushMode::Erase
        } else {
            BrushMode::Restore
        };
        // egui does not report drag_started when down/up arrive in one frame.
        // Topmost hover ownership plus the raw press also admits those clicks.
        let can_start =
            response.drag_started_by(egui::PointerButton::Primary) || response.contains_pointer();
        let mut released = None;
        let image_at = |point| {
            presented
                .document
                .elements
                .iter()
                .rev()
                .find_map(|element| match element {
                    Element::Image(image)
                        if image.base.visible && image.natural_pixel_at(point).is_some() =>
                    {
                        Some(image)
                    }
                    _ => None,
                })
        };
        if first_pass && !view.pending && !viewport_intercepted {
            let mut target = view.brush_points.first().copied().and_then(image_at);
            let mut last_move = None;
            ui.input(|input| {
                for event in &input.events {
                    match event {
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed: true,
                            ..
                        } if can_start && clipped_image.contains(*pos) => {
                            let point = image_point(*pos, preview, bounds);
                            view.brush_points = vec![point];
                            target = image_at(point);
                            last_move = None;
                        }
                        egui::Event::PointerMoved(pos) if !view.brush_points.is_empty() => {
                            let point = image_point(*pos, preview, bounds);
                            if target.is_none_or(|image| image.natural_pixel_at(point).is_some()) {
                                last_move = Some(point);
                            }
                        }
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed: false,
                            ..
                        } if !view.brush_points.is_empty() => {
                            // Release replaces an unpainted movement in this frame,
                            // as in shipping. Even a stationary release stamps again.
                            let point = image_point(*pos, preview, bounds);
                            if target.is_none_or(|image| image.natural_pixel_at(point).is_some()) {
                                view.brush_points.push(point);
                            } else if let Some(point) = last_move {
                                view.brush_points.push(point);
                            }
                            released = Some(());
                            break;
                        }
                        egui::Event::PointerGone if !view.brush_points.is_empty() => {
                            view.brush_points.clear();
                            last_move = None;
                        }
                        _ => {}
                    }
                }
            });
            if released.is_none()
                && let Some(point) = last_move
            {
                view.brush_points.push(point);
            }
        }
        if first_pass
            && previous_samples != view.brush_points.len()
            && released.is_none()
            && let Some(pixels) = &mut view.drawing_preview
        {
            if view.brush_points.is_empty() {
                pixels.cancel();
            } else {
                pixels.request(
                    tx,
                    Request::PaintImageBackground {
                        points: view.brush_points.clone(),
                        size: view.brush_size,
                        softness: view.brush_softness,
                        mode,
                    },
                );
            }
        }
        // Shipping `syncRemoveBgHoverCursor`: over a visible image (or for the
        // whole stroke) the system cursor hides behind the size ring;
        // elsewhere on the canvas it is `not-allowed`; panning hides the ring.
        let stroking = !view.brush_points.is_empty();
        let pointer = ui
            .input(|input| input.pointer.hover_pos())
            .filter(|pointer| stroking || (response.hovered() && available.contains(*pointer)));
        let ring = pointer.and_then(|pointer| {
            let panning = view.viewport_pan.is_some() || ui.input(|input| input.modifiers.command);
            let over_image = image_at(image_point(pointer, preview, bounds)).is_some();
            match brush_ring::hover(over_image, stroking, panning) {
                brush_ring::Hover::Ring => {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::None);
                    Some(pointer)
                }
                brush_ring::Hover::NotAllowed => {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::NotAllowed);
                    None
                }
                brush_ring::Hover::Pan => None,
            }
        });
        let position = |point: Point| {
            egui::pos2(
                preview.left() + (point.x / bounds.width) as f32 * preview.width(),
                preview.top() + (point.y / bounds.height) as f32 * preview.height(),
            )
        };
        let painter = ui.painter().with_clip_rect(clipped_image);
        let feedback = egui::Stroke::new(1.5, ui.visuals().text_color());
        if view.brush_points.len() > 1
            && view
                .drawing_preview
                .as_ref()
                .is_none_or(|pixels| pixels.texture.is_none())
        {
            painter.add(egui::Shape::line(
                view.brush_points.iter().copied().map(position).collect(),
                feedback,
            ));
        }
        if let Some(center) = ring {
            // `position: fixed`: the ring is not clipped to the image or canvas.
            let diameter = brush_ring::screen_diameter(
                view.brush_size,
                f64::from(preview.width()) / bounds.width,
            );
            canvas::paint_brush_cursor(
                &ui.ctx().layer_painter(egui::LayerId::new(
                    egui::Order::Foreground,
                    ui.scope_id().with("brush-cursor"),
                )),
                center,
                diameter as f32,
                mode == BrushMode::Restore,
                tokens.color("theme-accent"),
            );
        }
        if first_pass && released.is_some() {
            let points = std::mem::take(&mut view.brush_points);
            view.submit(
                tx,
                Request::PaintImageBackground {
                    points,
                    size: view.brush_size,
                    softness: view.brush_softness,
                    mode,
                },
            );
        }
        return;
    }
    let previous_geometry = (view.shape_drag, view.freehand_points.len());
    let started = response.drag_started_by(egui::PointerButton::Primary);
    if first_pass
        && !viewport_intercepted
        && started
        && let Some(origin) = ui.input(|input| input.pointer.press_origin())
    {
        let start = image_point(origin, preview, bounds);
        view.select_layer_exact(None);
        view.shape_drag = Some((start, start));
        view.shape_drag_frame = preview;
        if view.draw_shape == DrawShape::Freehand {
            view.freehand_points = vec![start];
        }
    }
    let frame = if view.shape_drag.is_some() {
        view.shape_drag_frame
    } else {
        preview
    };
    if first_pass
        && !viewport_intercepted
        && view.draw_shape == DrawShape::Freehand
        && view.shape_drag.is_some()
    {
        let minimum = 1.5 * bounds.width / f64::from(frame.width());
        // Keep every accepted movement in this frame, not just its final pointer
        // position. Ignore hover events preceding the press and moves after release.
        let mut held = !started;
        ui.input(|input| {
            for event in &input.events {
                match event {
                    egui::Event::PointerButton {
                        button: egui::PointerButton::Primary,
                        pressed,
                        ..
                    } => held = *pressed,
                    egui::Event::PointerMoved(position) if held => {
                        let point = image_point(*position, frame, bounds);
                        let last = view
                            .freehand_points
                            .last()
                            .expect("freehand starts at press origin");
                        if (point.x - last.x).hypot(point.y - last.y) >= minimum {
                            view.freehand_points.push(point);
                        }
                    }
                    _ => {}
                }
            }
        });
    }
    let stopped = response.drag_stopped_by(egui::PointerButton::Primary);
    let position = if stopped {
        // A later hover sample in this frame cannot overwrite release.
        ui.input(|input| {
            input.events.iter().find_map(|event| match event {
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    ..
                } => Some(*pos),
                _ => None,
            })
        })
    } else {
        response.interact_pointer_pos()
    };
    if first_pass
        && !viewport_intercepted
        && (response.dragged_by(egui::PointerButton::Primary) || stopped)
        && let Some(position) = position
        && let Some((_, end)) = &mut view.shape_drag
    {
        *end = image_point(position, frame, bounds);
    }
    if !viewport_intercepted && (response.hovered() || response.dragged()) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
    }
    let style = view.new_annotation_style.clone();
    let opacity = view.new_annotation_opacity;
    if first_pass
        && !viewport_intercepted
        && previous_geometry != (view.shape_drag, view.freehand_points.len())
        && !stopped
        && let Some((start, end)) = view.shape_drag
        && let Some(pixels) = &mut view.drawing_preview
    {
        let request = if view.draw_shape == DrawShape::Freehand {
            Some(Request::CreateFreehandPath {
                create: FreehandPathCreate {
                    points: view.freehand_points.clone(),
                    style: style.clone(),
                    opacity,
                },
            })
        } else {
            view.draw_shape.request(
                start,
                end,
                f64::from(frame.width()) / bounds.width,
                &style,
                opacity,
            )
        };
        if let Some(request) = request {
            pixels.request(tx, request);
        } else {
            pixels.cancel();
        }
    }
    if let Some((start, end)) = view.shape_drag
        && view
            .drawing_preview
            .as_ref()
            .is_none_or(|preview| preview.texture.is_none())
    {
        let position = |point: Point| {
            egui::pos2(
                preview.left() + (point.x / bounds.width) as f32 * preview.width(),
                preview.top() + (point.y / bounds.height) as f32 * preview.height(),
            )
        };
        let rect = egui::Rect::from_two_pos(position(start), position(end));
        let painter = ui
            .painter()
            .with_clip_rect(available.intersect(ui.clip_rect()));
        let color = |value: &str| {
            egui::Color32::from_hex(value)
                .unwrap_or(egui::Color32::TRANSPARENT)
                .gamma_multiply((opacity / 100.).clamp(0., 1.) as f32)
        };
        let fill = style
            .fill
            .as_deref()
            .map_or(egui::Color32::TRANSPARENT, color);
        // Open shapes always stroke, regardless of the retained closed-shape toggle.
        let stroke = if style.has_stroke() || view.draw_shape.closed_kind().is_none() {
            egui::Stroke::new(
                (style.stroke_width / bounds.width) as f32 * preview.width(),
                color(&style.color),
            )
        } else {
            egui::Stroke::NONE
        };
        match view.draw_shape {
            DrawShape::Rectangle => {
                let radius = (12. * preview.width() / bounds.width as f32)
                    .min(rect.width() / 6.)
                    .min(rect.height() / 6.);
                painter.rect(rect, radius, fill, stroke, egui::StrokeKind::Middle);
            }
            DrawShape::Ellipse => {
                painter.add(egui::Shape::ellipse_filled(
                    rect.center(),
                    rect.size() / 2.,
                    fill,
                ));
                painter.add(egui::Shape::ellipse_stroke(
                    rect.center(),
                    rect.size() / 2.,
                    stroke,
                ));
            }
            DrawShape::Triangle | DrawShape::Diamond | DrawShape::Star => {
                let points = view
                    .draw_shape
                    .closed_kind()
                    .expect("closed shape")
                    .polygon(start, end)
                    .expect("polygon kind");
                let mut points: Vec<_> = points.into_iter().map(position).collect();
                painter.add(egui::Shape::mesh(polygon_mesh(points.clone(), fill)));
                if stroke != egui::Stroke::NONE && !points.is_empty() {
                    points.push(points[0]);
                    painter.add(egui::Shape::line(points, stroke));
                }
            }
            DrawShape::Line => {
                painter.line_segment([position(start), position(end)], stroke);
            }
            DrawShape::Freehand => {
                let points: Vec<_> = smooth_path_centerline(&view.freehand_points)
                    .into_iter()
                    .map(position)
                    .collect();
                let width = (style.stroke_width / bounds.width) as f32 * preview.width();
                let color = color(&style.color);
                if points.len() == 1 {
                    painter.circle_filled(points[0], width / 2., color);
                } else if !points.is_empty() {
                    // egui's open path has flat caps; shipping Pen strokes are round.
                    painter.circle_filled(points[0], width / 2., color);
                    painter.circle_filled(points[points.len() - 1], width / 2., color);
                    painter.add(egui::Shape::line(points, egui::Stroke::new(width, color)));
                }
            }
            DrawShape::Arrow => {
                let arrow = ShapeElement {
                    base: ElementBase {
                        id: String::new(),
                        x: start.x,
                        y: start.y,
                        rotation: None,
                        locked: false,
                        visible: true,
                        opacity: 100.,
                        blend_mode: "source-over".into(),
                    },
                    shape: "arrow".into(),
                    end_x: end.x,
                    end_y: end.y,
                    controls: Vec::new(),
                    style: style.clone(),
                    extra: Default::default(),
                };
                painter.add(egui::Shape::mesh(polygon_mesh(
                    arrow_fill_polygon(&arrow)
                        .into_iter()
                        .map(position)
                        .collect(),
                    color(&style.color),
                )));
            }
            DrawShape::Text | DrawShape::Wand | DrawShape::Erase | DrawShape::Restore => {
                unreachable!("pixel tools do not start shape drags")
            }
        }
    }
    if first_pass
        && !viewport_intercepted
        && stopped
        && let Some((start, end)) = view.shape_drag.take()
    {
        let request = if view.draw_shape == DrawShape::Freehand {
            Some(Request::CreateFreehandPath {
                create: FreehandPathCreate {
                    points: std::mem::take(&mut view.freehand_points),
                    style,
                    opacity,
                },
            })
        } else {
            view.draw_shape.request(
                start,
                end,
                f64::from(frame.width()) / bounds.width,
                &style,
                opacity,
            )
        };
        if let Some(request) = request {
            view.submit(tx, request);
        } else if let Some(preview) = &mut view.drawing_preview {
            preview.cancel();
        }
    }
}

fn polygon_mesh(points: Vec<egui::Pos2>, color: egui::Color32) -> egui::Mesh {
    // epaint's filled paths require convex polygons. Arrow necks and stars are concave.
    let coordinates: Vec<_> = points
        .iter()
        .flat_map(|point| [f64::from(point.x), f64::from(point.y)])
        .collect();
    let indices = earcutr::earcut(&coordinates, &[], 2)
        .expect("shared polygon has finite two-dimensional coordinates");
    let mut mesh = egui::Mesh::default();
    // A press or axis-aligned drag has vertices but no triangles. Do not send
    // orphan vertices to egui-wgpu: its zero-length index-buffer slice panics.
    if indices.is_empty() {
        return mesh;
    }
    for point in points {
        mesh.colored_vertex(point, color);
    }
    mesh.indices = indices.into_iter().map(|index| index as u32).collect();
    mesh
}

fn show_crop(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    view: &mut View,
    available: egui::Rect,
    preview: egui::Rect,
    viewport_intercepted: bool,
) {
    let Some(presented) = &view.presented else {
        return;
    };
    let bounds = Rect {
        x: 0.,
        y: 0.,
        width: f64::from(presented.pixels.width()),
        height: f64::from(presented.pixels.height()),
    };
    let response = ui.interact(
        available,
        ui.scope_id().with("crop-canvas"),
        egui::Sense::drag(),
    );
    let aspect = CROP_ASPECTS[view.crop_aspect].1;
    let shift = ui.input(|input| input.modifiers.shift);
    // A selection starts once the press is decidedly a drag, so a click (on
    // the comparison's Hide, say) never leaves a 1 px crop behind while the
    // Crop tool stays ready for the next selection.
    if !viewport_intercepted
        && view.crop_drag.is_none()
        && response.dragged_by(egui::PointerButton::Primary)
        && ui.input(|input| input.pointer.is_decidedly_dragging())
        && let Some(origin) = ui.input(|input| input.pointer.press_origin())
    {
        view.crop_drag = Some(CropDrag::new(
            image_point(origin, preview, bounds),
            bounds,
            aspect,
            shift,
        ));
    }
    if !viewport_intercepted
        && (response.dragged_by(egui::PointerButton::Primary)
            || response.drag_stopped_by(egui::PointerButton::Primary))
        && let Some(position) = response.interact_pointer_pos()
        && let Some(drag) = &mut view.crop_drag
    {
        let rect = drag.update(image_point(position, preview, bounds), aspect, shift);
        view.crop = [rect.x, rect.y, rect.width, rect.height];
    }
    if response.drag_stopped() {
        view.crop_drag = None;
    }
    if response.hovered() || response.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
    }
    let [x, y, width, height] = view.crop;
    let selection = egui::Rect::from_min_max(
        egui::pos2(
            preview.left() + (x / bounds.width) as f32 * preview.width(),
            preview.top() + (y / bounds.height) as f32 * preview.height(),
        )
        .clamp(preview.min, preview.max),
        egui::pos2(
            preview.left() + ((x + width) / bounds.width) as f32 * preview.width(),
            preview.top() + ((y + height) / bounds.height) as f32 * preview.height(),
        )
        .clamp(preview.min, preview.max),
    );
    let painter = ui
        .painter()
        .with_clip_rect(preview.intersect(available).intersect(ui.clip_rect()));
    for (min, max) in [
        (preview.min, egui::pos2(preview.right(), selection.top())),
        (egui::pos2(preview.left(), selection.bottom()), preview.max),
        (
            egui::pos2(preview.left(), selection.top()),
            selection.left_bottom(),
        ),
        (
            selection.right_top(),
            egui::pos2(preview.right(), selection.bottom()),
        ),
    ] {
        painter.rect_filled(
            egui::Rect::from_min_max(min, max),
            0.,
            tokens.color("glass-veil-heavy"),
        );
    }
    let stroke = egui::Stroke::new(tokens.number("s-1"), tokens.color("glass-text"));
    let corners = [
        selection.left_top(),
        selection.right_top(),
        selection.right_bottom(),
        selection.left_bottom(),
        selection.left_top(),
    ];
    painter.extend(egui::Shape::dashed_line(
        &corners,
        stroke,
        tokens.number("s-3"),
        tokens.number("s-2"),
    ));
    let label = painter.layout_no_wrap(
        format!("{width} × {height}"),
        egui::FontId::proportional(tokens.number("text-sm")),
        tokens.color("glass-text"),
    );
    let padding = tokens.number("s-2");
    let origin = egui::pos2(
        (selection.center().x - label.size().x / 2.).clamp(
            preview.left() + padding,
            (preview.right() - label.size().x - padding).max(preview.left() + padding),
        ),
        (selection.top() + padding)
            .min((preview.bottom() - label.size().y - padding).max(preview.top() + padding)),
    );
    painter.rect_filled(
        egui::Rect::from_min_size(origin, label.size()).expand(padding),
        tokens.number("r-sm"),
        tokens.color("glass-strong"),
    );
    painter.galley(origin, label, tokens.color("glass-text"));
}

/// The collapsed bar keeps the former pinned footer's height; the disclosure
/// adds a fixed settings area. Both are even, whole pixels so the canvas
/// geometry stays predictable and odd window heights center images on pixels.
fn export_bar_height(tokens: &Tokens, open: bool) -> f32 {
    let collapsed = tokens.number("s-12") + tokens.number("s-6");
    if open {
        collapsed + export_settings_height(tokens)
    } else {
        collapsed
    }
}

fn export_settings_height(tokens: &Tokens) -> f32 {
    tokens.number("s-12") * 2.
}

const EXPORT_DISCLOSURE_WIDTH: std::ops::RangeInclusive<f32> = 160.0..=210.0;
const EXPORT_SUFFIX_WIDTH: f32 = 68.;
const EXPORT_COPY_WIDTH: f32 = 92.;
const EXPORT_SAVE_WIDTH: f32 = 100.;
const EXPORT_SWITCH_WIDTH: f32 = 128.;
const EXPORT_STEM_WIDTH: std::ops::RangeInclusive<f32> = 128.0..=320.0;

/// Shipping bottom export bar: settings disclosure with a live summary,
/// filename and format suffix, save location, Copy image, the "Save as new
/// file" switch and the primary Save. Everything else in the editor stays usable.
fn show_export_bar(ui: &mut egui::Ui, tokens: &Tokens, view: &mut View, tx: &Sender<Job>) {
    let now = Instant::now();
    if view.notice_until.is_some_and(|until| until <= now) {
        view.notice_until = None;
        view.output_notice = None;
    }
    if view.copied_until.is_some_and(|until| until <= now) {
        view.copied_until = None;
    }
    for until in [view.notice_until, view.copied_until].into_iter().flatten() {
        ui.ctx().request_repaint_after(until - now);
    }
    let ready = view.presented.is_some()
        && !view.pending
        && view.inline.is_none()
        && !view.closed
        && !view.close_requested;
    let (horizontal, vertical) = (tokens.number("s-5"), tokens.number("s-4"));
    let height = export_bar_height(tokens, view.export_settings_open);
    egui::Panel::bottom("editor-export-bar")
        .resizable(false)
        .show_separator_line(false)
        .exact_size(height)
        .frame(
            egui::Frame::new()
                .fill(tokens.color("surface-raised"))
                .inner_margin(egui::Margin::symmetric(horizontal as i8, vertical as i8)),
        )
        .show(ui, |ui| {
            // Contents never exceed this height, so egui reports the exact panel
            // size and the canvas beside it stays on whole pixels.
            ui.set_height(height - 2. * vertical);
            let top = ui.max_rect().top() - vertical + 0.5;
            ui.painter().hline(
                ui.max_rect().x_range().expand(horizontal),
                top,
                egui::Stroke::new(1., tokens.color("border-subtle")),
            );
            ui.spacing_mut().item_spacing.y = tokens.number("s-2");
            if view.export_settings_open {
                let height = export_settings_height(tokens) - tokens.number("s-4");
                egui::Frame::new()
                    .fill(tokens.color("surface-sunken"))
                    .stroke(egui::Stroke::new(1., tokens.color("border")))
                    .corner_radius(tokens.number("r-xl"))
                    .inner_margin(egui::Margin::symmetric(
                        tokens.number("s-5") as i8,
                        tokens.number("s-4") as i8,
                    ))
                    .show(ui, |ui| {
                        let inner = height - 2. * tokens.number("s-4") - 2.;
                        ui.set_width(ui.available_width());
                        ui.set_height(inner);
                        crate::primitives::scroll_area(
                            ui,
                            tokens,
                            egui::ScrollArea::vertical()
                                .id_salt("export-settings")
                                .max_height(inner)
                                .auto_shrink([false, false]),
                            |ui| {
                                ui.add_enabled_ui(ready, |ui| {
                                    show_export_settings(ui, tokens, view)
                                });
                            },
                        );
                    });
            }
            let bar = view.export_view();
            let widths = export_widths(ui.available_width(), ui.spacing().item_spacing.x);
            show_export_heading(ui, tokens, view, ready, bar.as_ref(), widths);
            show_export_row(ui, tokens, view, tx, ready, bar.as_ref(), widths);
        });
}

fn show_export_heading(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    view: &mut View,
    ready: bool,
    bar: Option<&ExportBarView>,
    (disclosure, stem): (f32, f32),
) {
    let small = tokens.number("text-xs");
    let row = small + tokens.number("s-4") + 1.;
    let width = ui.available_width();
    ui.allocate_ui_with_layout(
        egui::vec2(width, row),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.set_height(row);
            ui.add_space(disclosure);
            let heading = stem + EXPORT_SUFFIX_WIDTH + ui.spacing().item_spacing.x;
            ui.allocate_ui_with_layout(
                egui::vec2(heading, row),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.label(
                        RichText::new("Filename")
                            .size(small)
                            .color(tokens.color("text-muted")),
                    );
                    ui.label(
                        RichText::new("Saving to")
                            .size(small)
                            .color(tokens.color("text-subtle")),
                    );
                    let directory = view
                        .export_target
                        .as_ref()
                        .map(|target| target.directory.display().to_string())
                        .unwrap_or_default();
                    let change = RichText::new("Change…").size(small);
                    let reserve = ui.fonts_mut(|fonts| {
                        fonts
                            .layout_no_wrap(
                                "Change…".into(),
                                egui::FontId::proportional(small),
                                egui::Color32::WHITE,
                            )
                            .size()
                            .x
                    }) + ui.spacing().item_spacing.x
                        + 2. * ui.spacing().button_padding.x;
                    let path_width = (ui.available_width() - reserve).max(24.);
                    ui.add_sized(
                        [path_width, row],
                        egui::Label::new(
                            RichText::new(&directory)
                                .size(small)
                                .monospace()
                                .color(tokens.color("text-subtle")),
                        )
                        .truncate(),
                    )
                    .on_hover_text(&directory)
                    .widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::WidgetType::Label,
                            true,
                            format!("Save location: {directory}"),
                        )
                    });
                    let response = ui
                        .add_enabled(
                            ready && view.folder_picker.is_none(),
                            egui::Button::new(change).small().frame(false),
                        )
                        .on_hover_text("Change save location");
                    response.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::WidgetType::Button,
                            response.enabled(),
                            "Change save location",
                        )
                    });
                    if response.clicked() {
                        view.choose_folder(ui.ctx());
                    }
                },
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if view.last_saved.is_some()
                    && ui
                        .add_enabled(
                            ready,
                            egui::Button::new(RichText::new("Show in Folder").size(small)).small(),
                        )
                        .on_hover_text("Open the folder that contains the saved file")
                        .clicked()
                {
                    view.reveal_saved();
                }
                // Like shipping, editor errors share the export status line
                // instead of pushing the canvas down.
                let (text, color) = if let Some(error) = &view.export_error {
                    (error.clone(), tokens.color("danger-text"))
                } else if let Some(error) = &view.error {
                    (error.clone(), tokens.color("danger-text"))
                } else if let Some(error) = bar.and_then(|bar| bar.error.clone()) {
                    (error, tokens.color("danger-text"))
                } else if let Some(notice) = &view.output_notice {
                    (notice.clone(), tokens.color("positive-text"))
                } else if let Some(bar) = bar {
                    let color = if bar.hint_warning {
                        "caution-text"
                    } else {
                        "text-subtle"
                    };
                    (bar.hint.clone(), tokens.color(color))
                } else {
                    (String::new(), tokens.color("text-subtle"))
                };
                // An elided label already supplies the full message as its tooltip.
                ui.add(egui::Label::new(RichText::new(text).size(small).color(color)).truncate());
            });
        },
    );
}

/// Disclosure and filename widths: fixed actions first, then the disclosure
/// grows to fit its summary, then the filename field takes what remains.
fn export_widths(width: f32, spacing: f32) -> (f32, f32) {
    let fixed = EXPORT_SUFFIX_WIDTH
        + EXPORT_COPY_WIDTH
        + EXPORT_SWITCH_WIDTH
        + EXPORT_SAVE_WIDTH
        + 5. * spacing;
    let flexible = (width - fixed).max(0.);
    let disclosure = (flexible - EXPORT_STEM_WIDTH.start()).clamp(
        *EXPORT_DISCLOSURE_WIDTH.start(),
        *EXPORT_DISCLOSURE_WIDTH.end(),
    );
    let stem = (flexible - disclosure).clamp(*EXPORT_STEM_WIDTH.start(), *EXPORT_STEM_WIDTH.end());
    (disclosure, stem)
}

fn show_export_row(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    view: &mut View,
    tx: &Sender<Job>,
    ready: bool,
    bar: Option<&ExportBarView>,
    (disclosure, stem): (f32, f32),
) {
    let height = tokens.number("h-lg");
    ui.horizontal(|ui| {
        ui.set_height(height);
        let summary = bar.map_or_else(String::new, |bar| bar.summary.clone());
        if export_disclosure(ui, tokens, disclosure, view.export_settings_open, &summary).clicked()
        {
            view.export_settings_open = !view.export_settings_open;
        }
        ui.add_enabled_ui(ready, |ui| {
            let response = ui
                .add_sized(
                    [stem, height],
                    egui::TextEdit::singleline(&mut view.filename)
                        .align(egui::Align2::LEFT_CENTER)
                        .hint_text("Filename"),
                )
                .on_hover_text("Saved filename");
            response.widget_info(|| {
                egui::WidgetInfo::text_edit(
                    response.enabled(),
                    "",
                    view.filename.clone(),
                    "Saved filename",
                )
            });
            if response.changed() {
                let stem = view.filename.clone();
                view.update_export_target(|target, _| target.set_stem(stem));
            }
            let previous = view.export_options.format;
            let transparent = view
                .presented
                .as_ref()
                .is_some_and(|presented| presented.document.background.is_none());
            let suffix = bar.map_or_else(|| ".png".to_owned(), |bar| bar.suffix.clone());
            // Shipping `.filename-format-select`: the suffix trigger over a
            // token listbox; JPEG explains that it fills transparent areas.
            let jpeg_note = if transparent {
                "Fills in transparent areas."
            } else {
                ""
            };
            let options = [
                crate::primitives::SelectOption::new(ExportFormat::Png, "PNG"),
                crate::primitives::SelectOption::new(ExportFormat::Jpeg, "JPEG")
                    .description(jpeg_note),
                crate::primitives::SelectOption::new(ExportFormat::Webp, "WebP"),
            ];
            let format =
                crate::primitives::Select::new("export-format", "Format", EXPORT_SUFFIX_WIDTH)
                    .height(height)
                    .trigger_text(&suffix)
                    .show(ui, tokens, &options, &view.export_options.format);
            format.response.on_hover_text("Format");
            if let Some(chosen) = format.chosen {
                view.export_options.format = chosen;
            }
            if view.export_options.format != previous {
                view.invalidate_output();
                view.export_target_changed();
            }
            let copied = view.copied_until.is_some();
            let label = if copied { "Copied" } else { "Copy image" };
            let mut copy = egui::Button::new(RichText::new(label).color(if copied {
                tokens.color("positive-text")
            } else {
                tokens.color("text")
            }));
            if copied {
                copy = copy.fill(tokens.color("positive-surface"));
            }
            let response = ui
                .add_sized([EXPORT_COPY_WIDTH, height], copy)
                .on_hover_text("Copy the edited image to the clipboard. Does not save a file.");
            response.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::WidgetType::Button,
                    response.enabled(),
                    if copied { "Copied" } else { "Copy image" },
                )
            });
            if copied {
                // The bundled UI font has no check glyph; draw the shipping check icon.
                let text = ui.fonts_mut(|fonts| {
                    fonts
                        .layout_no_wrap(
                            label.into(),
                            egui::TextStyle::Button.resolve(ui.style()),
                            egui::Color32::WHITE,
                        )
                        .size()
                        .x
                });
                let center = response.rect.center() - egui::vec2(text / 2. + 10., 0.);
                crate::capture_controls::paint_icon(
                    ui.painter(),
                    "check",
                    egui::Rect::from_center_size(center, egui::Vec2::splat(16.)),
                    1.8,
                    tokens.color("positive-text"),
                );
            }
            if response.clicked() {
                view.copy(tx);
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let plan_ready = bar.is_some_and(|bar| bar.plan.is_some() && bar.error.is_none());
            let save = egui::Button::new(
                RichText::new(if view.pending && view.export_job {
                    "Saving…"
                } else {
                    "Save"
                })
                .strong()
                .color(tokens.color("theme-accent-ink")),
            )
            .fill(tokens.color("theme-accent"));
            let hint = bar.map_or_else(String::new, |bar| bar.hint.clone());
            let response = ui
                .add_enabled_ui(ready && plan_ready && view.folder_picker.is_none(), |ui| {
                    ui.add_sized([EXPORT_SAVE_WIDTH, height], save)
                })
                .inner
                .on_hover_text(&hint);
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Button, response.enabled(), "Save")
            });
            if response.clicked() {
                view.save(tx);
            }
            if let Some(bar) = bar.filter(|bar| !bar.format_requires_copy) {
                let mut enabled = bar.saving_copy;
                let response = ui
                    .add_enabled_ui(ready, |ui| {
                        export_switch(ui, tokens, &mut enabled, "Save as new file")
                    })
                    .inner
                    .on_hover_text("Save as a new file and leave the original untouched");
                if response.changed() {
                    view.update_export_target(|target, format| {
                        target.set_save_as_new(enabled, format);
                    });
                }
            }
        });
    });
}

fn export_disclosure(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    width: f32,
    open: bool,
    summary: &str,
) -> egui::Response {
    let size = egui::vec2(width - ui.spacing().item_spacing.x, tokens.number("h-lg"));
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::Button,
            true,
            open,
            format!("Export settings: {summary}"),
        )
    });
    let response = response.on_hover_text(if open {
        "Hide export settings"
    } else {
        "Show export settings"
    });
    if ui.is_rect_visible(rect) {
        let hovered = response.hovered();
        let painter = ui.painter();
        painter.rect(
            rect,
            tokens.number("r-md"),
            tokens.color(if hovered { "control-hover" } else { "control" }),
            egui::Stroke::new(
                1.,
                tokens.color(if hovered {
                    "border-strong"
                } else {
                    "control-border"
                }),
            ),
            egui::StrokeKind::Inside,
        );
        let left = rect.left() + tokens.number("s-5");
        let chevron = tokens.number("s-6");
        let text_width = rect.width() - tokens.number("s-5") - chevron - tokens.number("s-4");
        let label = painter.layout_no_wrap(
            "Export settings".into(),
            egui::FontId::proportional(tokens.number("text-sm")),
            tokens.color(if hovered { "text" } else { "text-muted" }),
        );
        let mut job = egui::text::LayoutJob::simple_singleline(
            summary.into(),
            egui::FontId::monospace(tokens.number("text-2xs")),
            tokens.color("text-subtle"),
        );
        job.wrap = egui::text::TextWrapping::truncate_at_width(text_width);
        let summary = ui.fonts_mut(|fonts| fonts.layout_job(job));
        let gap = tokens.number("s-1");
        let top = rect.center().y - (label.size().y + gap + summary.size().y) / 2.;
        painter.galley(egui::pos2(left, top), label.clone(), egui::Color32::WHITE);
        painter.galley(
            egui::pos2(left, top + label.size().y + gap),
            summary,
            egui::Color32::WHITE,
        );
        let center = egui::pos2(
            rect.right() - tokens.number("s-4") - chevron / 2.,
            rect.center().y,
        );
        crate::capture_controls::paint_icon(
            painter,
            if open {
                "editor-chevron-up"
            } else {
                "editor-chevron-down"
            },
            egui::Rect::from_center_size(center, egui::Vec2::splat(15.)),
            1.8,
            tokens.color(if hovered { "text" } else { "text-muted" }),
        );
        if response.has_focus() {
            crate::primitives::focus_indicated(ui.ctx());
            painter.rect_stroke(
                rect.expand(2.),
                tokens.number("r-md") + 2.,
                ui.visuals().selection.stroke,
                egui::StrokeKind::Outside,
            );
        }
    }
    response
}

/// Shipping labelled pill switch: the whole label toggles it, accent when on,
/// focusable and announced as a checkbox.
pub(crate) fn export_switch(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    value: &mut bool,
    label: &str,
) -> egui::Response {
    let text = ui.painter().layout_no_wrap(
        label.into(),
        egui::FontId::proportional(tokens.number("text-sm")),
        tokens.color("text"),
    );
    let pill = egui::vec2(28., 16.);
    let gap = tokens.number("s-3");
    let size = egui::vec2(pill.x + gap + text.size().x, tokens.number("h-lg"));
    let (rect, mut response) = ui.allocate_exact_size(size, egui::Sense::click());
    if response.clicked() {
        *value = !*value;
        response.mark_changed();
    }
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Checkbox, ui.is_enabled(), *value, label)
    });
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let alpha = if ui.is_enabled() { 1. } else { 0.55 };
        let track = egui::Rect::from_center_size(
            egui::pos2(rect.left() + pill.x / 2., rect.center().y),
            pill,
        );
        let (fill, border, knob) = if *value {
            (
                tokens.color("theme-accent"),
                egui::Color32::TRANSPARENT,
                tokens.color("theme-accent-ink"),
            )
        } else {
            (
                tokens.color("control"),
                tokens.color("control-border"),
                tokens.color("text-muted"),
            )
        };
        painter.rect(
            track,
            track.height() / 2.,
            fill.gamma_multiply(alpha),
            egui::Stroke::new(1., border.gamma_multiply(alpha)),
            egui::StrokeKind::Inside,
        );
        let radius = track.height() / 2. - 3.;
        let x = if *value {
            track.right() - 3. - radius
        } else {
            track.left() + 3. + radius
        };
        painter.circle_filled(
            egui::pos2(x, track.center().y),
            radius,
            knob.gamma_multiply(alpha),
        );
        painter.galley(
            egui::pos2(track.right() + gap, rect.center().y - text.size().y / 2.),
            text,
            tokens.color("text").gamma_multiply(alpha),
        );
        if response.has_focus() {
            crate::primitives::focus_indicated(ui.ctx());
            painter.rect_stroke(
                track.expand(2.),
                track.height() / 2. + 2.,
                ui.visuals().selection.stroke,
                egui::StrokeKind::Outside,
            );
        }
    }
    response
}

#[derive(Clone, Copy)]
enum ExportGroup {
    Size,
    Custom,
    Quality,
    Preset,
    Maximum,
    Estimate,
    /// Shipping `.screenshot-show-comparison`, while a hidden comparison applies.
    Comparison,
}

/// Export settings behind the disclosure: output size, save quality and the
/// live size estimate. The format lives in the filename suffix menu. Groups
/// wrap into rows explicitly; egui cannot measure nested groups before placing.
fn show_export_settings(ui: &mut egui::Ui, tokens: &Tokens, view: &mut View) {
    let previous = view.export_options;
    let mut groups = vec![(ExportGroup::Size, 200.)];
    if matches!(view.export_options.size, ExportSize::Custom { .. }) {
        groups.push((ExportGroup::Custom, 250.));
    }
    groups.push((ExportGroup::Quality, 150.));
    match view.export_options.quality {
        ExportQuality::Compress => groups.push((ExportGroup::Preset, 160.)),
        ExportQuality::Maximum => groups.push((ExportGroup::Maximum, 190.)),
        ExportQuality::Preserve => {}
    }
    groups.push((ExportGroup::Estimate, 150.));
    if view.export_options.quality != ExportQuality::Preserve && view.compare_dismissed {
        groups.push((ExportGroup::Comparison, 160.));
    }
    let spacing = tokens.number("s-5");
    let available = ui.available_width();
    let mut rows: Vec<Vec<(ExportGroup, f32)>> = vec![Vec::new()];
    let mut used = 0.;
    for (group, width) in groups {
        let row = rows.last_mut().expect("one row");
        if !row.is_empty() && used + spacing + width > available {
            rows.push(vec![(group, width)]);
            used = width;
        } else {
            used += if row.is_empty() {
                width
            } else {
                spacing + width
            };
            row.push((group, width));
        }
    }
    let height = tokens.number("text-xs") + tokens.number("s-2") + tokens.number("h-md");
    for row in rows {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = spacing;
            for (group, width) in row {
                ui.allocate_ui_with_layout(
                    egui::vec2(width, height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_min_size(egui::vec2(width, height));
                        show_export_group(ui, tokens, view, group);
                    },
                );
            }
        });
    }
    if view.export_options != previous {
        view.invalidate_output();
        if view.export_options.format != previous.format
            || view.export_options.quality != previous.quality
        {
            view.export_target_changed();
        }
        if view.export_options.quality != previous.quality {
            // Shipping `applyQualityMode`: a new mode shows the comparison
            // again; Preserve also recentres its split.
            view.compare_dismissed = false;
            if view.export_options.quality == ExportQuality::Preserve {
                view.compare_split = compare::DEFAULT_SPLIT;
                view.compare_error = None;
            }
        }
    }
}

fn show_export_group(ui: &mut egui::Ui, tokens: &Tokens, view: &mut View, group: ExportGroup) {
    let source_size = view
        .presented
        .as_ref()
        .map(|presented| presented.pixels.dimensions())
        .unwrap_or((0, 0));
    let small = tokens.number("text-xs");
    let caption = |ui: &mut egui::Ui, text: &str| {
        ui.label(
            RichText::new(text)
                .size(small)
                .color(tokens.color("text-muted")),
        );
    };
    match group {
        ExportGroup::Size => {
            caption(ui, "Output size");
            ui.horizontal(|ui| {
                let options = &mut view.export_options;
                // Shipping `CustomSelect` with each size's description.
                let selected = match options.size {
                    ExportSize::Original => 0,
                    ExportSize::Percent { percent: 75 } => 1,
                    ExportSize::Percent { .. } => 2,
                    ExportSize::Custom { .. } => 3,
                };
                let choices = [
                    crate::primitives::SelectOption::new(0, "Original")
                        .description("Keep the capture’s pixel dimensions."),
                    crate::primitives::SelectOption::new(1, "75%")
                        .description("Save at 75% of the pixel width and height."),
                    crate::primitives::SelectOption::new(2, "50%")
                        .description("Save at half the pixel width and height."),
                    crate::primitives::SelectOption::new(3, "Custom")
                        .description("Choose exact pixel dimensions."),
                ];
                match crate::primitives::Select::new("output-size", "Output size", 108.)
                    .show(ui, tokens, &choices, &selected)
                    .chosen
                {
                    Some(0) => options.size = ExportSize::Original,
                    Some(1) => options.size = ExportSize::Percent { percent: 75 },
                    Some(2) => options.size = ExportSize::Percent { percent: 50 },
                    Some(_) => {
                        view.custom_export_size = [source_size.0, source_size.1];
                        options.size = ExportSize::Custom {
                            width: source_size.0,
                            height: source_size.1,
                        };
                    }
                    None => {}
                }
                let dimensions = match options.size.dimensions(source_size.0, source_size.1) {
                    Ok((width, height)) => RichText::new(format!("{width} × {height}"))
                        .monospace()
                        .size(tokens.number("text-2xs"))
                        .color(tokens.color("text-subtle")),
                    Err(_) => RichText::new("Invalid size")
                        .size(tokens.number("text-2xs"))
                        .color(tokens.color("danger-text")),
                };
                ui.label(dimensions);
            });
        }
        ExportGroup::Custom => {
            caption(ui, "Width × height");
            let old = view.custom_export_size;
            ui.horizontal(|ui| {
                ui.add(
                    egui::DragValue::new(&mut view.custom_export_size[0])
                        .range(0..=16_384)
                        .prefix("W "),
                )
                .on_hover_text("Custom output width");
                ui.label("×");
                ui.add(
                    egui::DragValue::new(&mut view.custom_export_size[1])
                        .range(0..=16_384)
                        .prefix("H "),
                )
                .on_hover_text("Custom output height");
                ui.toggle_value(&mut view.export_aspect_locked, "Lock")
                    .on_hover_text("Lock output aspect ratio");
            });
            if view.export_aspect_locked && source_size.0 > 0 && source_size.1 > 0 {
                if view.custom_export_size[0] != old[0] {
                    view.custom_export_size[1] =
                        ((u64::from(view.custom_export_size[0]) * u64::from(source_size.1)
                            + u64::from(source_size.0) / 2)
                            / u64::from(source_size.0))
                        .max(1)
                        .min(u64::from(u32::MAX)) as u32;
                } else if view.custom_export_size[1] != old[1] {
                    view.custom_export_size[0] =
                        ((u64::from(view.custom_export_size[1]) * u64::from(source_size.0)
                            + u64::from(source_size.1) / 2)
                            / u64::from(source_size.1))
                        .max(1)
                        .min(u64::from(u32::MAX)) as u32;
                }
            }
            view.export_options.size = ExportSize::Custom {
                width: view.custom_export_size[0],
                height: view.custom_export_size[1],
            };
        }
        ExportGroup::Quality => {
            caption(ui, "Save quality");
            let format = view.export_options.format;
            let choices: Vec<_> = export::SAVE_QUALITY_MODES
                .iter()
                .map(|mode| {
                    crate::primitives::SelectOption::new(*mode, export::quality_mode_label(*mode))
                        .description(export::quality_mode_description(*mode, format))
                })
                .collect();
            let current = view.export_options.quality;
            if let Some(mode) =
                crate::primitives::Select::new("output-quality-mode", "Save quality", 150.)
                    .show(ui, tokens, &choices, &current)
                    .chosen
                && mode != current
            {
                let options = &mut view.export_options;
                options.quality = mode;
                options.max_size_bytes = (mode == ExportQuality::Maximum)
                    .then(|| view.max_size_unit.bytes(&view.max_size_text).unwrap_or(0));
            }
        }
        ExportGroup::Preset => {
            caption(ui, "Quality");
            ui.horizontal(|ui| {
                let options = &mut view.export_options;
                let format = options.format;
                let selected = output_preset(options);
                let choices: Vec<_> = export::QUALITY_PRESETS
                    .iter()
                    .map(|preset| {
                        crate::primitives::SelectOption::new(Some(preset.quality), preset.label)
                            .description(preset.description(format))
                    })
                    .collect();
                let current = selected.map(|_| options.quality_value);
                if let Some(Some(quality)) = crate::primitives::Select::new(
                    "output-quality-preset",
                    "Compression quality",
                    104.,
                )
                .trigger_text(selected.unwrap_or("Custom"))
                .show(ui, tokens, &choices, &current)
                .chosen
                {
                    options.quality_value = quality;
                    // Shared encoding owns PNG palette selection.
                    options.png.max_colors = None;
                }
                let minimum = if options.format == ExportFormat::Jpeg {
                    40
                } else {
                    1
                };
                options.quality_value = options.quality_value.clamp(minimum, 100);
                ui.add(egui::DragValue::new(&mut options.quality_value).range(minimum..=100))
                    .on_hover_text("Compression quality value");
            });
        }
        ExportGroup::Maximum => {
            caption(ui, "Maximum file size");
            let help = export::maximum_size_help(view.export_options.format);
            ui.horizontal(|ui| {
                let value = ui
                    .add(
                        egui::TextEdit::singleline(&mut view.max_size_text)
                            .desired_width(88.)
                            .font(egui::TextStyle::Monospace),
                    )
                    .on_hover_text(help.as_str());
                value.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, true, "Maximum file size")
                });
                let mut unit = view.max_size_unit;
                let units: Vec<_> = FileSizeUnit::ALL
                    .iter()
                    .map(|unit| crate::primitives::SelectOption::new(*unit, unit.label()))
                    .collect();
                if let Some(chosen) = crate::primitives::Select::new(
                    "output-maximum-unit",
                    "Screenshot file size unit",
                    72.,
                )
                .show(ui, tokens, &units, &unit)
                .chosen
                {
                    unit = chosen;
                }
                if unit != view.max_size_unit {
                    view.set_max_size_unit(unit);
                }
            });
            // The group was laid out before a same-frame mode change.
            if view.export_options.quality == ExportQuality::Maximum {
                view.export_options.max_size_bytes = Some(view.max_size_bytes());
            }
        }
        ExportGroup::Estimate => {
            caption(ui, "Est. size");
            let bar = view.export_view();
            ui.horizontal(|ui| {
                ui.set_height(tokens.number("h-md"));
                let label = bar
                    .as_ref()
                    .map_or_else(|| "—".to_owned(), |bar| bar.estimate_label.clone());
                let color = if view.estimate.pending {
                    "text-subtle"
                } else {
                    "text"
                };
                ui.label(RichText::new(label).strong().color(tokens.color(color)))
                    .on_hover_text(
                        "Estimated export file size for the current format, quality, and output size",
                    );
                if let Some(delta) = bar.and_then(|bar| bar.delta) {
                    let color = if delta.percent < 0 {
                        "positive-text"
                    } else {
                        "caution-text"
                    };
                    ui.label(RichText::new(delta.label).size(small).color(tokens.color(color)))
                        .on_hover_text("Change versus the original image, before this export");
                }
            });
        }
        ExportGroup::Comparison => {
            caption(ui, compare::SHOW_CAPTION);
            if ui
                .add(
                    egui::Button::new(compare::SHOW)
                        .min_size(egui::vec2(0., tokens.number("h-md"))),
                )
                .clicked()
            {
                view.compare_dismissed = false;
            }
        }
    }
}

fn layer_label(element: &Element) -> &str {
    match element {
        Element::Image(image) => &image.name,
        Element::Text(_) => "Text",
        Element::Shape(shape) => &shape.shape,
        Element::Path(_) => "Freehand",
    }
}

#[derive(Clone, Copy)]
enum LayerAction {
    Copy,
    Paste,
    Duplicate,
    Delete,
    MergeDown,
    MergeVisible,
    Flatten,
}

fn layer_action_enabled(view: &View, action: LayerAction, target_id: Option<&str>) -> bool {
    if view.pending || view.inline.is_some() || view.closed || view.close_requested {
        return false;
    }
    let Some(presented) = &view.presented else {
        return false;
    };
    if matches!(action, LayerAction::Paste) {
        return presented.can_paste_layer
            && target_id.is_none_or(|id| {
                presented
                    .document
                    .elements
                    .iter()
                    .any(|element| element.base().id == id)
            });
    }
    if matches!(action, LayerAction::MergeVisible) {
        return presented.can_merge_visible;
    }
    if matches!(action, LayerAction::Flatten) {
        return presented.can_flatten;
    }
    if matches!(action, LayerAction::MergeDown) {
        return target_id.is_some_and(|id| presented.merge_down_ids.iter().any(|item| item == id));
    }
    presented
        .document
        .elements
        .iter()
        .find(|element| Some(element.base().id.as_str()) == target_id)
        .is_some_and(|element| !matches!(action, LayerAction::Delete) || !element.base().locked)
}

fn dispatch_layer_action(
    view: &mut View,
    tx: &Sender<Job>,
    action: LayerAction,
    target_id: Option<String>,
) {
    if !layer_action_enabled(view, action, target_id.as_deref()) {
        return;
    }
    let request = match action {
        LayerAction::Copy => Request::CopyLayer {
            id: target_id.expect("enabled copy has a target"),
        },
        LayerAction::Paste => {
            let new_id = uuid::Uuid::new_v4().to_string();
            view.pending_layer_selection = Some(new_id.clone());
            Request::PasteLayer {
                new_id,
                after_id: target_id,
            }
        }
        LayerAction::Duplicate => {
            let new_id = uuid::Uuid::new_v4().to_string();
            view.pending_layer_selection = Some(new_id.clone());
            Request::Layer {
                id: target_id.expect("enabled duplicate has a target"),
                edit: LayerEdit::Duplicate { new_id },
            }
        }
        LayerAction::Delete => Request::Layer {
            id: target_id.expect("enabled delete has a target"),
            edit: LayerEdit::Delete,
        },
        LayerAction::MergeDown => {
            let new_id = uuid::Uuid::new_v4().to_string();
            view.pending_layer_selection = Some(new_id.clone());
            view.combine_pending = true;
            Request::MergeDown {
                id: target_id.expect("enabled merge down has a target"),
                new_id,
            }
        }
        LayerAction::MergeVisible => {
            let new_id = uuid::Uuid::new_v4().to_string();
            view.pending_layer_selection = Some(new_id.clone());
            view.combine_pending = true;
            Request::MergeVisible { new_id }
        }
        LayerAction::Flatten => {
            let new_id = uuid::Uuid::new_v4().to_string();
            view.pending_layer_selection = Some(new_id.clone());
            view.combine_pending = true;
            Request::Flatten { new_id }
        }
    };
    view.cancel_edit_gestures();
    view.viewport_pan = None;
    view.submit(tx, request);
}

fn layer_context_menu(
    ui: &mut egui::Ui,
    view: &mut View,
    tx: &Sender<Job>,
    target_id: Option<String>,
) {
    for (label, action) in [
        ("Copy layer", LayerAction::Copy),
        ("Paste layer", LayerAction::Paste),
        ("Duplicate", LayerAction::Duplicate),
        ("Delete", LayerAction::Delete),
        ("Merge down", LayerAction::MergeDown),
        ("Merge visible", LayerAction::MergeVisible),
        ("Flatten image", LayerAction::Flatten),
    ] {
        if target_id.is_none()
            && !matches!(
                action,
                LayerAction::Paste | LayerAction::MergeVisible | LayerAction::Flatten
            )
        {
            continue;
        }
        if ui
            .add_enabled(
                layer_action_enabled(view, action, target_id.as_deref()),
                egui::Button::new(label),
            )
            .clicked()
        {
            dispatch_layer_action(view, tx, action, target_id.clone());
            ui.close();
        }
    }
}

/// Shipping Properties for the selected layer under Select: the Shift
/// rotation snap section, then the live text fields, image
/// Width/Height/X/Y, or annotation style and curve controls. Visibility,
/// lock, rename, blend, opacity, transforms, arrange and combine live in the
/// Layers rows and their ⋯ menu.
fn show_layer_properties(ui: &mut egui::Ui, tokens: &Tokens, view: &mut View, tx: &Sender<Job>) {
    let Some(presented) = &view.presented else {
        return;
    };
    let document = presented.document.clone();
    let Some(element) = document
        .elements
        .iter()
        .find(|element| Some(&element.base().id) == view.selected_layer.as_ref())
    else {
        if document.elements.is_empty() {
            inspector::section(ui, tokens, |ui| {
                inspector::hint(ui, tokens, "No layers. Undo to restore a deleted layer.");
            });
        }
        return;
    };
    if view.tool_shows_transform_chrome() {
        inspector::section(ui, tokens, |ui| {
            inspector::labelled(ui, tokens, "Shift rotation snap", |ui| {
                let mut degrees = view.rotation_snap_degrees;
                if crate::primitives::NumberInput::new(
                    "rotation-snap",
                    "Shift rotation snap",
                    ui.available_width(),
                )
                .range(1. ..=180.)
                .show(ui, tokens, &mut degrees)
                .changed()
                {
                    view.rotation_snap_degrees = degrees.round().clamp(1., 180.);
                    view.layer_gesture = None;
                }
            });
            inspector::hint(
                ui,
                tokens,
                &format!(
                    "Hold Shift while dragging the rotate handle to snap in {}° increments.",
                    view.rotation_snap_degrees
                ),
            );
        });
    }
    inspector::section(ui, tokens, |ui| match element {
        Element::Text(_) => show_text(ui, tokens, view, tx),
        Element::Image(image) => show_image_geometry(ui, tokens, view, tx, image),
        // Style fields stay live while an edit applies; their edits queue.
        Element::Shape(shape) => {
            show_annotation(
                ui,
                tokens,
                view,
                tx,
                &shape.base.id,
                &shape.style,
                matches!(
                    shape.shape.as_str(),
                    "rectangle" | "ellipse" | "triangle" | "diamond" | "star"
                ),
            );
            ui.add_enabled_ui(!view.pending, |ui| {
                ui.spacing_mut().item_spacing.y = tokens.number("s-5");
                canvas::show_curve_controls(ui, tokens, view, tx, shape);
            });
        }
        Element::Path(path) => {
            show_annotation(ui, tokens, view, tx, &path.base.id, &path.style, false);
        }
    });
}

/// Shipping image Width/Height/X/Y (`.screenshot-number-pair`): live
/// NumberInputs; each field's burst of changes is one undo step.
fn show_image_geometry(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    view: &mut View,
    tx: &Sender<Job>,
    image: &captures_app::editor::ImageElement,
) {
    use captures_app::editor_layers::{self as shared, geometry as copy};
    let locked = image.base.locked;
    let fields = [
        (copy::WIDTH, copy::WIDTH_LABEL),
        (copy::HEIGHT, copy::HEIGHT_LABEL),
        ("X", copy::X_LABEL),
        ("Y", copy::Y_LABEL),
    ];
    ui.add_enabled_ui(!locked, |ui| {
        // One grid of four cells: `--s-4` between rows and columns.
        ui.spacing_mut().item_spacing.y = tokens.number("s-4");
        for row in 0..2 {
            inspector::pair(ui, tokens, |ui, column, width| {
                let index = row * 2 + column;
                let (title, label) = fields[index];
                inspector::labelled(ui, tokens, title, |ui| {
                    let mut value = view.layer_geometry[index].round();
                    let mut input = crate::primitives::NumberInput::new(
                        ("layer-geometry", index),
                        label,
                        width,
                    );
                    if index < 2 {
                        input = input.range(1. ..=copy::MAX_SIZE);
                    }
                    let response = input.show(ui, tokens, &mut value);
                    let response = if locked {
                        response.on_disabled_hover_text(copy::LOCKED)
                    } else if index < 2 {
                        response.on_hover_text(copy::KEEPS_ASPECT)
                    } else {
                        response
                    };
                    if response.changed() {
                        view.layer_geometry[index] = value;
                        let (mut x, mut y, mut w, mut h) = (None, None, None, None);
                        match index {
                            0 => {
                                w = Some(value);
                                let size = shared::image_size_at_width(image, value);
                                view.layer_geometry[1] = size.1;
                            }
                            1 => {
                                h = Some(value);
                                let size = shared::image_size_at_height(image, value);
                                view.layer_geometry[0] = size.0;
                            }
                            2 => x = Some(value),
                            _ => y = Some(value),
                        }
                        view.live_edit(
                            tx,
                            format!("geometry:{}:{index}", image.base.id),
                            Request::Layer {
                                id: image.base.id.clone(),
                                edit: LayerEdit::Geometry {
                                    x,
                                    y,
                                    width: w,
                                    height: h,
                                },
                            },
                        );
                    }
                });
            });
        }
    });
    inspector::hint(
        ui,
        tokens,
        if locked {
            copy::LOCKED
        } else {
            copy::PROPORTIONAL
        },
    );
}

/// Shipping selected-text properties: Text style, Text, Font and Size, the
/// format buttons, Text color, Text background (and its color), then Drop
/// shadow.
fn show_text(ui: &mut egui::Ui, tokens: &Tokens, view: &mut View, tx: &Sender<Job>) {
    let Some(fields) = &mut view.text else {
        return;
    };
    if let Some(presented) = &view.presented {
        // Shipping `TextStylePicker` shows the layer's current treatment.
        let current = captures_app::editor_text::text_style_preset_id(
            &fields.staged.font_family,
            fields.staged.background.is_some(),
            fields.staged.outlined,
            fields.staged.rounded_background,
        );
        let mut value = Some(current.to_owned());
        let changed = inspector::labelled(ui, tokens, "Text style", |ui| {
            pickers::text_style_picker(
                ui,
                tokens,
                "Text style",
                &presented.text_style_presets,
                &mut value,
                false,
            )
        });
        if changed
            && let Some(preset) = presented
                .text_style_presets
                .iter()
                .find(|preset| Some(preset.id) == value.as_deref())
        {
            fields.staged.apply_preset(preset);
            view.new_text_preset = Some(preset.id.into());
        }
    }
    inspector::labelled(ui, tokens, "Text", |ui| {
        ui.add(
            egui::TextEdit::multiline(&mut fields.staged.text)
                .desired_width(f32::INFINITY)
                .desired_rows(4),
        )
    });
    let options = view
        .presented
        .as_ref()
        .map(|presented| font_family_options(&presented.font_families))
        .unwrap_or_default();
    inspector::pair(ui, tokens, |ui, column, width| {
        if column == 0 {
            inspector::labelled(ui, tokens, "Font", |ui| {
                // Shipping labels and order ("Sans serif", …), never pinned asset names.
                let mut choices: Vec<_> = options
                    .iter()
                    .map(|(key, label)| {
                        crate::primitives::SelectOption::new(key.clone(), label.as_str())
                    })
                    .collect();
                if !options
                    .iter()
                    .any(|(key, _)| *key == fields.staged.font_family)
                {
                    choices.push(crate::primitives::SelectOption::new(
                        fields.staged.font_family.clone(),
                        fields.staged.font_family.as_str(),
                    ));
                }
                let chosen = crate::primitives::Select::new("text-font-family", "Font", width)
                    .show(ui, tokens, &choices, &fields.staged.font_family)
                    .chosen;
                drop(choices);
                if let Some(family) = chosen {
                    fields.staged.font_family = family;
                }
            });
        } else {
            inspector::labelled(ui, tokens, "Size", |ui| {
                crate::primitives::NumberInput::new("text-size", "Text size", width)
                    .range(8. ..=512.)
                    .show(ui, tokens, &mut fields.staged.font_size);
            });
        }
    });
    text_format_buttons(
        ui,
        tokens,
        &mut fields.staged.bold,
        &mut fields.staged.italic,
        &mut fields.staged.align,
    );
    swatch_color(ui, tokens, colors::TEXT_COLOR, &mut fields.staged.color);
    let mut plate = fields.staged.background.is_some();
    if inspector::check_row(ui, tokens, &mut plate, "Text background").changed() {
        // Shipping: a new plate is `#111318` and clears outline and rounding.
        if plate {
            fields.staged.background = Some("#111318".into());
            fields.staged.outlined = false;
            fields.staged.rounded_background = false;
        } else {
            fields.staged.background = None;
        }
    }
    if let Some(background) = &mut fields.staged.background {
        swatch_color(ui, tokens, colors::BACKGROUND, background);
    }
    drop_shadow_fields(
        ui,
        tokens,
        "text",
        &mut fields.staged.drop_shadow,
        &mut fields.staged.shadow,
    );
    let invalid_color = egui::Color32::from_hex(&fields.staged.color).is_err()
        || (fields.staged.drop_shadow
            && egui::Color32::from_hex(&fields.staged.shadow.color).is_err())
        || fields
            .staged
            .background
            .as_deref()
            .is_some_and(|color| egui::Color32::from_hex(color).is_err());
    // Shipping applies text edits live. A typing burst in one field is one
    // undo step; toggles and menu choices are each their own.
    let patch = fields.staged.patch(&fields.accepted);
    if patch == TextPatch::default()
        || invalid_color
        || fields.sent.as_ref() == Some(&fields.staged)
    {
        return;
    }
    fields.sent = Some(fields.staged.clone());
    let id = fields.id.clone();
    let key = match ui.memory(|memory| memory.focused()) {
        Some(focused) => format!("text:{id}:{focused:?}"),
        None => view.live_once("text"),
    };
    view.live_edit(tx, key, Request::EditText { id, patch });
}

/// Shipping `.screenshot-format-buttons`: B, I and the three alignment icons
/// in five equal 32 pt columns; the active ones fill with the accent.
fn text_format_buttons(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    bold: &mut bool,
    italic: &mut bool,
    align: &mut String,
) {
    use captures_app::editor_chrome::text_format as f;
    let gap = tokens.number("s-2");
    let (row, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), f::BUTTON_HEIGHT as f32),
        egui::Sense::hover(),
    );
    let columns = f::COLUMNS as f32;
    let width = (row.width() - (columns - 1.) * gap) / columns;
    for index in 0..f::COLUMNS {
        let rect = egui::Rect::from_min_size(
            egui::pos2(row.left() + index as f32 * (width + gap), row.top()),
            egui::vec2(width, row.height()),
        );
        let (label, active) = match index {
            0 => (f::BOLD, *bold),
            1 => (f::ITALIC, *italic),
            _ => {
                let (value, label, _) = f::ALIGN[index - 2];
                (label, align == value)
            }
        };
        let response = ui.interact(
            rect,
            ui.scope_id().with(("text-format", label)),
            egui::Sense::click(),
        );
        let (fill, border, ink) = if active {
            (
                tokens.color("theme-accent"),
                tokens.color("theme-accent"),
                tokens.color("theme-accent-ink"),
            )
        } else {
            (
                tokens.color(if response.hovered() {
                    "control-hover"
                } else {
                    "control"
                }),
                tokens.color("border-subtle"),
                tokens.color("text-muted"),
            )
        };
        ui.painter().rect(
            rect,
            tokens.number("r-sm"),
            fill,
            egui::Stroke::new(1., border),
            egui::StrokeKind::Inside,
        );
        if response.has_focus() {
            crate::primitives::focus_ring(ui, tokens, rect, tokens.number("r-sm"));
        }
        if index < 2 {
            let mut job = egui::text::LayoutJob::default();
            job.append(
                if index == 0 { "B" } else { "I" },
                0.,
                egui::TextFormat {
                    font_id: egui::FontId::new(
                        tokens.number("text-sm"),
                        egui::FontFamily::Name(crate::ui_fonts::SEMIBOLD.into()),
                    ),
                    color: ink,
                    italics: index == 1,
                    ..Default::default()
                },
            );
            let galley = ui.painter().layout_job(job);
            ui.painter()
                .galley(rect.center() - galley.size() / 2., galley, ink);
        } else {
            chrome::icon(
                ui.painter(),
                f::ALIGN[index - 2].2,
                rect.center(),
                f::ICON as f32,
                1.8,
                ink,
            );
        }
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::Button, true, active, label)
        });
        if response.clicked() {
            match index {
                0 => *bold = !*bold,
                1 => *italic = !*italic,
                _ => *align = f::ALIGN[index - 2].0.into(),
            }
        }
    }
}

/// Shipping `<label>Name<RangeSlider/></label>`: the visible name above the
/// slider, whose readout, ticks and mark labels follow it.
fn labelled_slider(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    label: &str,
    slider: crate::primitives::RangeSlider,
    value: &mut f64,
) -> bool {
    inspector::labelled(ui, tokens, label, |ui| {
        slider.show(ui, tokens, value).changed()
    })
}

/// Shipping `ColorField` swatches for a staged annotation color.
fn swatch_color(ui: &mut egui::Ui, tokens: &Tokens, label: &str, value: &mut String) {
    if let Some(color) = pickers::color_field(ui, tokens, label, value, false, true) {
        *value = color;
    }
}

/// Shipping `DropShadowFields`: the Drop shadow check row and, while on, the
/// indented Shadow color, Opacity and Blur sliders and the X/Y offset pair.
/// Returns whether the toggle changed; compare `shadow` for field edits.
fn drop_shadow_fields(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    id: &str,
    enabled: &mut bool,
    shadow: &mut DropShadowStyle,
) -> bool {
    let toggled = inspector::check_row(ui, tokens, enabled, "Drop shadow").changed();
    if *enabled {
        ui.push_id(("drop-shadow", id), |ui| {
            inspector::indented(ui, tokens, |ui| {
                swatch_color(ui, tokens, colors::SHADOW_COLOR, &mut shadow.color);
                let text = format!("{}%", shadow.opacity.round());
                labelled_slider(
                    ui,
                    tokens,
                    "Opacity",
                    crate::primitives::RangeSlider::new(
                        "shadow-opacity",
                        "Shadow opacity",
                        ui.available_width(),
                        0. ..=100.,
                        text,
                    ),
                    &mut shadow.opacity,
                );
                let text = format!("{} px", shadow.blur.round());
                labelled_slider(
                    ui,
                    tokens,
                    "Blur",
                    crate::primitives::RangeSlider::new(
                        "shadow-blur",
                        "Shadow blur",
                        ui.available_width(),
                        0. ..=100.,
                        text,
                    ),
                    &mut shadow.blur,
                );
                inspector::pair(ui, tokens, |ui, column, width| {
                    let (label, name, value) = if column == 0 {
                        ("X offset", "Shadow X offset", &mut shadow.offset_x)
                    } else {
                        ("Y offset", "Shadow Y offset", &mut shadow.offset_y)
                    };
                    inspector::labelled(ui, tokens, label, |ui| {
                        let mut offset = *value;
                        if crate::primitives::NumberInput::new(
                            ("shadow-offset", column),
                            name,
                            width,
                        )
                        .range(-500. ..=500.)
                        .commit_on_enter()
                        .show(ui, tokens, &mut offset)
                        .changed()
                        {
                            *value = offset.round();
                        }
                    });
                });
            });
        });
    }
    toggled
}

/// Shipping applies stroke, opacity, fill and shadow changes as they are made
/// (`ScreenshotEditor.tsx` shape/path properties). A burst in one field is
/// one undo step (`Request::Live`); each toggle is its own.
fn show_annotation(
    ui: &mut egui::Ui,
    tokens: &Tokens,
    view: &mut View,
    tx: &Sender<Job>,
    id: &str,
    original: &ElementStyle,
    closed: bool,
) {
    let Some(fields) = &mut view.annotation else {
        return;
    };
    let before = fields.clone();
    let style = &mut fields.style;
    if closed {
        let mut stroke = style.has_stroke();
        if inspector::check_row(ui, tokens, &mut stroke, "Stroke").changed() {
            style.stroke_enabled = Some(stroke);
        }
    }
    if !closed || style.has_stroke() {
        swatch_color(ui, tokens, colors::STROKE_COLOR, &mut style.color);
        let text = format!("{} px", style.stroke_width.round());
        labelled_slider(
            ui,
            tokens,
            "Stroke width",
            crate::primitives::RangeSlider::new(
                "annotation-stroke-width",
                "Stroke width",
                ui.available_width(),
                2. ..=40.,
                text,
            ),
            &mut style.stroke_width,
        );
    }
    let mut opacity = view.layer_opacity;
    let changed = labelled_slider(
        ui,
        tokens,
        captures_app::editor_layers::menu::OPACITY,
        crate::primitives::RangeSlider::new(
            "annotation-opacity",
            captures_app::editor_layers::menu::OPACITY,
            ui.available_width(),
            0. ..=100.,
            format!("{}%", opacity.round()),
        ),
        &mut opacity,
    );
    if changed {
        let id = id.to_owned();
        view.layer_opacity = opacity;
        view.live_edit(
            tx,
            format!("opacity:{id}"),
            Request::Layer {
                id,
                edit: LayerEdit::Opacity { opacity },
            },
        );
    }
    let Some(fields) = &mut view.annotation else {
        return;
    };
    // Shipping order: Drop shadow, then Filled shape and Fill color.
    let mut shadow = fields.style.has_drop_shadow();
    if drop_shadow_fields(ui, tokens, "annotation", &mut shadow, &mut fields.shadow) {
        fields.style.drop_shadow = Some(shadow);
    }
    let style = &mut fields.style;
    if closed {
        let mut filled = style.fill.is_some();
        if inspector::check_row(ui, tokens, &mut filled, "Filled shape").changed() {
            style.fill = filled.then(|| style.color.clone());
        }
        if let Some(fill) = &mut style.fill {
            swatch_color(ui, tokens, colors::FILL_COLOR, fill);
        }
    }
    if *fields == before {
        return;
    }
    let patch = fields.patch(original);
    let invalid = [
        patch.color.as_deref(),
        match &patch.fill {
            OptionalNullable::Value(fill) => Some(fill.as_str()),
            _ => None,
        },
        patch
            .drop_shadow_style
            .as_ref()
            .and_then(|shadow| shadow.color.as_deref()),
    ]
    .into_iter()
    .flatten()
    .any(|color| egui::Color32::from_hex(color).is_err());
    if patch == AnnotationStylePatch::default() || invalid {
        return;
    }
    let field = fields.changed_field(&before);
    let id = id.to_owned();
    let key = match field {
        Some(field) => format!("style:{id}:{field}"),
        None => view.live_once("style"),
    };
    view.live_edit(
        tx,
        key,
        Request::Layer {
            id,
            edit: LayerEdit::AnnotationStyle { patch },
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, time::Duration};

    #[test]
    fn combine_uses_shared_capabilities_and_selects_new_layer_only_on_success() {
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel();
        for action in [
            LayerAction::MergeDown,
            LayerAction::MergeVisible,
            LayerAction::Flatten,
        ] {
            let mut view = View::default();
            let mut initial = presented(false);
            let target = initial.document.elements[0].base().id.clone();
            initial.merge_down_ids = vec![target.clone()];
            initial.can_merge_visible = true;
            initial.can_flatten = true;
            view.receive(&ctx, Ok(initial));
            view.activate_tool(Section::Draw, Some(DrawShape::Freehand));
            let selected = view.selected_layer.clone();
            assert!(!layer_action_enabled(
                &view,
                LayerAction::MergeDown,
                Some("removed")
            ));
            for blocked in 0..3 {
                view.pending = blocked == 0;
                view.close_requested = blocked == 1;
                view.closed = blocked == 2;
                dispatch_layer_action(&mut view, &tx, action, Some(target.clone()));
                assert!(rx.try_recv().is_err());
            }
            view.closed = false;
            dispatch_layer_action(&mut view, &tx, action, Some(target.clone()));
            assert!(matches!(rx.try_recv(), Ok(Job::Apply(_))));
            view.receive(&ctx, Err("render failed".into()));
            assert_eq!(view.selected_layer, selected);
            assert!(view.pending_layer_selection.is_none() && !view.combine_pending);
            assert_eq!(view.section, Section::Draw);
            assert_eq!(view.draw_shape, DrawShape::Freehand);

            dispatch_layer_action(&mut view, &tx, action, Some(target.clone()));
            let new_id = match rx.try_recv().unwrap() {
                Job::Apply(Request::MergeDown { id, new_id }) => {
                    assert_eq!(id, target);
                    new_id
                }
                Job::Apply(Request::MergeVisible { new_id } | Request::Flatten { new_id }) => {
                    new_id
                }
                _ => panic!("unexpected combine request"),
            };
            assert_eq!(
                view.pending_layer_selection.as_deref(),
                Some(new_id.as_str())
            );
            assert_eq!(view.selected_layer, selected);
            let mut result = presented(true);
            let Element::Image(image) = &mut Arc::make_mut(&mut result.document).elements[0] else {
                panic!()
            };
            image.base.id = new_id.clone();
            view.receive(&ctx, Ok(result));
            assert_eq!(view.selected_layer.as_deref(), Some(new_id.as_str()));
            assert_eq!(view.section, Section::Layers);
            assert!(!view.combine_pending);
            for unavailable in [
                LayerAction::MergeDown,
                LayerAction::MergeVisible,
                LayerAction::Flatten,
            ] {
                dispatch_layer_action(&mut view, &tx, unavailable, Some(new_id.clone()));
            }
            assert!(
                rx.try_recv().is_err(),
                "new snapshot capabilities replace the old ones"
            );
        }
    }

    #[test]
    fn live_edits_queue_while_a_job_runs_and_fold_by_key() {
        let ctx = egui::Context::default();
        let mut view = View::default();
        view.receive(&ctx, Ok(presented(false)));
        let (tx, rx) = mpsc::channel();
        let x = |value: f64| Request::Layer {
            id: "capture-background".into(),
            edit: LayerEdit::Geometry {
                x: Some(value),
                y: None,
                width: None,
                height: None,
            },
        };
        view.live_edit(&tx, "geometry:x".into(), x(1.));
        assert!(
            matches!(rx.try_recv(), Ok(Job::Apply(Request::Live { key, .. })) if key == "geometry:x")
        );
        assert!(view.pending);
        for value in [12., 123.] {
            view.live_edit(&tx, "geometry:x".into(), x(value));
        }
        view.live_edit(&tx, "opacity".into(), x(7.));
        assert!(
            rx.try_recv().is_err(),
            "live edits wait for the running job"
        );
        assert_eq!(
            view.live_queue.len(),
            2,
            "the newest edit per key waits, in order"
        );
        view.receive(&ctx, Ok(presented(false)));
        view.flush_live(&tx);
        let Ok(Job::Apply(Request::Live { key, request })) = rx.try_recv() else {
            panic!("the queued edit applies once the worker is free");
        };
        assert_eq!(key, "geometry:x");
        assert!(matches!(
            *request,
            Request::Layer { edit: LayerEdit::Geometry { x: Some(value), .. }, .. } if value == 123.
        ));
        view.flush_live(&tx);
        assert!(rx.try_recv().is_err(), "one job at a time");
        view.receive(&ctx, Err("rejected".into()));
        assert!(
            view.live_queue.is_empty(),
            "a rejected edit drops the rest of its burst"
        );
    }

    #[test]
    fn layer_rows_rename_on_double_click_drag_to_reorder_and_open_the_layer_menu() {
        let ctx = egui::Context::default();
        let tokens = crate::tokens::load().remove("light-mustard").unwrap();
        let mut view = View::default();
        let mut initial = presented(false);
        let mut second = initial.document.elements[0].clone();
        let Element::Image(image) = &mut second else {
            panic!()
        };
        image.base.id = "front".into();
        image.base.locked = false;
        image.name = "Front".into();
        Arc::make_mut(&mut initial.document).elements.push(second);
        view.receive(&ctx, Ok(initial));
        view.section = Section::Layers;
        let (tx, rx) = mpsc::channel();
        let frame = |view: &mut View, events| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(500., 700.),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    egui::CentralPanel::default().show(ui, |ui| {
                        layers::show(ui, &tokens, view, &tx, 400.);
                    });
                },
            );
            output.textures_delta.clear();
            output
        };
        let position = |output: &egui::FullOutput, label: &str| {
            output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.job.text == label => {
                        Some(text.pos + text.galley.rect.center().to_vec2())
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("missing text: {label}"))
        };
        let press = |view: &mut View, pos, pressed| {
            frame(
                view,
                vec![egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
        };
        let output = frame(&mut view, vec![]);
        let front = position(&output, "Front");
        let back = position(&output, "Original screenshot");
        assert!(front.y < back.y, "rows list front to back");
        frame(&mut view, vec![egui::Event::PointerMoved(front)]);
        for _ in 0..2 {
            press(&mut view, front, true);
            press(&mut view, front, false);
        }
        assert_eq!(view.selected_layer.as_deref(), Some("front"));
        assert!(
            view.layers.rename.is_some(),
            "double-clicking an image row renames it inline"
        );
        frame(&mut view, vec![]);
        assert_eq!(
            view.layers
                .rename
                .as_ref()
                .map(|rename| rename.value.as_str()),
            Some("Front")
        );
        view.layers.rename.as_mut().unwrap().value = "Renamed".into();
        // Leaving the field (a click elsewhere) commits, like shipping's blur.
        press(&mut view, egui::pos2(250., 650.), true);
        press(&mut view, egui::pos2(250., 650.), false);
        frame(&mut view, vec![]);
        assert!(view.layers.rename.is_none());
        match rx.try_recv() {
            Ok(Job::Apply(Request::Layer {
                id,
                edit: LayerEdit::Rename { name },
            })) => {
                assert_eq!(id, "front");
                assert_eq!(name, "Renamed");
            }
            other => panic!("expected one rename, got {}", other.is_ok()),
        }
        view.pending = false;

        // Drag the front row below the back row: one reorder request.
        let target = back + egui::vec2(0., 20.);
        frame(&mut view, vec![egui::Event::PointerMoved(front)]);
        press(&mut view, front, true);
        for step in 1..=4 {
            let pos = front.lerp(target, step as f32 / 4.);
            frame(&mut view, vec![egui::Event::PointerMoved(pos)]);
        }
        assert!(view.layers.dragging() == Some("front"));
        press(&mut view, target, false);
        frame(&mut view, vec![]);
        match rx.try_recv() {
            Ok(Job::Apply(Request::Layer {
                id,
                edit:
                    LayerEdit::Reorder {
                        target_id,
                        placement: LayerPlacement::After,
                    },
            })) => {
                assert_eq!(
                    (id.as_str(), target_id.as_str()),
                    ("front", "capture-background")
                );
            }
            _ => panic!("expected one reorder below the back row"),
        }
        assert!(rx.try_recv().is_err(), "a drag is one undo step");
        view.pending = false;

        // The ⋯ button opens the layer settings popover for its row.
        view.layers.menu = Some("front".into());
        frame(&mut view, vec![]); // A new popover area measures itself first.
        let output = frame(&mut view, vec![]);
        for label in [
            "Blend mode",
            "Bring to front",
            "Send to back",
            "Merge visible",
            "Duplicate",
        ] {
            position(&output, label);
        }
    }

    #[test]
    fn context_menu_targets_row_not_selection_and_keeps_output_until_acceptance() {
        let ctx = egui::Context::default();
        let tokens = crate::tokens::load().remove("light-mustard").unwrap();
        let mut view = View::default();
        let mut initial = presented(false);
        let target = initial.document.elements[0].base().id.clone();
        let mut second = initial.document.elements[0].clone();
        let Element::Image(image) = &mut second else {
            panic!()
        };
        image.base.id = "other".into();
        image.name = "Other".into();
        Arc::make_mut(&mut initial.document).elements.push(second);
        let document = initial.document.clone();
        let pixels = initial.pixels.clone();
        view.receive(&ctx, Ok(initial));
        view.select_layer_exact(Some("other".into()));
        view.output = Some((view.texture.as_ref().unwrap().clone(), 101));
        let (tx, rx) = mpsc::channel();
        let frame = |view: &mut View, events| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(500., 700.),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    egui::CentralPanel::default().show(ui, |ui| {
                        layers::show(ui, &tokens, view, &tx, 400.);
                    });
                    if ctx.current_pass_index() == 0 {
                        ctx.request_discard("layer menu multipass");
                    }
                },
            );
            output.textures_delta.clear();
            output
        };
        let click = |view: &mut View, pos, button| {
            frame(view, vec![egui::Event::PointerMoved(pos)]);
            for pressed in [true, false] {
                frame(
                    view,
                    vec![egui::Event::PointerButton {
                        pos,
                        button,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    }],
                );
            }
        };
        let position = |output: &egui::FullOutput, label: &str| {
            output
                .shapes
                .iter()
                .rev() // Popup text is above same-named inspector buttons.
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.job.text == label => {
                        Some(text.pos + text.galley.rect.center().to_vec2())
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("missing text: {label}"))
        };
        let output = frame(&mut view, vec![]);
        let row = position(&output, "Original screenshot");
        click(&mut view, row, egui::PointerButton::Secondary);
        assert!(egui::Popup::is_any_open(&ctx));
        assert_eq!(view.selected_layer.as_deref(), Some("other"));
        assert!(view.output.is_some() && rx.try_recv().is_err());
        frame(
            &mut view,
            vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert!(!egui::Popup::is_any_open(&ctx));
        assert!(rx.try_recv().is_err());
        click(&mut view, row, egui::PointerButton::Secondary);
        let output = frame(&mut view, vec![]);
        click(
            &mut view,
            position(&output, "Copy layer"),
            egui::PointerButton::Primary,
        );
        assert!(matches!(rx.try_recv(), Ok(Job::Apply(Request::CopyLayer { id })) if id == target));
        assert!(
            rx.try_recv().is_err(),
            "discarded egui passes must not enqueue twice"
        );
        let mut copied = presented(false);
        copied.document = document;
        copied.pixels = pixels;
        copied.copied_layer = true;
        copied.can_paste_layer = true;
        view.receive(&ctx, Ok(copied));
        assert_eq!(view.selected_layer.as_deref(), Some("other"));
        assert!(view.output.is_some());
        click(&mut view, row, egui::PointerButton::Secondary);
        let output = frame(&mut view, vec![]);
        click(
            &mut view,
            position(&output, "Duplicate"),
            egui::PointerButton::Primary,
        );
        assert!(
            matches!(rx.try_recv(), Ok(Job::Apply(Request::Layer { id, edit: LayerEdit::Duplicate { .. } })) if id == target)
        );
        assert_eq!(view.selected_layer.as_deref(), Some("other"));
        assert!(view.pending_layer_selection.is_some());
        view.receive(&ctx, Err("rejected".into()));
        assert_eq!(view.selected_layer.as_deref(), Some("other"));
        assert!(view.pending_layer_selection.is_none() && view.output.is_some());
        assert!(!layer_action_enabled(
            &view,
            LayerAction::Delete,
            Some(&target)
        ));
        assert!(!layer_action_enabled(
            &view,
            LayerAction::Paste,
            Some("removed")
        ));
        assert!(layer_action_enabled(&view, LayerAction::Paste, None));
        for blocked in 0..3 {
            view.pending = blocked == 0;
            view.close_requested = blocked == 1;
            view.closed = blocked == 2;
            for action in [
                LayerAction::Copy,
                LayerAction::Paste,
                LayerAction::Duplicate,
                LayerAction::Delete,
            ] {
                dispatch_layer_action(&mut view, &tx, action, Some(target.clone()));
            }
            assert!(rx.try_recv().is_err());
        }
        view.closed = false;
        let mut empty = presented(false);
        Arc::make_mut(&mut empty.document).elements.clear();
        empty.can_paste_layer = true;
        view.receive(&ctx, Ok(empty));
        frame(&mut view, vec![]);
        click(
            &mut view,
            egui::pos2(40., 80.),
            egui::PointerButton::Secondary,
        );
        let output = frame(&mut view, vec![]);
        click(
            &mut view,
            position(&output, "Paste layer"),
            egui::PointerButton::Primary,
        );
        assert!(matches!(
            rx.try_recv(),
            Ok(Job::Apply(Request::PasteLayer { after_id: None, .. }))
        ));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn properties_titles_stay_fixed_while_the_minimum_inspector_scrolls() {
        for (section, shape, selected, title, field) in [
            (
                Section::Layers,
                DrawShape::Rectangle,
                true,
                "Original screenshot",
                "Shift rotation snap",
            ),
            (
                Section::Draw,
                DrawShape::Rectangle,
                false,
                "Rectangle",
                "Stroke",
            ),
            (
                Section::Draw,
                DrawShape::Text,
                false,
                "Text",
                "New text style",
            ),
            (
                Section::Geometry,
                DrawShape::Rectangle,
                false,
                "Crop",
                "Aspect ratio",
            ),
        ] {
            let ctx = egui::Context::default();
            let tokens = crate::tokens::load().remove("light-mustard").unwrap();
            let (tx, _rx) = mpsc::channel();
            let mut view = View::default();
            view.receive(&ctx, Ok(presented(true)));
            view.section = section;
            view.draw_shape = shape;
            view.select_layer_exact(selected.then(|| "capture-background".into()));
            if section == Section::Geometry {
                // A staged crop exposes the size/actions below Aspect ratio;
                // the no-selection hint alone fits without scrolling.
                view.crop_tool = true;
                view.crop_previous = Some(view.crop);
                view.crop = [1., 0., 3., 2.];
            }
            let document = view.presented.as_ref().unwrap().document.clone();
            let frame = |view: &mut View, events| {
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(760., 540.),
                        )),
                        events,
                        ..Default::default()
                    },
                    |ui| show(ui, &tokens, view, &tx),
                );
                output.textures_delta.clear();
                output
            };
            let position = |output: &egui::FullOutput, label: &str| {
                output
                    .shapes
                    .iter()
                    .rev()
                    .find_map(|shape| match &shape.shape {
                        egui::Shape::Text(text)
                            if text.galley.job.text == label
                                // The Text style preview also says "Text";
                                // layer rows repeat image titles in smaller type.
                                && (label != title || text.galley.job.sections.first().is_some_and(|section| {
                                    section.format.font_id.size == tokens.number("text-md")
                                        && section.format.font_id.family == egui::FontFamily::Name("semibold".into())
                                })) => Some(text.pos),
                        _ => None,
                    })
            };
            frame(&mut view, vec![]);
            let before = frame(&mut view, vec![]);
            let heading = position(&before, title).expect(title);
            let initial_field = position(&before, field).expect(field);
            let pointer = egui::pos2(600., heading.y + 60.);
            frame(&mut view, vec![egui::Event::PointerMoved(pointer)]);
            frame(
                &mut view,
                vec![egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0., -600.),
                    phase: egui::TouchPhase::Move,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
            for _ in 0..30 {
                frame(&mut view, vec![]);
            }
            let after = frame(&mut view, vec![]);
            assert_eq!(
                position(&after, title),
                Some(heading),
                "{title}: title stays outside scrolling content"
            );
            assert_ne!(
                position(&after, field),
                Some(initial_field),
                "{title}: fields actually scroll"
            );
            assert_eq!(view.presented.as_ref().unwrap().document, document);
        }
    }

    #[test]
    fn header_keeps_canvas_toolbar_clear_of_controls_and_hides_history_at_1040() {
        // Assertions compare layouts with each other, never platform font widths.
        let ctx = egui::Context::default();
        crate::ui_fonts::install(&ctx);
        let tokens = crate::tokens::load().remove("light-mustard").unwrap();
        tokens.apply(&ctx, true);
        let (tx, _rx) = mpsc::channel::<Job>();
        let layout = |width: f32| {
            let mut view = View::default();
            view.receive(&ctx, Ok(presented(true)));
            view.viewport.zoom_percent = 5.; // Show the slider without a canvas pass.
            let mut header = None;
            for _ in 0..2 {
                // The first pass loads the fonts.
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width, 540.),
                        )),
                        ..Default::default()
                    },
                    |ui| {
                        egui::Panel::top("header").show(ui, |ui| {
                            header = Some(chrome::show_header(ui, &tokens, &mut view, &tx));
                        });
                    },
                );
                output.textures_delta.clear();
            }
            header.unwrap()
        };
        let wide = layout(1180.);
        assert!(wide.history_visible, "undo/redo show above 1040px");
        assert!(wide.canvas.right() < wide.controls.left());
        for width in [1040., 760., 560.] {
            let header = layout(width);
            assert!(!header.history_visible, "{width}px hides undo/redo");
            assert!(
                header.canvas.right() <= header.controls.left(),
                "{width}px: canvas toolbar {:?} overlaps controls {:?}",
                header.canvas,
                header.controls
            );
            // The controls stay right-aligned and inside the window.
            assert_eq!(
                width - header.controls.right(),
                1180. - wide.controls.right()
            );
            assert!(header.controls.left() >= 0.);
        }
        // Hiding undo/redo (two 34px buttons and their 4px gap) and narrowing
        // the zoom slider and preset shrinks the controls by a fixed amount.
        let narrow = layout(1040.);
        assert_eq!(
            wide.controls.width() - narrow.controls.width(),
            2. * 34. + 4. + (92. - 72.) + (76. - 72.)
        );
        // At 560px the toolbar has to yield: it compacts, then clips.
        assert!(layout(560.).canvas.width() < wide.canvas.width());
    }

    #[test]
    fn restored_draft_banner_and_layer_quick_actions_follow_shipping() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let tokens = crate::tokens::load().remove("light-mustard").unwrap();
        let (tx, rx) = mpsc::channel();
        let mut view = View::default();
        // A draft present at open is restored: shipping shows its banner.
        view.receive(&ctx, Ok(presented(false)));
        assert!(view.draft_restored);
        view.section = Section::Layers;
        let size = egui::vec2(1100., 700.);
        let frame = |view: &mut View, events| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                    events,
                    ..Default::default()
                },
                |ui| show(ui, &tokens, view, &tx),
            );
            output.textures_delta.clear();
            output
        };
        // Accessible names locate controls without assuming platform font widths.
        let find = |output: &egui::FullOutput, label: &str| {
            let update = output
                .platform_output
                .accesskit_update
                .as_ref()
                .expect("accesskit tree");
            update
                .nodes
                .iter()
                .find(|(_, node)| node.label() == Some(label))
                .and_then(|(_, node)| node.bounds())
                .map(|rect| {
                    egui::pos2(
                        ((rect.x0 + rect.x1) / 2.) as f32,
                        ((rect.y0 + rect.y1) / 2.) as f32,
                    )
                })
                .unwrap_or_else(|| panic!("missing control: {label}"))
        };
        let click = |view: &mut View, pos| {
            frame(view, vec![egui::Event::PointerMoved(pos)]);
            for pressed in [true, false] {
                frame(
                    view,
                    vec![egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    }],
                );
            }
        };
        frame(&mut view, vec![]);
        let output = frame(&mut view, vec![]);
        for label in [
            "Undo",
            "Redo",
            "Fit canvas",
            "Zoom out",
            "Zoom in",
            "Add images",
            "Canvas width",
            "Canvas height",
            "Trim edges",
            "Add image layer",
            "Layer settings for Original screenshot",
        ] {
            find(&output, label);
        }
        let dismiss = find(&output, "Dismiss restored-edits notice");
        click(&mut view, dismiss);
        assert!(
            !view.draft_restored && rx.try_recv().is_err(),
            "Dismiss only hides the notice"
        );

        // Row quick actions target their own layer; the eye does not select it.
        let output = frame(&mut view, vec![]);
        let hide = find(&output, "Hide Original screenshot");
        view.selected_layer = None;
        click(&mut view, hide);
        match rx.try_recv() {
            Ok(Job::Apply(Request::Layer {
                id,
                edit: LayerEdit::Visibility { visible: false },
            })) => {
                assert_eq!(id, "capture-background");
            }
            _ => panic!("the eye hides its row"),
        }
        assert!(view.selected_layer.is_none());
        view.pending = false;
        let output = frame(&mut view, vec![]);
        let unlock = find(&output, "Unlock Original screenshot");
        click(&mut view, unlock);
        match rx.try_recv() {
            Ok(Job::Apply(Request::Layer {
                id,
                edit: LayerEdit::Lock { locked: false },
            })) => {
                assert_eq!(id, "capture-background");
            }
            _ => panic!("the lock unlocks its row"),
        }
        assert_eq!(
            view.selected_layer.as_deref(),
            Some("capture-background"),
            "lock selects its row"
        );
        view.pending = false;

        // Drafts autosave, as in shipping: the header has no draft menu.
        let output = frame(&mut view, vec![]);
        let update = output.platform_output.accesskit_update.as_ref().unwrap();
        for label in ["Draft actions", "Save draft", "Discard edits…"] {
            assert!(
                !update
                    .nodes
                    .iter()
                    .any(|(_, node)| node.label() == Some(label)),
                "{label}"
            );
        }
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn rail_selects_tools_and_shape_menu_without_editing_or_leaking_busy_clicks() {
        let ctx = egui::Context::default();
        let tokens = crate::tokens::load().remove("light-mustard").unwrap();
        let mut view = View::default();
        view.receive(&ctx, Ok(presented(true)));
        let original = view.presented.as_ref().unwrap().document.clone();
        let frame = |view: &mut View, events| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(760., 540.),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    chrome::show_tool_rail(ui, &tokens, view);
                    egui::CentralPanel::default().show(ui, |ui| {
                        assert_eq!(ui.available_rect_before_wrap().left(), 64.);
                    });
                    if ctx.current_pass_index() == 0 {
                        ctx.request_discard("multipass");
                    }
                },
            );
            assert!(output.platform_output.num_completed_passes >= 2);
            output.textures_delta.clear();
            output
        };
        let click = |view: &mut View, pos| {
            frame(view, vec![egui::Event::PointerMoved(pos)]);
            for pressed in [true, false] {
                frame(
                    view,
                    vec![egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    }],
                );
            }
        };
        frame(&mut view, vec![]);
        // 8px top padding, 38px buttons and 2px gaps, as in shipping.
        let rail = |row: usize| egui::pos2(28., 27. + row as f32 * 40.);
        view.select_layer(Some("capture-background".into()));
        click(&mut view, rail(2));
        assert_eq!(
            (view.section, view.draw_shape),
            (Section::Draw, DrawShape::Text)
        );
        assert!(view.selected_layer.is_none(), "rail tools clear selection");
        view.select_layer(Some("capture-background".into()));
        click(&mut view, rail(2));
        assert!(
            view.selected_layer.is_none(),
            "reactivating Text also clears selection"
        );
        view.select_layer(Some("capture-background".into()));
        click(&mut view, rail(1));
        assert!(view.crop_previous.is_some());
        assert!(view.selected_layer.is_none(), "Crop clears selection");
        view.crop = [13., 21., 97., 53.];
        click(&mut view, rail(1));
        assert_eq!(view.crop, [13., 21., 97., 53.]);
        view.select_layer(Some("capture-background".into()));
        click(&mut view, rail(0));
        assert_eq!(view.section, Section::Layers);
        assert_eq!(
            view.selected_layer.as_deref(),
            Some("capture-background"),
            "Select retains selection"
        );
        assert!(view.crop_previous.is_none());
        click(&mut view, rail(3));
        assert!(view.selected_layer.is_none(), "Shapes clears selection");
        assert!(egui::Popup::is_any_open(&ctx));
        frame(&mut view, vec![]);
        // The flyout opens 10px right of Shapes, centred on it: a 3×2 grid of
        // 44px buttons with 4px gaps and 6px padding. Star is last.
        let star = egui::pos2(
            47. + 10. + 1. + 6. + 2. * 48. + 22.,
            147. - 53. + 7. + 48. + 22.,
        );
        click(&mut view, star);
        assert_eq!(view.draw_shape, DrawShape::Star);
        assert!(!egui::Popup::is_any_open(&ctx));
        for (row, shape) in [
            (4, DrawShape::Arrow),
            (5, DrawShape::Freehand),
            (6, DrawShape::Wand),
        ] {
            click(&mut view, rail(row));
            assert_eq!(view.draw_shape, shape);
        }
        click(&mut view, rail(3));
        assert_eq!(
            view.draw_shape,
            DrawShape::Star,
            "Shapes recalls the last grouped tool"
        );
        frame(
            &mut view,
            vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert!(!egui::Popup::is_any_open(&ctx));
        for blocked in 0..2 {
            view.pending = blocked == 0;
            view.close_requested = blocked == 1;
            click(&mut view, rail(4));
            assert_eq!(view.draw_shape, DrawShape::Star);
        }
        assert_eq!(view.presented.as_ref().unwrap().document, original);
    }

    #[test]
    fn tool_keys_preserve_document_repeats_focus_and_background_mode() {
        let ctx = egui::Context::default();
        let tokens = crate::tokens::load().remove("light-mustard").unwrap();
        let mut view = View::default();
        view.receive(&ctx, Ok(presented(true)));
        let document = view.presented.as_ref().unwrap().document.clone();
        let (tx, rx) = mpsc::channel();
        let key = |key, modifiers| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: true,
            modifiers,
        };
        let frame = |view: &mut View, events| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1000., 900.),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    show(ui, &tokens, view, &tx);
                    if ctx.current_pass_index() == 0 {
                        ctx.request_discard("multipass");
                    }
                },
            );
            assert!(output.platform_output.num_completed_passes >= 2);
            output.textures_delta.clear();
        };
        for (code, shape) in [
            (egui::Key::T, DrawShape::Text),
            (egui::Key::R, DrawShape::Rectangle),
            (egui::Key::O, DrawShape::Ellipse),
            (egui::Key::L, DrawShape::Line),
            (egui::Key::D, DrawShape::Diamond),
            (egui::Key::S, DrawShape::Star),
            (egui::Key::A, DrawShape::Arrow),
            (egui::Key::P, DrawShape::Freehand),
            (egui::Key::B, DrawShape::Wand),
        ] {
            view.select_layer(Some("capture-background".into()));
            frame(&mut view, vec![key(code, egui::Modifiers::SHIFT)]);
            assert_eq!((view.section, view.draw_shape), (Section::Draw, shape));
            assert!(view.selected_layer.is_none(), "{code:?} clears selection");
        }
        for shape in [DrawShape::Erase, DrawShape::Restore] {
            view.draw_shape = shape;
            frame(&mut view, vec![]); // The Draw controls remember the chosen background mode.
            frame(&mut view, vec![key(egui::Key::P, egui::Modifiers::NONE)]);
            frame(&mut view, vec![key(egui::Key::B, egui::Modifiers::NONE)]);
            assert_eq!(view.draw_shape, shape);
        }
        frame(&mut view, vec![key(egui::Key::R, egui::Modifiers::NONE)]);
        let gesture = Some((Point { x: 13., y: 21. }, Point { x: 97., y: 53. }));
        view.shape_drag = gesture;
        view.select_layer(Some("capture-background".into()));
        frame(&mut view, vec![key(egui::Key::R, egui::Modifiers::NONE)]);
        assert!(
            view.selected_layer.is_none(),
            "reactivating Rectangle clears selection"
        );
        assert_eq!(view.shape_drag, gesture);
        frame(&mut view, vec![key(egui::Key::C, egui::Modifiers::NONE)]);
        assert!(view.shape_drag.is_none());
        assert_eq!(view.section, Section::Geometry);
        let previous = view.crop;
        assert_eq!(view.crop_previous, Some(previous));
        view.crop = [13., 21., 97., 53.];
        frame(&mut view, vec![key(egui::Key::C, egui::Modifiers::NONE)]);
        assert_eq!(
            view.crop,
            [13., 21., 97., 53.],
            "repeat does not cancel the crop candidate"
        );
        view.select_layer(Some("capture-background".into()));
        frame(&mut view, vec![key(egui::Key::V, egui::Modifiers::NONE)]);
        assert_eq!(view.section, Section::Layers);
        assert_eq!(
            view.selected_layer.as_deref(),
            Some("capture-background"),
            "V retains selection"
        );
        assert_eq!(view.crop, previous);
        assert!(view.crop_previous.is_none());
        for modifiers in [
            egui::Modifiers::CTRL,
            egui::Modifiers::MAC_CMD,
            egui::Modifiers::ALT,
        ] {
            frame(&mut view, vec![key(egui::Key::P, modifiers)]);
            assert_eq!(view.section, Section::Layers);
        }
        for blocked in 0..3 {
            view.pending = blocked == 0;
            view.close_requested = blocked == 1;
            view.closed = blocked == 2;
            frame(&mut view, vec![key(egui::Key::P, egui::Modifiers::NONE)]);
            assert_eq!(view.section, Section::Layers);
        }
        view.closed = false;
        let mut output = ctx.run_ui(Default::default(), |ui| {
            ui.text_edit_singleline(&mut String::new()).request_focus();
        });
        output.textures_delta.clear();
        frame(&mut view, vec![key(egui::Key::P, egui::Modifiers::NONE)]);
        assert_eq!(view.section, Section::Layers, "typing keeps its keys");
        assert_eq!(
            view.selected_layer.as_deref(),
            Some("capture-background"),
            "blocked tool keys retain selection"
        );
        let mut output = ctx.run_ui(Default::default(), |ui| {
            ui.add(egui::Slider::new(&mut 50., 0.0..=100.0))
                .request_focus();
        });
        output.textures_delta.clear();
        frame(&mut view, vec![key(egui::Key::P, egui::Modifiers::NONE)]);
        assert_eq!(
            view.section,
            Section::Layers,
            "other focused controls keep their keys"
        );
        assert_eq!(view.presented.as_ref().unwrap().document, document);
        assert!(
            rx.try_recv().is_err(),
            "tool selection never submits document, draft or export work"
        );
    }

    #[test]
    fn tool_and_layer_shortcuts_in_one_frame_follow_input_order() {
        for key in [egui::Key::Delete, egui::Key::D, egui::Key::ArrowRight] {
            for tool_first in [true, false] {
                let ctx = egui::Context::default();
                let (mut view, id) = covered_canvas_view(&ctx);
                view.section = Section::Draw;
                view.draw_shape = DrawShape::Arrow;
                view.select_layer(Some(id.clone()));
                let (tx, rx) = mpsc::channel();
                let event = |key, modifiers| egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers,
                };
                let tool = event(egui::Key::A, egui::Modifiers::NONE);
                let action = event(
                    key,
                    if key == egui::Key::D {
                        egui::Modifiers::CTRL
                    } else {
                        egui::Modifiers::NONE
                    },
                );
                let events = if tool_first {
                    vec![tool, action]
                } else {
                    vec![action, tool]
                };
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        events,
                        ..Default::default()
                    },
                    |_| {
                        handle_document_shortcuts(&ctx, &mut view, &tx);
                        if ctx.current_pass_index() == 0 {
                            ctx.request_discard("ordered keys");
                        }
                    },
                );
                output.textures_delta.clear();
                if tool_first {
                    assert!(view.selected_layer.is_none());
                    assert!(
                        !view.pending && rx.try_recv().is_err(),
                        "{key:?} has no target after A"
                    );
                } else {
                    assert!(view.pending, "{key:?} must act before A");
                    let Ok(Job::Apply(request)) = rx.try_recv() else {
                        panic!("missing layer action")
                    };
                    match (key, request) {
                        (
                            egui::Key::Delete,
                            Request::Layer {
                                id: target,
                                edit: LayerEdit::Delete,
                            },
                        )
                        | (
                            egui::Key::D,
                            Request::Layer {
                                id: target,
                                edit: LayerEdit::Duplicate { .. },
                            },
                        ) => assert_eq!(target, id),
                        (
                            egui::Key::ArrowRight,
                            Request::Layer {
                                id: target,
                                edit: LayerEdit::Translate { delta_x, delta_y },
                            },
                        ) => {
                            assert_eq!(target, id);
                            assert_eq!((delta_x, delta_y), (1., 0.));
                        }
                        _ => panic!("wrong ordered layer action"),
                    }
                    assert!(
                        rx.try_recv().is_err(),
                        "multipass must not repeat an action"
                    );
                }
                assert_eq!(
                    (view.section, view.draw_shape),
                    (Section::Draw, DrawShape::Arrow)
                );
            }
        }
    }

    #[test]
    fn export_bar_spans_every_section_and_preserves_job_gates() {
        let ctx = egui::Context::default();
        let tokens = crate::tokens::load().remove("light-mustard").unwrap();
        let mut view = View {
            artifact_id: "shot".into(),
            default_directory: "/exports".into(),
            default_stem: "edited".into(),
            ..View::default()
        };
        view.receive(&ctx, Ok(presented(true)));
        view.export_options.size = ExportSize::Custom {
            width: 13,
            height: 7,
        };
        view.custom_export_size = [13, 7];
        let (tx, rx) = mpsc::channel();
        let frame = |view: &mut View, size, events| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                    events,
                    ..Default::default()
                },
                |ui| show(ui, &tokens, view, &tx),
            );
            output.textures_delta.clear();
            output
        };
        let find = |output: &egui::FullOutput, label: &str| {
            output.shapes.iter().find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.job.text == label => {
                    Some(text.pos + text.galley.rect.center().to_vec2())
                }
                _ => None,
            })
        };
        let position = |output: &egui::FullOutput, label: &str| {
            find(output, label).unwrap_or_else(|| panic!("missing action {label}"))
        };
        let click = |view: &mut View, size, pos| {
            frame(view, size, vec![egui::Event::PointerMoved(pos)]);
            for pressed in [true, false] {
                frame(
                    view,
                    size,
                    vec![egui::Event::PointerButton {
                        pos,
                        pressed,
                        button: egui::PointerButton::Primary,
                        modifiers: egui::Modifiers::NONE,
                    }],
                );
            }
        };
        for size in [egui::vec2(1000., 900.), egui::vec2(760., 540.)] {
            for section in [Section::Geometry, Section::Layers, Section::Draw] {
                view.section = section;
                frame(&mut view, size, vec![]);
                let output = frame(&mut view, size, vec![]);
                let copy = position(&output, "Copy image");
                let save = position(&output, "Save");
                assert!(
                    find(&output, "Save as new file").is_none(),
                    "a first save is always a new file"
                );
                for pos in [copy, save] {
                    assert!(pos.x > 0. && pos.x < size.x);
                    assert!(pos.y > size.y - 80. && pos.y < size.y);
                }
                assert!(save.x > size.x - 120., "Save is the rightmost action");
                click(&mut view, size, save);
                let Job::Save { plan, options } = rx.try_recv().unwrap() else {
                    panic!()
                };
                assert_eq!(
                    plan,
                    SavePlan::NewFile {
                        path: PathBuf::from("/exports/edited.png")
                    }
                );
                assert_eq!(
                    options.size,
                    ExportSize::Custom {
                        width: 13,
                        height: 7
                    }
                );
                assert!(view.export_job);
                click(&mut view, size, copy);
                assert!(
                    rx.try_recv().is_err(),
                    "copy cannot queue behind an accepted save"
                );
                view.pending = false;
                view.export_job = false;
                click(&mut view, size, copy);
                assert!(matches!(rx.try_recv(), Ok(Job::Copy)));
                view.pending = false;
                view.export_job = false;
            }
        }
        let size = egui::vec2(760., 540.);
        let output = frame(&mut view, size, vec![]);
        let save = position(&output, "Save");
        let copy = position(&output, "Copy image");
        view.export_options.size = ExportSize::Custom {
            width: 0,
            height: 7,
        };
        view.custom_export_size = [0, 7];
        click(&mut view, size, save);
        assert!(rx.try_recv().is_err(), "invalid dimensions disable save");
        let output = frame(&mut view, size, vec![]);
        assert!(
            find(
                &output,
                "Output dimensions must be from 1 through 16,384 pixels."
            )
            .is_some()
        );
        click(&mut view, size, copy);
        assert!(
            matches!(rx.try_recv(), Ok(Job::Copy)),
            "copy ignores invalid export dimensions"
        );
        view.pending = false;
        view.close_requested = true;
        click(&mut view, size, copy);
        click(&mut view, size, save);
        assert!(
            rx.try_recv().is_err(),
            "a closing editor blocks both actions"
        );

        view.close_requested = false;
        view.section = Section::Geometry;
        view.export_options.size = ExportSize::Custom {
            width: 13,
            height: 7,
        };
        view.custom_export_size = [13, 7];
        let notice = "Saved /exports/a-long-filename-for-the-edited-image.png. History was not updated: the destination is unavailable.";
        view.output_notice = Some(notice.into());
        let output = frame(&mut view, size, vec![]);
        let pos = position(&output, notice);
        // Let egui's hover delay elapse. The elided label already supplies a
        // full-message tooltip; an extra on_hover_text would paint it twice.
        frame(&mut view, size, vec![egui::Event::PointerMoved(pos)]);
        for _ in 0..90 {
            frame(&mut view, size, vec![]);
        }
        let output = frame(&mut view, size, vec![]);
        let messages = output.shapes.iter().filter(|shape|
            matches!(&shape.shape, egui::Shape::Text(text) if text.galley.job.text == notice)
        ).count();
        assert_eq!(
            messages, 2,
            "one status label and one complete hover tooltip"
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn export_bar_overwrites_saved_source_by_default_and_switch_saves_a_copy() {
        let ctx = egui::Context::default();
        let tokens = crate::tokens::load().remove("dark-mustard").unwrap();
        let data = tempfile::tempdir().unwrap();
        let source = data.path().join("Shot.png");
        fs::write(&source, b"png").unwrap();
        let mut view = View {
            artifact_id: "shot".into(),
            default_directory: "/exports".into(),
            default_stem: "unused".into(),
            ..View::default()
        };
        let mut current = presented(false);
        current.original_export_path = Some(source.clone());
        view.receive(&ctx, Ok(current));
        assert_eq!(view.filename, "Shot");
        let (tx, rx) = mpsc::channel();
        let size = egui::vec2(1000., 700.);
        let frame = |view: &mut View, events| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                    events,
                    ..Default::default()
                },
                |ui| show(ui, &tokens, view, &tx),
            );
            output.textures_delta.clear();
            output
        };
        let position = |output: &egui::FullOutput, label: &str| {
            output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.job.text == label => {
                        Some(text.pos + text.galley.rect.center().to_vec2())
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("missing {label}"))
        };
        let click = |view: &mut View, pos| {
            frame(view, vec![egui::Event::PointerMoved(pos)]);
            for pressed in [true, false] {
                frame(
                    view,
                    vec![egui::Event::PointerButton {
                        pos,
                        pressed,
                        button: egui::PointerButton::Primary,
                        modifiers: egui::Modifiers::NONE,
                    }],
                );
            }
            frame(view, vec![])
        };
        frame(&mut view, vec![]);
        let output = frame(&mut view, vec![]);
        position(
            &output,
            "Save keeps original quality as PNG and overwrites the original.",
        );
        click(&mut view, position(&output, "Save"));
        let Job::Save { plan, .. } = rx.try_recv().unwrap() else {
            panic!()
        };
        assert_eq!(
            plan,
            SavePlan::Overwrite {
                artifact_id: "shot".into(),
                path: source.clone()
            }
        );
        view.pending = false;
        view.export_job = false;

        let output = click(&mut view, position(&output, "Save as new file"));
        assert_eq!(view.filename, "Shot-edited");
        position(
            &output,
            "Save writes a new PNG at original quality and leaves the original untouched.",
        );
        click(&mut view, position(&output, "Save"));
        let Job::Save { plan, .. } = rx.try_recv().unwrap() else {
            panic!()
        };
        assert_eq!(
            plan,
            SavePlan::NewFile {
                path: data.path().join("Shot-edited.png")
            }
        );
        view.pending = false;
        view.export_job = false;

        // A format that differs from the source always saves a copy and hides the switch.
        click(&mut view, position(&output, "Save as new file"));
        assert_eq!(view.filename, "Shot");
        view.export_options.format = ExportFormat::Webp;
        frame(&mut view, vec![]);
        let output = frame(&mut view, vec![]);
        assert!(!output.shapes.iter().any(|shape| matches!(&shape.shape,
            egui::Shape::Text(text) if text.galley.job.text == "Save as new file")));
        position(&output, ".webp");
        click(&mut view, position(&output, "Save"));
        let Job::Save { plan, .. } = rx.try_recv().unwrap() else {
            panic!()
        };
        assert_eq!(
            plan,
            SavePlan::NewFile {
                path: data.path().join("Shot.webp")
            }
        );
    }

    #[test]
    fn output_presets_set_exact_quality_clear_png_override_and_invalidate_preview() {
        let ctx = egui::Context::default();
        let mut view = View::default();
        view.receive(&ctx, Ok(presented(false)));
        view.export_options.quality = ExportQuality::Compress;
        let original = view.presented.as_ref().unwrap().document.clone();
        let tokens = crate::tokens::load().remove("light-mustard").unwrap();
        let (_tx, rx) = mpsc::channel::<Job>();
        let frame = |view: &mut View, events| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(250., 900.),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| show_export_settings(ui, &tokens, view),
            );
            output.textures_delta.clear();
            output
        };
        let position = |output: &egui::FullOutput, label: &str| {
            output
                .shapes
                .iter()
                // The preset's Custom label follows the Output size Custom button.
                .rev()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.job.text == label => {
                        Some(text.pos + text.galley.rect.center().to_vec2())
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("missing UI label {label}"))
        };
        let click = |view: &mut View, pos| {
            frame(view, vec![egui::Event::PointerMoved(pos)]);
            for pressed in [true, false] {
                frame(
                    view,
                    vec![egui::Event::PointerButton {
                        pos,
                        pressed,
                        button: egui::PointerButton::Primary,
                        modifiers: egui::Modifiers::NONE,
                    }],
                );
            }
            frame(view, vec![])
        };
        for (label, expected) in [("Tiny", 55), ("Highest", 98)] {
            view.export_options.png.max_colors = Some(17);
            view.output = Some((view.texture.as_ref().unwrap().clone(), 123));
            let output = frame(&mut view, vec![]);
            let popup = click(&mut view, position(&output, "Custom"));
            click(&mut view, position(&popup, label));
            assert_eq!(view.export_options.quality_value, expected);
            assert_eq!(view.export_options.png.max_colors, None);
            assert_eq!(output_preset(&view.export_options), Some(label));
            assert!(view.output.is_none());
            assert_eq!(view.presented.as_ref().unwrap().document, original);
            assert!(
                !view.pending && rx.try_recv().is_err(),
                "a preset never encodes or edits"
            );
        }
    }

    #[test]
    fn comparison_controls_own_gestures_before_the_select_canvas() {
        for appearance in ["light-mustard", "dark-mustard"] {
            let ctx = egui::Context::default();
            ctx.enable_accesskit();
            let tokens = crate::tokens::load().remove(appearance).unwrap();
            let mut view = View::default();
            let mut value = presented(false);
            value.document = Arc::new(Document::new_capture("compare", 640., 360., None));
            value.pixels = Arc::new(RgbaImage::new(640, 360));
            view.receive(&ctx, Ok(value));
            let original = view.presented.as_ref().unwrap().document.clone();
            view.export_settings_open = true;
            view.export_options.quality = ExportQuality::Compress;
            view.output = Some((view.texture.as_ref().unwrap().clone(), 500));
            view.compare_pending = false;
            view.compare_key = Some((view.pixels_revision, view.export_options));
            let (tx, rx) = mpsc::channel();
            let frame = |view: &mut View, events| {
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(1000., 1000.),
                        )),
                        events,
                        ..Default::default()
                    },
                    |ui| show(ui, &tokens, view, &tx),
                );
                output.textures_delta.clear();
                output
            };
            let control = |output: &egui::FullOutput, label: &str| {
                let rect = output
                    .platform_output
                    .accesskit_update
                    .as_ref()
                    .unwrap()
                    .nodes
                    .iter()
                    .find(|(_, node)| node.label() == Some(label))
                    .and_then(|(_, node)| node.bounds())
                    .unwrap_or_else(|| panic!("missing comparison control: {label}"));
                egui::pos2(
                    ((rect.x0 + rect.x1) / 2.) as f32,
                    ((rect.y0 + rect.y1) / 2.) as f32,
                )
            };
            let press = |pos, pressed| egui::Event::PointerButton {
                pos,
                pressed,
                button: egui::PointerButton::Primary,
                modifiers: egui::Modifiers::NONE,
            };
            frame(&mut view, vec![]);
            let output = frame(&mut view, vec![]);
            let handle = control(&output, compare::HANDLE_LABEL);
            let preview = fitted_image_rect(view.viewport_area.unwrap(), egui::vec2(640., 360.));
            let end = egui::pos2(preview.left() + preview.width() * 0.25, handle.y);
            frame(&mut view, vec![egui::Event::PointerMoved(handle)]);
            frame(&mut view, vec![press(handle, true)]);
            assert!(
                view.layer_gesture.is_none(),
                "{appearance}: split press must not select"
            );
            assert!(!view.compare_suppressed());
            frame(&mut view, vec![egui::Event::PointerMoved(end)]);
            frame(&mut view, vec![press(end, false)]);
            assert!((view.compare_split - 0.25).abs() < 1e-6);

            // A complete click in one frame also belongs only to the overlay.
            let output = frame(&mut view, vec![]);
            let range = control(&output, compare::RANGE_LABEL);
            frame(&mut view, vec![egui::Event::PointerMoved(range)]);
            frame(&mut view, vec![press(range, true), press(range, false)]);
            assert!((view.compare_split - 0.5).abs() < 1e-6);
            let output = frame(&mut view, vec![]);
            let dismiss = control(&output, compare::DISMISS_LABEL);
            frame(&mut view, vec![egui::Event::PointerMoved(dismiss)]);
            frame(&mut view, vec![press(dismiss, true), press(dismiss, false)]);
            assert!(view.compare_dismissed);
            assert!(view.selected_layer.is_none() && view.layer_gesture.is_none());
            assert_eq!(view.presented.as_ref().unwrap().document, original);
            assert!(rx.try_recv().is_err() && !view.pending);

            // The ordinary canvas still starts a Select gesture away from chrome.
            view.compare_dismissed = false;
            frame(&mut view, vec![]);
            let canvas = preview.min + egui::vec2(30., 40.);
            frame(&mut view, vec![egui::Event::PointerMoved(canvas)]);
            frame(&mut view, vec![press(canvas, true)]);
            assert!(view.layer_gesture.is_some() && view.compare_suppressed());
        }
    }

    #[test]
    fn comparison_hide_show_and_quality_modes_follow_the_shipping_export_bar() {
        let ctx = egui::Context::default();
        let mut view = View::default();
        view.receive(&ctx, Ok(presented(false)));
        view.export_settings_open = true;
        let tokens = crate::tokens::load().remove("light-mustard").unwrap();
        let frame = |view: &mut View, events| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(900., 400.),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| show_export_settings(ui, &tokens, view),
            );
            output.textures_delta.clear();
            output
        };
        let find = |output: &egui::FullOutput, label: &str| {
            output
                .shapes
                .iter()
                .rev()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.job.text == label => {
                        Some(text.pos + text.galley.rect.center().to_vec2())
                    }
                    _ => None,
                })
        };
        let click = |view: &mut View, pos| {
            frame(view, vec![egui::Event::PointerMoved(pos)]);
            for pressed in [true, false] {
                frame(
                    view,
                    vec![egui::Event::PointerButton {
                        pos,
                        pressed,
                        button: egui::PointerButton::Primary,
                        modifiers: egui::Modifiers::NONE,
                    }],
                );
            }
            frame(view, vec![])
        };
        let output = frame(&mut view, vec![]);
        assert!(!view.compare_visible() && find(&output, compare::SHOW).is_none());
        // Save quality lists the per-format shipping descriptions.
        let popup = click(&mut view, find(&output, "Preserve quality").unwrap());
        assert!(
            find(
                &popup,
                "Smaller PNG with Tiny through Highest quality presets."
            )
            .is_some()
        );
        let output = click(&mut view, find(&popup, "Compress").unwrap());
        assert_eq!(view.export_options.quality, ExportQuality::Compress);
        assert!(view.compare_visible());
        let popup = click(&mut view, find(&output, "Custom").unwrap());
        assert!(find(&popup, "Smallest PNG with the most visible dithering.").is_some());
        run_escape(&ctx, &mut view, &frame);

        view.compare_dismissed = true;
        view.compare_split = 0.3;
        assert!(!view.compare_visible());
        let output = frame(&mut view, vec![]);
        assert!(find(&output, compare::SHOW_CAPTION).is_some());
        click(&mut view, find(&output, compare::SHOW).unwrap());
        assert!(view.compare_visible());

        // A new quality mode shows it again; Preserve also recentres it.
        view.compare_dismissed = true;
        let output = frame(&mut view, vec![]);
        let popup = click(&mut view, find(&output, "Compress").unwrap());
        let output = click(&mut view, find(&popup, "Maximum file size").unwrap());
        assert!(!view.compare_dismissed && view.compare_visible());
        assert_eq!(view.export_options.max_size_bytes, Some(10_000_000));
        assert!(find(&output, "MB").is_some() && find(&output, "10").is_some());
        let popup = click(&mut view, find(&output, "MB").unwrap());
        click(&mut view, find(&popup, "KB").unwrap());
        assert_eq!(
            (view.max_size_text.as_str(), view.max_size_unit),
            ("10000", FileSizeUnit::Kb)
        );
        assert_eq!(view.export_options.max_size_bytes, Some(10_000_000));
        view.max_size_text = "9.5".into();
        frame(&mut view, vec![]);
        assert_eq!(view.export_options.max_size_bytes, Some(9_500));
        assert!(view.export_view().unwrap().error.unwrap().contains("10 KB"));
        view.max_size_text = "ten".into();
        frame(&mut view, vec![]);
        assert_eq!(view.export_options.max_size_bytes, Some(0));
        let output = frame(&mut view, vec![]);
        // The Save quality trigger, not the Maximum file size caption after it.
        let trigger = output.shapes.iter().find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.job.text == "Maximum file size" => {
                Some(text.pos + text.galley.rect.center().to_vec2())
            }
            _ => None,
        });
        let popup = click(&mut view, trigger.unwrap());
        click(&mut view, find(&popup, "Preserve quality").unwrap());
        assert_eq!(view.export_options.max_size_bytes, None);
        assert_eq!(view.compare_split, compare::DEFAULT_SPLIT);
        let output = frame(&mut view, vec![]);
        for removed in ["PNG colors", "Canvas", "Encoded", "Edited"] {
            assert!(find(&output, removed).is_none(), "{removed}");
        }
    }

    fn run_escape(
        ctx: &egui::Context,
        view: &mut View,
        frame: &dyn Fn(&mut View, Vec<egui::Event>) -> egui::FullOutput,
    ) {
        let _ = ctx;
        frame(
            view,
            vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        frame(view, vec![]);
    }

    #[test]
    fn output_size_controls_use_document_dimensions_without_encoding_or_editing() {
        let ctx = egui::Context::default();
        let mut view = View::default();
        view.receive(&ctx, Ok(presented(false)));
        let document = view.presented.as_ref().unwrap().document.clone();
        let tokens = crate::tokens::load().remove("light-mustard").unwrap();
        let (_tx, rx) = mpsc::channel::<Job>();
        let frame = |view: &mut View, events| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(250., 900.),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| show_export_settings(ui, &tokens, view),
            );
            output.textures_delta.clear();
            output
        };
        let position = |output: &egui::FullOutput, label: &str| {
            output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.job.text == label => {
                        Some(text.pos + text.galley.rect.center().to_vec2())
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("missing UI label {label}"))
        };
        let click = |view: &mut View, pos| {
            frame(view, vec![egui::Event::PointerMoved(pos)]);
            for pressed in [true, false] {
                frame(
                    view,
                    vec![egui::Event::PointerButton {
                        pos,
                        pressed,
                        button: egui::PointerButton::Primary,
                        modifiers: egui::Modifiers::NONE,
                    }],
                );
            }
            frame(view, vec![])
        };

        view.output = Some((view.texture.as_ref().unwrap().clone(), 123));
        let output = frame(&mut view, vec![]);
        let popup = click(&mut view, position(&output, "Original"));
        click(&mut view, position(&popup, "75%"));
        assert_eq!(
            view.export_options.size,
            ExportSize::Percent { percent: 75 }
        );
        assert_eq!(view.export_options.size.dimensions(7, 3), Ok((5, 2)));
        assert!(view.output.is_none());

        let output = frame(&mut view, vec![]);
        let popup = click(&mut view, position(&output, "75%"));
        click(&mut view, position(&popup, "Custom"));
        assert_eq!(view.custom_export_size, [7, 3]);
        assert_eq!(
            view.export_options.size,
            ExportSize::Custom {
                width: 7,
                height: 3
            }
        );
        assert!(view.export_aspect_locked);
        assert!(rx.try_recv().is_err() && !view.pending);
        assert!(Arc::ptr_eq(
            &document,
            &view.presented.as_ref().unwrap().document
        ));
    }

    #[test]
    fn output_preset_label_keeps_arbitrary_quality_and_custom_png_options_custom() {
        let mut options = View::default().export_options;
        options.quality = ExportQuality::Compress;
        options.quality_value = 83;
        let unchanged = options;
        assert_eq!(output_preset(&options), None);
        assert_eq!(
            options, unchanged,
            "deriving a label must not normalize quality"
        );

        options.quality_value = 85;
        options.png.max_colors = Some(37);
        let unchanged = options;
        assert_eq!(output_preset(&options), None);
        assert_eq!(
            options, unchanged,
            "a custom PNG palette must remain an explicit override"
        );

        options.format = ExportFormat::Jpeg;
        assert_eq!(output_preset(&options), Some("Balanced"));
        assert_eq!(options.png.max_colors, Some(37));
    }

    #[test]
    fn fit_centers_without_upscaling_and_preserves_the_tauri_floor() {
        let area = egui::Rect::from_min_size(egui::pos2(31., 47.), egui::vec2(400., 300.));
        for (image, expected, origin) in [
            (
                egui::vec2(160., 90.),
                egui::vec2(160., 90.),
                egui::pos2(151., 152.),
            ),
            (
                egui::vec2(400., 300.),
                egui::vec2(400., 300.),
                egui::pos2(31., 47.),
            ),
            (
                egui::vec2(800., 200.),
                egui::vec2(400., 100.),
                egui::pos2(31., 147.),
            ),
            (
                egui::vec2(200., 1200.),
                egui::vec2(50., 300.),
                egui::pos2(206., 47.),
            ),
            (
                egui::vec2(40000., 20000.),
                egui::vec2(800., 400.),
                egui::pos2(-169., -3.),
            ),
            (
                egui::vec2(161., 91.),
                egui::vec2(161., 91.),
                egui::pos2(150.5, 151.5),
            ),
        ] {
            let fit = fitted_image_rect(area, image);
            assert_eq!(fit.min, origin);
            assert_eq!(fit.center(), area.center());
            assert_eq!(fit.size(), expected);
            assert_eq!(viewport_rect(Viewport::default(), fit, image), Some(fit));
        }
    }

    #[test]
    fn slider_tracks_actual_fit_and_cancels_gestures_without_changing_output() {
        let ctx = egui::Context::default();
        let mut view = View::default();
        assert_eq!(displayed_zoom(&view), None);
        view.receive(&ctx, Ok(presented(false)));
        let document = view.presented.as_ref().unwrap().document.clone();
        view.output = Some((view.texture.as_ref().unwrap().clone(), 123));
        view.viewport_area = Some(egui::Rect::from_min_size(
            egui::pos2(31., 47.),
            egui::vec2(400., 200.),
        ));
        view.viewport_image_size = Some(egui::vec2(800., 400.));
        assert_eq!(displayed_zoom(&view), Some(50.));
        view.shape_drag = Some((Point { x: 3., y: 7. }, Point { x: 20., y: 30. }));
        set_viewport_zoom(&mut view, zoom_from_slider(0.75).unwrap(), None);
        assert_eq!(displayed_zoom(&view), Some(224.9));
        assert!(view.shape_drag.is_none());
        assert!(view.output.is_some() && !view.pending);
        assert!(Arc::ptr_eq(
            &document,
            &view.presented.as_ref().unwrap().document
        ));
        view.reset_viewport();
        assert_eq!(displayed_zoom(&view), Some(50.));
    }

    #[test]
    fn layer_clipboard_shortcuts_keep_output_focus_and_transactional_selection() {
        let ctx = egui::Context::default();
        let mut view = View::default();
        view.receive(&ctx, Ok(presented(false)));
        view.select_layer_exact(Some("capture-background".into()));
        view.section = Section::Draw;
        view.output = Some((view.texture.as_ref().unwrap().clone(), 123));
        view.last_solid_background = "#123456".into();
        let original_id = view.selected_layer.clone().unwrap();
        let (tx, rx) = mpsc::channel();
        let frame = |view: &mut View, events| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |_| {
                    handle_document_shortcuts(&ctx, view, &tx);
                    if ctx.current_pass_index() == 0 {
                        ctx.request_discard("clipboard multipass");
                    }
                },
            );
            output.textures_delta.clear();
        };
        frame(
            &mut view,
            vec![egui::Event::Paste("unrelated OS text".into())],
        );
        assert!(
            rx.try_recv().is_err(),
            "empty internal clipboard is not an image/text import"
        );
        frame(&mut view, vec![egui::Event::Copy, egui::Event::Copy]);
        assert!(
            matches!(rx.try_recv(), Ok(Job::Apply(Request::CopyLayer { id })) if id == original_id)
        );
        assert!(rx.try_recv().is_err() && view.output.is_some());
        let mut copied = presented(false);
        copied.pixels = view.presented.as_ref().unwrap().pixels.clone();
        copied.copied_layer = true;
        copied.can_paste_layer = true;
        view.receive(&ctx, Ok(copied));
        assert_eq!(view.section, Section::Draw);
        assert_eq!(view.selected_layer.as_ref(), Some(&original_id));
        assert!(view.output.is_some());
        assert_eq!(
            view.last_solid_background, "#123456",
            "copy preserves remembered fields"
        );
        frame(
            &mut view,
            vec![
                egui::Event::Paste(String::new()),
                egui::Event::Key {
                    key: egui::Key::V,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::CTRL,
                },
            ],
        );
        let Ok(Job::Apply(Request::PasteLayer { new_id, after_id })) = rx.try_recv() else {
            panic!()
        };
        assert_eq!(after_id, Some(original_id.clone()));
        assert_ne!(new_id, original_id);
        assert!(
            rx.try_recv().is_err(),
            "semantic and native key-down paste enqueue once"
        );
        frame(&mut view, vec![egui::Event::Copy]);
        assert!(
            rx.try_recv().is_err(),
            "busy copy cannot replace the snapshot"
        );
        view.receive(&ctx, Err("paste failed".into()));
        assert_eq!(view.section, Section::Draw);
        assert_eq!(view.selected_layer.as_ref(), Some(&original_id));
        frame(&mut view, vec![egui::Event::Paste(String::new())]);
        let Ok(Job::Apply(Request::PasteLayer { new_id, .. })) = rx.try_recv() else {
            panic!()
        };
        let mut pasted = presented(true);
        let mut layer = pasted.document.elements[0].clone();
        let Element::Image(image) = &mut layer else {
            panic!()
        };
        image.base.id = new_id.clone();
        Arc::make_mut(&mut pasted.document).elements.push(layer);
        pasted.pasted_layer = true;
        pasted.can_paste_layer = true;
        view.receive(&ctx, Ok(pasted));
        assert_eq!(view.section, Section::Layers);
        assert_eq!(view.selected_layer.as_ref(), Some(&new_id));
        assert!(view.output.is_none());
        let mut output = ctx.run_ui(Default::default(), |ui| {
            ui.text_edit_singleline(&mut "field".to_owned())
                .request_focus();
        });
        output.textures_delta.clear();
        frame(
            &mut view,
            vec![egui::Event::Copy, egui::Event::Paste("text".into())],
        );
        assert!(
            rx.try_recv().is_err(),
            "focused text keeps its own clipboard"
        );
    }

    #[test]
    fn layer_shortcuts_preserve_typing_locks_selection_and_one_in_flight_work() {
        let ctx = egui::Context::default();
        let mut view = View::default();
        let mut initial = presented(true);
        let Element::Image(image) = &mut Arc::make_mut(&mut initial.document).elements[0] else {
            panic!()
        };
        image.base.locked = true;
        image.base.visible = false;
        let original_id = image.base.id.clone();
        view.receive(&ctx, Ok(initial));
        view.select_layer_exact(Some(original_id.clone()));
        let (tx, rx) = mpsc::channel();
        let key = |key, ctrl| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: true,
            modifiers: egui::Modifiers {
                ctrl,
                ..Default::default()
            },
        };
        let frame = |view: &mut View, events, text_focus| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ui| {
                    handle_document_shortcuts(&ctx, view, &tx);
                    if text_focus {
                        ui.push_id("layer-typing", |ui| {
                            ui.text_edit_singleline(&mut "typed text".to_owned())
                                .request_focus();
                        });
                    } else {
                        if let Some(focused) = ctx.memory(|memory| memory.focused()) {
                            ctx.memory_mut(|memory| memory.surrender_focus(focused));
                        }
                    }
                    if ctx.current_pass_index() == 0 {
                        ctx.request_discard("layer shortcut multipass");
                    }
                },
            );
            output.textures_delta.clear();
        };
        frame(&mut view, vec![], true);
        frame(
            &mut view,
            vec![
                key(egui::Key::D, true),
                key(egui::Key::Delete, false),
                key(egui::Key::ArrowLeft, false),
            ],
            true,
        );
        assert!(rx.try_recv().is_err(), "typing must not edit a layer");
        frame(&mut view, vec![], false);
        frame(&mut view, vec![key(egui::Key::D, false)], false);
        assert_eq!(
            view.draw_shape,
            DrawShape::Diamond,
            "plain D selects a tool"
        );
        assert!(view.selected_layer.is_none() && rx.try_recv().is_err());
        view.select_layer_exact(Some(original_id.clone()));
        frame(
            &mut view,
            vec![
                key(egui::Key::Delete, false),
                key(egui::Key::Backspace, false),
                key(egui::Key::ArrowUp, false),
            ],
            false,
        );
        assert!(
            rx.try_recv().is_err(),
            "locked deletion and movement do nothing"
        );
        frame(
            &mut view,
            vec![
                egui::Event::PointerButton {
                    pos: egui::pos2(5., 5.),
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                },
                key(egui::Key::D, true),
            ],
            false,
        );
        assert!(
            rx.try_recv().is_err(),
            "focus-changing clicks must be processed before document keys"
        );
        frame(
            &mut view,
            vec![egui::Event::PointerButton {
                pos: egui::pos2(5., 5.),
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            }],
            false,
        );
        view.close_requested = true;
        frame(&mut view, vec![key(egui::Key::D, true)], false);
        assert!(rx.try_recv().is_err());
        view.close_requested = false;
        view.shape_drag = Some((Point { x: 3., y: 7. }, Point { x: 20., y: 30. }));
        frame(
            &mut view,
            vec![key(egui::Key::D, true), key(egui::Key::D, true)],
            false,
        );
        let Job::Apply(Request::Layer {
            id,
            edit: LayerEdit::Duplicate { new_id },
        }) = rx.try_recv().unwrap()
        else {
            panic!()
        };
        assert_eq!(id, original_id);
        assert_ne!(new_id, original_id);
        assert!(uuid::Uuid::parse_str(&new_id).is_ok());
        assert!(view.shape_drag.is_none() && rx.try_recv().is_err());
        assert_eq!(
            view.selected_layer.as_deref(),
            Some(original_id.as_str()),
            "selection waits for accepted copy"
        );
        frame(&mut view, vec![key(egui::Key::Delete, false)], false);
        assert!(rx.try_recv().is_err(), "busy shortcuts must not queue");
        view.receive(&ctx, Err("render rejected".into()));
        assert_eq!(view.selected_layer.as_deref(), Some(original_id.as_str()));
        assert!(view.pending_layer_selection.is_none());
        frame(&mut view, vec![key(egui::Key::D, true)], false);
        let Job::Apply(Request::Layer {
            edit: LayerEdit::Duplicate {
                new_id: accepted_id,
            },
            ..
        }) = rx.try_recv().unwrap()
        else {
            panic!()
        };
        assert_ne!(accepted_id, new_id);
        let mut accepted = presented(true);
        let mut copy = accepted.document.elements[0].clone();
        let Element::Image(image) = &mut copy else {
            panic!()
        };
        image.base.id = accepted_id.clone();
        image.base.locked = false;
        image.base.visible = false;
        Arc::make_mut(&mut accepted.document).elements.push(copy);
        view.receive(&ctx, Ok(accepted));
        assert_eq!(view.selected_layer.as_deref(), Some(accepted_id.as_str()));
        let mut output = ctx.run_ui(Default::default(), |ui| {
            ui.add(egui::Slider::new(&mut 50., 0.0..=100.0))
                .request_focus();
        });
        output.textures_delta.clear();
        frame(&mut view, vec![key(egui::Key::ArrowRight, false)], false);
        assert!(
            rx.try_recv().is_err(),
            "a focused slider owns arrow navigation"
        );
        for (arrow, shift, expected) in [
            (egui::Key::ArrowLeft, false, (-1., 0.)),
            (egui::Key::ArrowRight, true, (10., 0.)),
            (egui::Key::ArrowUp, true, (0., -10.)),
            (egui::Key::ArrowDown, false, (0., 1.)),
        ] {
            let mut event = key(arrow, false);
            if let egui::Event::Key { modifiers, .. } = &mut event {
                modifiers.shift = shift;
            }
            view.shape_drag = Some((Point { x: 3., y: 7. }, Point { x: 20., y: 30. }));
            frame(&mut view, vec![event.clone(), event], false);
            let Job::Apply(Request::Layer {
                id,
                edit: LayerEdit::Translate { delta_x, delta_y },
            }) = rx.try_recv().unwrap()
            else {
                panic!()
            };
            assert_eq!(id, accepted_id);
            assert_eq!((delta_x, delta_y), expected);
            assert!(
                view.shape_drag.is_none() && rx.try_recv().is_err(),
                "one job across repeat events and layout passes"
            );
            view.receive(&ctx, Err("move rejected".into()));
            assert_eq!(view.selected_layer.as_deref(), Some(accepted_id.as_str()));
        }
        frame(&mut view, vec![key(egui::Key::Backspace, false)], false);
        assert!(
            matches!(rx.try_recv(), Ok(Job::Apply(Request::Layer { id, edit: LayerEdit::Delete })) if id == accepted_id)
        );
        view.receive(&ctx, Ok(presented(true)));
        view.select_layer_exact(None);
        frame(
            &mut view,
            vec![
                key(egui::Key::D, true),
                key(egui::Key::Delete, false),
                key(egui::Key::ArrowDown, false),
            ],
            false,
        );
        assert!(
            rx.try_recv().is_err(),
            "no selection must not target a fallback layer"
        );
    }

    #[test]
    fn history_shortcuts_respect_text_focus_confirmation_and_one_in_flight_command() {
        let ctx = egui::Context::default();
        let mut view = View::default();
        view.receive(&ctx, Ok(presented(true)));
        let (tx, rx) = mpsc::channel();
        let key = |shift| egui::Event::Key {
            key: egui::Key::Z,
            physical_key: None,
            pressed: true,
            repeat: true,
            modifiers: egui::Modifiers {
                ctrl: true,
                shift,
                ..Default::default()
            },
        };
        let frame = |view: &mut View, events, text_focus| {
            let mut remaining = 0;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ui| {
                    handle_document_shortcuts(&ctx, view, &tx);
                    if ctx.current_pass_index() == 0 {
                        remaining = ctx.input(|input| input.events.len());
                    }
                    if text_focus {
                        ui.push_id("typing-field", |ui| {
                            ui.text_edit_singleline(&mut "typing".to_owned())
                                .request_focus();
                        });
                    } else {
                        ui.push_id("document-action", |ui| {
                            ui.button("Focused action").request_focus();
                        });
                    }
                    if ctx.current_pass_index() == 0 {
                        ctx.request_discard("history multipass");
                    }
                },
            );
            output.textures_delta.clear();
            remaining
        };
        frame(&mut view, vec![], true);
        assert_eq!(
            frame(&mut view, vec![key(false)], true),
            1,
            "TextEdit must receive its own undo event"
        );
        assert!(rx.try_recv().is_err() && !view.pending);
        frame(&mut view, vec![], false);
        view.close_requested = true;
        assert_eq!(frame(&mut view, vec![key(false)], false), 1);
        view.close_requested = false;
        view.shape_drag = Some((Point { x: 3., y: 7. }, Point { x: 20., y: 30. }));
        assert_eq!(frame(&mut view, vec![key(false), key(true)], false), 0);
        assert!(view.shape_drag.is_none());
        assert!(matches!(rx.try_recv(), Ok(Job::Apply(Request::Undo))));
        assert!(
            rx.try_recv().is_err(),
            "repeated keys and layout passes cannot queue extra work"
        );
        frame(&mut view, vec![key(false)], false);
        assert!(rx.try_recv().is_err());
        let mut undone = presented(false);
        undone.can_redo = true;
        view.receive(&ctx, Ok(undone));
        frame(&mut view, vec![key(true)], false);
        assert!(matches!(rx.try_recv(), Ok(Job::Apply(Request::Redo))));
        view.receive(&ctx, Ok(presented(false)));
        frame(&mut view, vec![key(false), key(true)], false);
        assert!(
            rx.try_recv().is_err(),
            "disabled history actions must stay disabled"
        );
    }

    #[test]
    fn zoom_shortcuts_keep_event_order_cancel_gestures_and_do_not_zoom_ui_or_edit() {
        let ctx = egui::Context::default();
        ctx.options_mut(|options| options.zoom_with_keyboard = true);
        let mut view = View::default();
        view.receive(&ctx, Ok(presented(false)));
        let document = view.presented.as_ref().unwrap().document.clone();
        view.output = Some((view.texture.as_ref().unwrap().clone(), 123));
        view.viewport_area = Some(egui::Rect::from_min_size(
            egui::pos2(31., 47.),
            egui::vec2(400., 200.),
        ));
        view.viewport_image_size = Some(egui::vec2(800., 400.));
        view.shape_drag = Some((Point { x: 3., y: 7. }, Point { x: 20., y: 30. }));
        let key = |key, command| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: true,
            modifiers: if command {
                egui::Modifiers::COMMAND
            } else {
                egui::Modifiers::NONE
            },
        };
        let frame = |view: &mut View, events, focused| {
            let mut text = "unchanged".to_owned();
            let mut output = ctx.run_ui(
                egui::RawInput {
                    events,
                    focused,
                    ..Default::default()
                },
                |ui| {
                    handle_viewport_shortcuts(&ctx, view);
                    ui.add(
                        egui::TextEdit::singleline(&mut text).id(egui::Id::unique("zoom-field")),
                    )
                    .request_focus();
                    if ctx.current_pass_index() == 0 {
                        ctx.request_discard("shortcut multipass");
                    }
                },
            );
            output.textures_delta.clear();
            assert_eq!(text, "unchanged");
            assert_eq!(
                ctx.zoom_factor(),
                1.,
                "document shortcuts must not scale every window"
            );
        };
        frame(&mut view, vec![], true);
        frame(
            &mut view,
            vec![
                key(egui::Key::Num0, true),
                key(egui::Key::Equals, true),
                key(egui::Key::Plus, true),
                key(egui::Key::Minus, true),
            ],
            true,
        );
        assert_eq!(
            view.viewport.zoom_percent, 125.,
            "ordered events, not one action per frame"
        );
        assert!(view.shape_drag.is_none());
        frame(&mut view, vec![key(egui::Key::Num0, true)], true);
        assert_eq!(
            view.viewport.zoom_percent, 100.,
            "zero is actual size, not Fit (50%)"
        );
        frame(&mut view, vec![key(egui::Key::Plus, false)], true);
        assert_eq!(view.viewport.zoom_percent, 100.);
        let physical = |physical_key| egui::Event::Key {
            key: egui::Key::Slash,
            physical_key: Some(physical_key),
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers {
                ctrl: true,
                ..Default::default()
            },
        };
        frame(
            &mut view,
            vec![physical(egui::Key::Equals), physical(egui::Key::Num0)],
            true,
        );
        assert_eq!(
            view.viewport.zoom_percent, 125.,
            "physical Equal works; arbitrary shifted zero does not reset"
        );
        frame(&mut view, vec![physical(egui::Key::Minus)], true);
        assert_eq!(view.viewport.zoom_percent, 100.);
        view.close_requested = true;
        frame(&mut view, vec![key(egui::Key::Plus, true)], true);
        assert_eq!(view.viewport.zoom_percent, 100.);
        view.close_requested = false;
        frame(&mut view, vec![key(egui::Key::Plus, true)], false);
        assert_eq!(view.viewport.zoom_percent, 100.);
        frame(&mut view, vec![key(egui::Key::Plus, true); 20], true);
        assert_eq!(view.viewport.zoom_percent, 800.);
        frame(&mut view, vec![key(egui::Key::Minus, true); 30], true);
        assert_eq!(view.viewport.zoom_percent, 5.);
        assert_eq!(view.presented.as_ref().unwrap().document, document);
        assert!(view.output.is_some());
        assert!(!view.pending);
    }

    #[test]
    fn viewport_events_anchor_zoom_pan_once_and_never_submit_edits() {
        let ctx = egui::Context::default();
        let mut view = View::default();
        let mut value = presented(false);
        value.pixels = Arc::new(RgbaImage::new(200, 100));
        value.document = Arc::new(Document::new_capture("viewport", 200., 100., None));
        let original = value.document.clone();
        view.receive(&ctx, Ok(value));
        view.output = Some((view.texture.as_ref().unwrap().clone(), 123));
        let area = egui::Rect::from_min_size(egui::pos2(100., 80.), egui::vec2(400., 200.));
        let size = egui::vec2(200., 100.);
        let fit = fitted_image_rect(area, size);
        view.viewport_area = Some(area);
        view.viewport_image_size = Some(size);
        let (tx, rx) = mpsc::channel();
        let frame = |view: &mut View, events, focused| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(700., 500.),
                    )),
                    events,
                    focused,
                    ..Default::default()
                },
                |root| {
                    let mut ui = root.new_child(egui::UiBuilder::new().max_rect(area));
                    let intercepted = handle_viewport_input(&ui, view, area);
                    let preview = viewport_rect(view.viewport, fit, size).unwrap();
                    if view.section == Section::Layers {
                        let tokens = crate::tokens::load().into_values().next().unwrap();
                        show_layer_canvas(&mut ui, &tokens, view, &tx, area, preview, intercepted);
                    } else {
                        show_shape(
                            &mut ui,
                            test_tokens(),
                            view,
                            &tx,
                            area,
                            preview,
                            intercepted,
                        );
                    }
                    if ctx.current_pass_index() == 0 {
                        ctx.request_discard("viewport multipass");
                    }
                },
            );
            output.textures_delta.clear();
        };
        let command = egui::Modifiers {
            command: true,
            ctrl: true,
            ..Default::default()
        };
        let anchor = egui::pos2(220., 140.);
        view.shape_drag = Some((Point { x: 3., y: 7. }, Point { x: 20., y: 30. }));
        frame(
            &mut view,
            vec![
                egui::Event::PointerMoved(anchor),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0., 80.),
                    phase: egui::TouchPhase::Move,
                    modifiers: command,
                },
            ],
            true,
        );
        assert_eq!(
            view.viewport.zoom_percent, 117.4,
            "positive egui scroll zooms in once"
        );
        let preview = viewport_rect(view.viewport, fit, size).unwrap();
        let point = image_point(
            anchor,
            preview,
            Rect {
                x: 0.,
                y: 0.,
                width: 200.,
                height: 100.,
            },
        );
        // The centered 200×100 image starts at (200,130), so this pointer
        // must stay over document (20,10), not the old top-left Fit's (120,60).
        assert!((point.x - 20.).abs() < 1e-5 && (point.y - 10.).abs() < 1e-5);
        assert!(
            view.shape_drag.is_none(),
            "zoom cancels an uncommitted drawing"
        );
        let before = view.viewport;
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            pressed,
            button: egui::PointerButton::Primary,
            modifiers: command,
        };
        frame(
            &mut view,
            vec![
                button(anchor, true),
                egui::Event::PointerMoved(anchor + egui::vec2(17., -11.)),
                button(anchor + egui::vec2(23., -7.), false),
                egui::Event::PointerMoved(egui::pos2(350., 250.)),
            ],
            true,
        );
        assert_eq!(view.viewport.pan_x, before.pan_x + 23.);
        assert_eq!(view.viewport.pan_y, before.pan_y - 7.);
        assert!(view.viewport_pan.is_none());
        assert!(rx.try_recv().is_err() && !view.pending && view.output.is_some());
        assert_eq!(view.presented.as_ref().unwrap().document, original);
        frame(&mut view, vec![button(anchor, true)], true);
        assert!(view.viewport_pan.is_some());
        frame(&mut view, vec![], false);
        assert!(view.viewport_pan.is_none());
        set_viewport_zoom(&mut view, 100., None);
        view.viewport.recenter();
        assert_eq!(view.viewport.zoom_percent, 100.);
        assert_eq!(view.viewport.pan_x, 0.);
        view.reset_viewport();
        assert_eq!(viewport_rect(view.viewport, fit, size), Some(fit));
        assert!(rx.try_recv().is_err() && view.output.is_some());
        view.section = Section::Layers;
        set_viewport_zoom(&mut view, 500., None);
        frame(
            &mut view,
            vec![egui::Event::PointerButton {
                pos: egui::pos2(50., 140.),
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
            true,
        );
        assert!(
            view.layer_gesture.is_none(),
            "zoomed pixels behind the sidebar cannot receive layer input"
        );
    }

    #[test]
    fn background_changes_apply_live_and_queue_the_latest_while_busy() {
        let ctx = egui::Context::default();
        let (tx, rx) = mpsc::channel();
        let mut view = View::default();
        assert_eq!(
            view.last_solid_background,
            colors::DEFAULT_CANVAS_BACKGROUND
        );
        let mut solid = presented(false);
        Arc::make_mut(&mut solid.document).background = Some("#21436580".into());
        view.receive(&ctx, Ok(solid));
        assert_eq!(view.last_solid_background, "#21436580");
        assert_eq!(view.shown_background().as_deref(), Some("#21436580"));
        // Turning Solid off applies at once and remembers the last solid color.
        view.set_background(&tx, None);
        assert!(matches!(
            rx.try_recv(),
            Ok(Job::Apply(Request::SetBackground { color: None }))
        ));
        assert!(view.pending);
        assert_eq!(view.last_solid_background, "#21436580");
        // While the worker is busy only the latest change is kept and shown.
        view.set_background(&tx, Some("#2d9cff".into()));
        view.set_background(&tx, Some("#ff3b5c".into()));
        assert!(rx.try_recv().is_err());
        assert_eq!(view.shown_background().as_deref(), Some("#ff3b5c"));
        assert_eq!(view.last_solid_background, "#ff3b5c");
        view.flush_background(&tx);
        assert!(
            rx.try_recv().is_err(),
            "a busy worker defers the queued change"
        );
        let mut transparent = presented(true);
        Arc::make_mut(&mut transparent.document).background = None;
        view.receive(&ctx, Ok(transparent));
        assert!(!view.pending);
        assert_eq!(view.last_solid_background, "#ff3b5c");
        view.flush_background(&tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Job::Apply(Request::SetBackground { color: Some(color) })) if color == "#ff3b5c"
        ));
        assert!(view.background_queued.is_none());
        view.flush_background(&tx);
        assert!(rx.try_recv().is_err(), "each change is submitted once");
        // A rejected change falls back to the published document.
        view.receive(&ctx, Err("retry".into()));
        assert_eq!(view.shown_background(), None);
        assert_eq!(view.last_solid_background, "#ff3b5c");
    }

    #[test]
    fn opening_and_refreshing_do_not_invent_a_layer_selection() {
        let ctx = egui::Context::default();
        for has_draft in [false, true] {
            let mut view = View::default();
            let mut initial = presented(false);
            initial.has_draft = has_draft;
            view.receive(&ctx, Ok(initial));
            assert_eq!(view.section, Section::Layers, "Select is the initial tool");
            assert!(view.selected_layer.is_none());
            view.receive(&ctx, Ok(presented(false)));
            assert!(
                view.selected_layer.is_none(),
                "a refresh keeps the empty selection"
            );
            view.select_layer_exact(Some("capture-background".into()));
            view.receive(&ctx, Ok(presented(false)));
            assert_eq!(view.selected_layer.as_deref(), Some("capture-background"));
            view.select_layer_exact(None);
            view.receive(&ctx, Ok(presented(false)));
            assert!(
                view.selected_layer.is_none(),
                "a refresh does not undo deselection"
            );
            let mut created = presented_text("new-text", "Hello");
            created.created_layer = Some("new-text".into());
            view.receive(&ctx, Ok(created));
            assert_eq!(view.selected_layer.as_deref(), Some("new-text"));
        }
    }

    pub(super) fn presented(unsaved: bool) -> Presented {
        Presented {
            document: Arc::new(Document::new_capture("fixture", 7., 3., None)),
            thumbnails: BTreeMap::new(),
            image_assets: BTreeMap::new(),
            pixels: Arc::new(RgbaImage::new(7, 3)),
            original_export_path: None,
            initial_text_size: 24.,
            font_families: captures_app::editor_fonts::bundled().families,
            text_style_presets: captures_app::editor_text::TEXT_STYLE_PRESETS.into(),
            saved: None,
            copied: false,
            copied_layer: false,
            pasted_layer: false,
            created_layer: None,
            can_undo: unsaved,
            can_redo: false,
            can_paste_layer: false,
            merge_down_ids: Vec::new(),
            can_merge_visible: false,
            can_flatten: false,
            active_text_input: None,
            unsaved,
            has_draft: !unsaved,
        }
    }

    pub(super) fn presented_text(id: &str, text: &str) -> Presented {
        let mut value = presented(true);
        Arc::make_mut(&mut value.document)
            .elements
            .push(Element::Text(TextElement {
                base: ElementBase {
                    id: id.into(),
                    x: 2.,
                    y: 1.,
                    rotation: None,
                    locked: false,
                    visible: true,
                    opacity: 100.,
                    blend_mode: "source-over".into(),
                },
                text: text.into(),
                font_size: 32.,
                width: 80.,
                auto_width: Some(true),
                font_family: "saved-unknown-family".into(),
                bold: false,
                italic: false,
                align: "left".into(),
                color: "#ff3b5c".into(),
                background: None,
                outlined: false,
                rounded_background: false,
                drop_shadow: None,
                drop_shadow_style: None,
                extra: Default::default(),
            }));
        value.created_layer = Some(id.into());
        value
    }

    #[test]
    fn text_staging_survives_async_updates_and_errors_then_accepts_or_cancels_cleanly() {
        let ctx = egui::Context::default();
        let mut view = View::default();
        view.receive(&ctx, Ok(presented_text("fresh", "accepted")));
        assert_eq!(view.selected_layer.as_deref(), Some("fresh"));
        assert_eq!(
            view.text.as_ref().unwrap().staged.font_family,
            "saved-unknown-family"
        );

        view.text.as_mut().unwrap().staged.text = "composing".into();
        view.text.as_mut().unwrap().staged.font_family = "serif".into();
        view.text.as_mut().unwrap().staged.drop_shadow = true;
        view.text.as_mut().unwrap().staged.outlined = true;
        let fields = view.text.as_ref().unwrap();
        assert_eq!(fields.staged.patch(&fields.accepted).outlined, Some(true));
        assert_eq!(
            fields.staged.patch(&fields.accepted).drop_shadow,
            Some(true)
        );
        assert_eq!(
            fields.staged.patch(&fields.accepted).font_family.as_deref(),
            Some("serif")
        );
        view.request_close();
        assert!(!view.closed && !view.close_requested);
        assert!(view.error.as_deref().unwrap().contains("pending text"));
        let mut unrelated = presented_text("fresh", "accepted");
        unrelated.created_layer = None;
        view.receive(&ctx, Ok(unrelated));
        assert_eq!(view.text.as_ref().unwrap().staged.text, "composing");

        view.text_apply_pending = true;
        view.receive(&ctx, Err("transaction rejected".into()));
        assert_eq!(view.text.as_ref().unwrap().staged.text, "composing");
        assert_eq!(view.text.as_ref().unwrap().accepted.text, "accepted");
        assert_eq!(view.text.as_ref().unwrap().staged.font_family, "serif");
        assert!(view.text.as_ref().unwrap().staged.drop_shadow);
        assert!(view.text.as_ref().unwrap().staged.outlined);
        assert_eq!(
            view.text.as_ref().unwrap().accepted.font_family,
            "saved-unknown-family"
        );

        let accepted = view.text.as_ref().unwrap().accepted.clone();
        view.text.as_mut().unwrap().staged = accepted;
        assert_eq!(view.text.as_ref().unwrap().staged.text, "accepted");
        let fields = view.text.as_ref().unwrap();
        assert_eq!(fields.staged.font_family, "saved-unknown-family");
        assert!(fields.staged.patch(&fields.accepted).font_family.is_none());
        assert!(!fields.staged.drop_shadow);
        assert!(fields.staged.patch(&fields.accepted).drop_shadow.is_none());
        assert!(!fields.staged.outlined);
        assert!(fields.staged.patch(&fields.accepted).outlined.is_none());
        view.text.as_mut().unwrap().staged.text = "applied".into();
        view.text_apply_pending = true;
        view.output = Some((view.texture.as_ref().unwrap().clone(), 9));
        view.receive(&ctx, Ok(presented_text("fresh", "applied")));
        let fields = view.text.as_ref().unwrap();
        assert_eq!(fields.staged, fields.accepted);
        assert_eq!(fields.accepted.text, "applied");
        assert!(view.output.is_none());
    }

    #[test]
    fn text_presets_stage_only_treatment_fields_and_preserve_custom_plate_and_shadow() {
        let ctx = egui::Context::default();
        let mut view = View::default();
        view.receive(&ctx, Ok(presented_text("fresh", "keep this")));
        let fields = view.text.as_mut().unwrap();
        fields.accepted.background = Some("#123456".into());
        fields.accepted.rounded_background = true;
        fields.accepted.drop_shadow = true;
        fields.accepted.shadow.offset_y = -12.75;
        fields.accepted.bold = true;
        fields.accepted.align = "right".into();
        fields.staged = fields.accepted.clone();
        let presets = captures_app::editor_text::TEXT_STYLE_PRESETS;
        fields.staged.apply_preset(
            presets
                .iter()
                .find(|preset| preset.id == "mono-box")
                .unwrap(),
        );
        assert_eq!(
            fields.staged.patch(&fields.accepted),
            TextPatch {
                font_family: Some("mono".into()),
                rounded_background: Some(false),
                ..Default::default()
            }
        );
        assert_eq!(fields.staged.background.as_deref(), Some("#123456"));
        fields.staged.background = None;
        fields
            .staged
            .apply_preset(presets.iter().find(|preset| preset.id == "box").unwrap());
        assert_eq!(fields.staged.background.as_deref(), Some("#111318"));
        fields.staged.apply_preset(
            presets
                .iter()
                .find(|preset| preset.id == "outlined")
                .unwrap(),
        );
        assert_eq!(
            fields.staged.patch(&fields.accepted),
            TextPatch {
                font_family: Some("sans".into()),
                background: OptionalNullable::Null,
                outlined: Some(true),
                rounded_background: Some(false),
                ..Default::default()
            }
        );
        assert_eq!(fields.accepted.background.as_deref(), Some("#123456"));
    }

    #[test]
    fn named_text_preset_carries_forward_independently_of_staged_label_edits() {
        let ctx = egui::Context::default();
        let mut view = View::default();
        let mut initial = presented_text("old", "Label");
        initial.initial_text_size = 39.;
        view.receive(&ctx, Ok(initial));
        view.new_annotation_style.color = "#2367ab".into();
        let fields = view.text.as_mut().unwrap();
        fields.accepted.font_size = 83.;
        fields.accepted.color = "#abcdef".into();
        fields.accepted.bold = true;
        fields.staged = fields.accepted.clone();
        let (tx, rx) = mpsc::channel();
        let tokens = crate::tokens::load().remove("light-mustard").unwrap();
        let frame = |view: &mut View, events| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(400., 1600.),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    egui::CentralPanel::default().show(ui, |ui| show_text(ui, &tokens, view, &tx));
                },
            );
            output.textures_delta.clear();
            output
        };
        let position = |output: &egui::FullOutput, label: &str| {
            output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.job.text == label => {
                        Some(text.pos + text.galley.rect.center().to_vec2())
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("missing text: {label}"))
        };
        let click = |view: &mut View, pos| {
            frame(view, vec![egui::Event::PointerMoved(pos)]);
            for pressed in [true, false] {
                frame(
                    view,
                    vec![egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    }],
                );
            }
        };
        // Shipping `TextStylePicker`: the trigger sits under its "Text style"
        // legend and shows the current treatment; the menu lists every style.
        let choose_mono = |view: &mut View| {
            let output = frame(view, vec![]);
            click(view, position(&output, "Text style") + egui::vec2(20., 32.));
            let output = frame(view, vec![]);
            let row = output
                .shapes
                .iter()
                .rev()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.job.text == "Mono Box" => {
                        Some(text.pos + text.galley.rect.center().to_vec2())
                    }
                    _ => None,
                })
                .next()
                .expect("the open menu lists Mono Box");
            click(view, row);
        };
        choose_mono(&mut view);
        assert_eq!(view.new_text_preset.as_deref(), Some("mono-box"));
        // The font menu uses shipping labels, not pinned asset names.
        let output = frame(&mut view, vec![]);
        position(&output, "Monospace");
        assert!(!output.shapes.iter().any(|shape| matches!(&shape.shape,
            egui::Shape::Text(text) if text.galley.job.text.starts_with("Liberation"))));
        assert_eq!(view.new_text_size, 39.);
        assert_eq!(view.new_annotation_style.color, "#2367ab");
        assert_eq!(view.text.as_ref().unwrap().staged.font_size, 83.);
        // Shipping applies the preset at once: one live edit, its own undo step.
        let Ok(Job::Apply(Request::Live { key, request })) = rx.try_recv() else {
            panic!("the preset applies live");
        };
        assert!(key.starts_with("text:once:"));
        assert!(matches!(*request, Request::EditText { .. }));
        assert!(
            rx.try_recv().is_err(),
            "an unchanged composition is not resent"
        );
        assert_eq!(view.new_text_preset.as_deref(), Some("mono-box"));
        view.receive(&ctx, Err("render rejected".into()));
        assert_eq!(view.new_text_preset.as_deref(), Some("mono-box"));
        // A no-op choice on this label still chooses the next label's preset.
        view.text.as_mut().unwrap().accepted = view.text.as_ref().unwrap().staged.clone();
        view.new_text_preset = Some("box".into());
        choose_mono(&mut view);
        assert_eq!(view.new_text_preset.as_deref(), Some("mono-box"));
        // Later explicit creation choices win over unrelated worker snapshots.
        view.new_text_preset = Some("outlined".into());
        view.receive(&ctx, Ok(presented_text("other", "Other label")));
        assert_eq!(view.new_text_preset.as_deref(), Some("outlined"));
        assert_eq!(view.new_text_size, 39.);
        assert_eq!(view.new_annotation_style.color, "#2367ab");
    }

    #[test]
    fn text_shadow_settings_patch_only_changed_enabled_fields() {
        let ctx = egui::Context::default();
        let mut view = View::default();
        view.receive(&ctx, Ok(presented_text("fresh", "accepted")));
        let fields = view.text.as_mut().unwrap();
        fields.staged.drop_shadow = true;
        assert_eq!(
            fields.staged.patch(&fields.accepted),
            TextPatch {
                drop_shadow: Some(true),
                ..Default::default()
            }
        );
        fields.staged.shadow.offset_y = -12.75;
        fields.staged.font_size = 80.;
        assert_eq!(
            fields.staged.patch(&fields.accepted),
            TextPatch {
                font_size: Some(80.),
                drop_shadow: Some(true),
                drop_shadow_style: Some(DropShadowStylePatch {
                    offset_y: Some(-12.75),
                    ..Default::default()
                }),
                ..Default::default()
            }
        );
        fields.staged.drop_shadow = false;
        fields.staged.font_size = fields.accepted.font_size;
        fields.staged.shadow.color = "invalid hidden input".into();
        assert_eq!(fields.staged.patch(&fields.accepted), TextPatch::default());
        view.request_close();
        assert!(
            view.close_requested,
            "disabled custom fields must not block closing"
        );
    }

    #[test]
    fn text_click_placement_is_coalesced_across_egui_passes() {
        let ctx = egui::Context::default();
        let mut view = View::default();
        view.receive(&ctx, Ok(presented(false)));
        view.draw_shape = DrawShape::Text;
        view.new_text_preset = Some("mono-box".into());
        view.new_text_size = 37.5;
        view.new_annotation_style.color = "#2367ab".into();
        // Shipping places text with the drawing defaults' shadow.
        let custom = DropShadowStyle {
            color: "#123456".into(),
            opacity: 30.,
            blur: 4.,
            offset_x: 1.,
            offset_y: 2.,
            extra: Default::default(),
        };
        view.new_annotation_style.drop_shadow = Some(true);
        view.new_annotation_style.drop_shadow_style = Some(custom.clone());
        let (tx, rx) = mpsc::channel();
        let area = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(140., 60.));
        let click = egui::pos2(70., 30.);
        let button = |pressed| egui::Event::PointerButton {
            pos: click,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let run = |view: &mut View, events, discard| {
            ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(area),
                    events,
                    focused: true,
                    ..Default::default()
                },
                |_| {
                    let mut ui = egui::Ui::new(
                        ctx.clone(),
                        egui::Id::unique("text-placement-test"),
                        egui::UiBuilder::new().max_rect(area),
                    );
                    show_shape(&mut ui, test_tokens(), view, &tx, area, area, false);
                    if discard && ctx.current_pass_index() == 0 {
                        ctx.request_discard("verify text placement is one command");
                    }
                },
            )
        };
        let mut hover = run(&mut view, vec![], false);
        hover.textures_delta.clear();
        let mut priming = run(
            &mut view,
            vec![egui::Event::PointerMoved(click), button(true)],
            false,
        );
        priming.textures_delta.clear();
        assert!(rx.try_recv().is_err());
        let mut output = run(&mut view, vec![button(false)], true);
        assert!(output.platform_output.num_completed_passes >= 2);
        output.textures_delta.clear();
        let Job::Apply(Request::BeginTextInput {
            target: captures_app::editor_session::TextInputTarget::New { create },
            ..
        }) = rx.try_recv().unwrap()
        else {
            panic!()
        };
        assert_eq!(create.point, Point { x: 3.5, y: 1.5 });
        assert!(create.text.is_empty());
        assert_eq!(create.font_family, "sans");
        assert_eq!(create.style_preset.as_deref(), Some("mono-box"));
        assert_eq!(create.font_size, 37.5);
        assert_eq!(create.color, "#2367ab");
        assert_eq!(create.drop_shadow, Some(true));
        assert_eq!(create.drop_shadow_style, Some(custom));
        assert!(rx.try_recv().is_err(), "multipass click creates one layer");
    }

    #[test]
    fn new_text_defaults_are_per_editor_and_survive_responses_and_errors() {
        let ctx = egui::Context::default();
        let mut view = View::default();
        let mut initial = presented_text("old", "accepted");
        initial.initial_text_size = 39.; // Original capture, not this tiny draft canvas.
        view.receive(&ctx, Ok(initial));
        assert_eq!(view.new_text_preset.as_deref(), Some("rounded-box"));
        assert_eq!(view.new_text_size, 39.);
        assert_eq!(view.new_annotation_style.color, "#ff3b5c");
        view.new_text_preset = Some("mono-box".into());
        view.new_text_size = 37.5;
        view.new_annotation_style.color = "invalid input".into();
        view.receive(&ctx, Err("Invalid color".into()));
        view.receive(&ctx, Ok(presented_text("old", "accepted")));
        assert_eq!(view.new_text_preset.as_deref(), Some("mono-box"));
        assert_eq!(view.new_text_size, 37.5);
        assert_eq!(view.new_annotation_style.color, "invalid input");
        assert_eq!(view.text.as_ref().unwrap().accepted.text, "accepted");
        let mut legacy = presented_text("old", "accepted");
        legacy.font_families.remove("rounded");
        legacy
            .text_style_presets
            .retain(|preset| preset.font_family != "rounded");
        let mut reopened = View::default();
        reopened.receive(&ctx, Ok(legacy));
        assert_eq!(reopened.new_text_preset.as_deref(), Some("standard"));
        let mut plain = presented_text("old", "accepted");
        plain.text_style_presets.clear();
        let mut reopened = View::default();
        reopened.receive(&ctx, Ok(plain));
        assert_eq!(reopened.new_text_preset, None);
        assert_eq!(reopened.new_text_size, 24.);
        assert_eq!(reopened.new_annotation_style.color, "#ff3b5c");
    }

    fn layer_frame(
        ctx: &egui::Context,
        view: &mut View,
        tx: &Sender<Job>,
        preview: egui::Rect,
        focused: bool,
        modifiers: egui::Modifiers,
        mut events: Vec<egui::Event>,
    ) {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400., 300.));
        events.insert(0, egui::Event::ModifiersChanged(modifiers));
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(screen),
                events,
                focused,
                ..Default::default()
            },
            |_| {
                let mut ui = egui::Ui::new(
                    ctx.clone(),
                    egui::Id::unique("rotation-layer-canvas-test"),
                    egui::UiBuilder::new().max_rect(screen),
                );
                let tokens = crate::tokens::load().into_values().next().unwrap();
                show_layer_canvas(&mut ui, &tokens, view, tx, screen, preview, false);
                if ctx.current_pass_index() == 0 {
                    ctx.request_discard("multipass");
                }
            },
        );
        assert!(output.platform_output.num_completed_passes >= 2);
        output.textures_delta.clear();
    }

    #[test]
    fn displaying_annotation_controls_does_not_clamp_legacy_values_into_a_patch() {
        let ctx = egui::Context::default();
        let original = ElementStyle {
            stroke_width: 200.,
            stroke_enabled: None,
            drop_shadow: Some(true),
            ..ElementStyle::default()
        };
        let mut view = View {
            annotation: Some(AnnotationFields::new(&original)),
            ..View::default()
        };
        let (tx, rx) = mpsc::channel();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(600., 1400.));
        ctx.begin_pass(egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        });
        let mut ui = egui::Ui::new(
            ctx.clone(),
            egui::Id::unique("annotation-test"),
            egui::UiBuilder::new().max_rect(screen),
        );
        let tokens = crate::tokens::load().remove("light-mustard").unwrap();
        show_annotation(&mut ui, &tokens, &mut view, &tx, "shape", &original, true);
        let mut output = ctx.end_pass();
        output.textures_delta.clear();
        let fields = view.annotation.as_ref().unwrap();
        assert_eq!(fields.style.stroke_width, 200.);
        assert_eq!(fields.shadow.blur, 170.);
        assert_eq!(fields.patch(&original), AnnotationStylePatch::default());
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn selected_draw_properties_edit_the_layer_not_creation_defaults() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let tokens = crate::tokens::load().remove("light-mustard").unwrap();
        let (mut view, id) = covered_canvas_view(&ctx);
        view.section = Section::Draw;
        view.draw_shape = DrawShape::Arrow;
        view.select_layer(Some(id.clone()));
        let defaults = view.new_annotation_style.clone();
        let (tx, rx) = mpsc::channel();
        let frame = |view: &mut View, events| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1200., 1600.),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| show(ui, &tokens, view, &tx),
            );
            output.textures_delta.clear();
            output
        };
        frame(&mut view, vec![]);
        let output = frame(&mut view, vec![]);
        let nodes = &output
            .platform_output
            .accesskit_update
            .as_ref()
            .unwrap()
            .nodes;
        assert!(
            nodes
                .iter()
                .any(|(_, node)| node.label() == Some("Shift rotation snap"))
        );
        let bounds = nodes
            .iter()
            .find(|(_, node)| node.label() == Some("Fill color: #2d9cff"))
            .and_then(|(_, node)| node.bounds())
            .expect("selected-layer color control");
        let blue = egui::pos2(
            ((bounds.x0 + bounds.x1) / 2.) as f32,
            ((bounds.y0 + bounds.y1) / 2.) as f32,
        );
        assert_eq!(
            (view.section, view.properties_section()),
            (Section::Draw, Section::Layers)
        );
        assert!(rx.try_recv().is_err(), "showing Properties is not an edit");
        frame(&mut view, vec![egui::Event::PointerMoved(blue)]);
        for pressed in [true, false] {
            frame(
                &mut view,
                vec![egui::Event::PointerButton {
                    pos: blue,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
        }
        let Ok(Job::Apply(Request::Live { key, request })) = rx.try_recv() else {
            panic!("selected swatch applies live")
        };
        assert_eq!(key, format!("style:{id}:fill-color"));
        assert!(
            matches!(*request, Request::Layer { id: target, edit: LayerEdit::AnnotationStyle { patch } }
            if target == id && patch.fill == OptionalNullable::Value("#2d9cff".into()))
        );
        assert_eq!(view.new_annotation_style, defaults);
        assert_eq!(
            (view.section, view.draw_shape),
            (Section::Draw, DrawShape::Arrow)
        );
        view.pending = false; // No worker runs in this detached View fixture.
        view.activate_tool(Section::Draw, Some(DrawShape::Arrow));
        frame(&mut view, vec![]);
        let output = frame(&mut view, vec![]);
        let nodes = &output
            .platform_output
            .accesskit_update
            .as_ref()
            .unwrap()
            .nodes;
        assert!(
            !nodes
                .iter()
                .any(|(_, node)| node.label() == Some("Shift rotation snap"))
        );
        assert!(
            nodes
                .iter()
                .any(|(_, node)| node.label() == Some("Color: #2d9cff"))
        );
        assert_eq!(view.properties_section(), Section::Draw);
        assert!(rx.try_recv().is_err());

        for (tool, title) in [(DrawShape::Text, "Text"), (DrawShape::Freehand, "Freehand")] {
            view.activate_tool(Section::Draw, Some(tool));
            view.select_layer(Some(id.clone()));
            frame(&mut view, vec![]);
            let output = frame(&mut view, vec![]);
            let nodes = &output
                .platform_output
                .accesskit_update
                .as_ref()
                .unwrap()
                .nodes;
            assert!(
                !nodes
                    .iter()
                    .any(|(_, node)| node.label() == Some("Shift rotation snap"))
            );
            assert!(
                nodes
                    .iter()
                    .any(|(_, node)| node.label() == Some("Fill color: #2d9cff")),
                "selected style controls stay present with {title} active"
            );
            assert!(output.shapes.iter().any(|shape| matches!(&shape.shape,
                egui::Shape::Text(text) if text.galley.job.text == title)));
            assert_eq!(view.selected_layer.as_deref(), Some(id.as_str()));
            assert_eq!(view.new_annotation_style, defaults);
            assert!(rx.try_recv().is_err());
        }
    }

    #[test]
    fn annotation_stroke_color_uses_shared_swatches_and_applies_live() {
        let ctx = egui::Context::default();
        let tokens = crate::tokens::load().remove("dark-mustard").unwrap();
        let original = ElementStyle {
            color: "#ff3b5c".into(),
            ..ElementStyle::default()
        };
        let mut view = View {
            annotation: Some(AnnotationFields::new(&original)),
            pending: false,
            ..View::default()
        };
        let (tx, rx) = mpsc::channel();
        let frame = |view: &mut View, events| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(320., 900.),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    egui::CentralPanel::default().show(ui, |ui| {
                        show_annotation(ui, &tokens, view, &tx, "shape", &original, false)
                    });
                },
            );
            output.textures_delta.clear();
            output
        };
        let circles = |output: &egui::FullOutput, color: &str| -> Vec<egui::Pos2> {
            let fill = egui::Color32::from_hex(color).unwrap();
            output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Circle(circle) if circle.fill == fill => Some(circle.center),
                    _ => None,
                })
                .collect()
        };
        let output = frame(&mut view, vec![]);
        // The shipping palette, in order, on the stroke color row.
        let mut previous = None;
        for swatch in colors::SWATCHES {
            let centers = circles(&output, swatch);
            assert_eq!(centers.len(), 1, "{swatch}");
            if let Some(previous) = previous {
                let egui::Pos2 { x, y } = centers[0];
                let prev: egui::Pos2 = previous;
                assert!((x > prev.x && y == prev.y) || y > prev.y, "{swatch}");
            }
            previous = Some(centers[0]);
        }
        let blue = circles(&output, "#2d9cff")[0];
        frame(&mut view, vec![egui::Event::PointerMoved(blue)]);
        for pressed in [true, false] {
            frame(
                &mut view,
                vec![egui::Event::PointerButton {
                    pos: blue,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
        }
        let fields = view.annotation.as_ref().unwrap();
        assert_eq!(fields.style.color, "#2d9cff");
        // Shipping applies the swatch at once, folded by field for undo.
        let Ok(Job::Apply(Request::Live { key, request })) = rx.try_recv() else {
            panic!("a swatch applies live");
        };
        assert_eq!(key, "style:shape:stroke-color");
        assert!(
            matches!(*request, Request::Layer { ref id, edit: LayerEdit::AnnotationStyle { ref patch } }
            if id == "shape" && patch.color.as_deref() == Some("#2d9cff"))
        );
        assert!(rx.try_recv().is_err(), "one edit per change");
        // The active ring moves to the chosen swatch.
        let output = frame(&mut view, vec![]);
        let accent = tokens.color("theme-accent");
        assert!(
            output.shapes.iter().any(|shape| matches!(&shape.shape,
            egui::Shape::Circle(circle) if circle.center == blue && circle.stroke.color == accent))
        );
    }

    #[test]
    fn live_annotation_changes_fold_by_field_and_toggles_stand_alone() {
        let before = AnnotationFields::new(&ElementStyle::default());
        let changed = |edit: &dyn Fn(&mut AnnotationFields)| {
            let mut after = before.clone();
            edit(&mut after);
            after.changed_field(&before)
        };
        assert_eq!(
            changed(&|f| f.style.stroke_width += 3.),
            Some("stroke-width")
        );
        assert_eq!(
            changed(&|f| f.style.color = "#2d9cff".into()),
            Some("stroke-color")
        );
        assert_eq!(changed(&|f| f.shadow.blur += 1.), Some("shadow-blur"));
        assert_eq!(changed(&|f| f.shadow.offset_y -= 1.), Some("shadow-y"));
        // Each toggle is its own undo step, as in shipping.
        let shadow = before.style.has_drop_shadow();
        assert_eq!(changed(&|f| f.style.drop_shadow = Some(!shadow)), None);
        assert_eq!(changed(&|f| f.style.fill = None), None);
        assert_eq!(
            changed(&|f| f.style.fill = Some("#ffffff".into())),
            Some("fill-color")
        );
        assert_eq!(changed(&|f| f.style.stroke_enabled = Some(false)), None);
    }

    #[test]
    fn annotation_fields_patch_only_changed_values_and_preserve_legacy_defaults() {
        use serde_json::json;
        let original = ElementStyle {
            stroke_enabled: None,
            drop_shadow: Some(true),
            drop_shadow_style: Some(DropShadowStyle {
                color: "#123456".into(),
                opacity: 80.,
                blur: 3.,
                offset_x: 900.,
                offset_y: -2.,
                extra: serde_json::from_value(json!({"futureShadow": "keep"})).unwrap(),
            }),
            extra: serde_json::from_value(json!({"futureStyle": [7, 2]})).unwrap(),
            ..ElementStyle::default()
        };
        let mut fields = AnnotationFields::new(&original);
        assert_eq!(fields.shadow.offset_x, 500.); // Display resolves, but does not rewrite raw data.
        assert_eq!(fields.patch(&original), AnnotationStylePatch::default());
        fields.style.fill = None;
        fields.shadow.offset_x = -7.;
        assert_eq!(
            serde_json::to_value(fields.patch(&original)).unwrap(),
            json!({"fill": null, "dropShadowStyle": {"offsetX": -7.0}})
        );
        fields.style.drop_shadow = Some(false);
        assert_eq!(
            serde_json::to_value(fields.patch(&original)).unwrap(),
            json!({"fill": null, "dropShadow": false})
        ); // Never materialize hidden shadow fields.
        assert_eq!(original.drop_shadow_style.as_ref().unwrap().offset_x, 900.);
        assert_eq!(fields.style.extra["futureStyle"], json!([7, 2]));
        assert_eq!(fields.shadow.extra["futureShadow"], "keep");

        let original = ElementStyle::default();
        let mut fields = AnnotationFields::new(&original);
        fields.style.stroke_width = 20.;
        fields.style.drop_shadow = Some(true);
        assert_eq!(
            serde_json::to_value(fields.patch(&original)).unwrap(),
            json!({"strokeWidth": 20.0, "dropShadow": true})
        );
    }

    #[test]
    fn annotation_selection_and_error_restore_the_published_layer_fields() {
        let ctx = egui::Context::default();
        let mut view = View::default();
        let mut value = presented(true);
        let document = Arc::make_mut(&mut value.document);
        let id = document
            .create_closed_shape(ClosedShapeCreate {
                shape: ClosedShapeKind::Ellipse,
                start: Point { x: 1., y: 1. },
                end: Point { x: 5., y: 2. },
                style: ElementStyle::default(),
                opacity: 100.,
            })
            .unwrap();
        document
            .edit_layer(&id, LayerEdit::Lock { locked: true })
            .unwrap();
        document
            .edit_layer(&id, LayerEdit::Visibility { visible: false })
            .unwrap();
        view.receive(&ctx, Ok(value));
        view.select_layer_exact(Some(id.clone()));
        assert_eq!(view.selected_layer.as_deref(), Some(id.as_str()));
        let pixels = view.presented.as_ref().unwrap().pixels.clone();
        view.annotation.as_mut().unwrap().style.fill = Some("bad color".into());
        view.receive(&ctx, Err("Style failed".into()));
        assert_eq!(
            view.annotation.as_ref().unwrap().style.fill.as_deref(),
            Some("#ff3b5c")
        );
        assert!(Arc::ptr_eq(
            &pixels,
            &view.presented.as_ref().unwrap().pixels
        ));
        view.select_layer(Some("capture-background".into()));
        assert!(view.annotation.is_none());
        view.select_layer(Some(id));
        assert!(view.annotation.is_some());
    }

    #[test]
    fn select_double_click_begins_text_once_without_bypassing_layer_or_viewport_guards() {
        for (visible, locked, intercepted, prior_click) in [
            (true, false, false, false),
            (false, false, false, false),
            (true, true, false, false),
            (true, false, true, false),
            (true, false, false, true),
            (false, false, false, true),
            (true, true, false, true),
            (true, false, true, true),
        ] {
            let ctx = egui::Context::default();
            let mut value = presented_text("label", "Text");
            value.pixels = Arc::new(RgbaImage::new(200, 100));
            let document = Arc::make_mut(&mut value.document);
            document.width = 200.;
            document.height = 100.;
            let Element::Text(text) = document.elements.last_mut().unwrap() else {
                panic!("expected text fixture")
            };
            text.base.visible = visible;
            text.base.locked = locked;
            let mut view = View::default();
            view.receive(&ctx, Ok(value));
            view.section = Section::Layers;
            view.select_layer_exact(None);
            let (tx, rx) = mpsc::channel();
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400., 300.));
            let preview = egui::Rect::from_min_size(egui::pos2(100., 100.), egui::vec2(200., 100.));
            let mut clicks = Vec::new();
            if prior_click {
                // egui counts only the most recent pair's distance when
                // classifying a triple-click. A recent click elsewhere can
                // therefore turn the second canvas click into a triple.
                clicks.extend([true, false].map(|pressed| (egui::pos2(10., 10.), pressed)));
            }
            clicks.extend(
                [true, false, true, false].map(|pressed| (egui::pos2(132., 116.), pressed)),
            );
            for (index, &(position, pressed)) in clicks.iter().enumerate() {
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        time: Some(index as f64 * 0.05),
                        screen_rect: Some(screen),
                        focused: true,
                        events: vec![
                            egui::Event::PointerMoved(position),
                            egui::Event::PointerButton {
                                pos: position,
                                button: egui::PointerButton::Primary,
                                pressed,
                                modifiers: egui::Modifiers::NONE,
                            },
                        ],
                        ..Default::default()
                    },
                    |ui| {
                        let tokens = crate::tokens::load().into_values().next().unwrap();
                        show_layer_canvas(
                            ui,
                            &tokens,
                            &mut view,
                            &tx,
                            screen,
                            preview,
                            intercepted,
                        );
                        if ctx.current_pass_index() == 0 {
                            ctx.request_discard("multipass double-click");
                        }
                    },
                );
                output.textures_delta.clear();
                if index + 1 < clicks.len() {
                    assert!(rx.try_recv().is_err(), "single click only selects");
                }
            }
            if visible && !locked && !intercepted {
                assert!(
                    matches!(rx.try_recv(), Ok(Job::Apply(Request::BeginTextInput {
                    target: captures_app::editor_session::TextInputTarget::Existing { id }, ..
                })) if id == "label")
                );
                assert!(view.inline.is_some() && view.layer_gesture.is_none());
            } else {
                assert!(view.inline.is_none());
            }
            assert!(
                rx.try_recv().is_err(),
                "one double-click owns one transaction"
            );
        }
    }

    #[test]
    fn layer_canvas_click_and_drag_pick_once_and_commit_only_on_release() {
        let ctx = egui::Context::default();
        let mut value = presented(false);
        value.pixels = Arc::new(RgbaImage::new(200, 100));
        let document = Arc::make_mut(&mut value.document);
        document.width = 200.;
        document.height = 100.;
        let id = document
            .create_closed_shape(ClosedShapeCreate {
                shape: ClosedShapeKind::Rectangle,
                start: Point { x: 20., y: 20. },
                end: Point { x: 80., y: 60. },
                style: ElementStyle::default(),
                opacity: 100.,
            })
            .unwrap();
        let mut view = View::default();
        view.receive(&ctx, Ok(value));
        view.pending = false;
        view.section = Section::Layers;
        view.select_layer(Some("capture-background".into()));
        view.output = Some((view.texture.as_ref().unwrap().clone(), 123));
        let (tx, rx) = mpsc::channel();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400., 300.));
        let preview = egui::Rect::from_min_size(egui::pos2(100., 100.), egui::vec2(100., 50.));
        let frame = |view: &mut View, events| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    events,
                    focused: true,
                    ..Default::default()
                },
                |_| {
                    let mut ui = egui::Ui::new(
                        ctx.clone(),
                        egui::Id::unique("layer-canvas-test"),
                        egui::UiBuilder::new().max_rect(screen),
                    );
                    let tokens = crate::tokens::load().into_values().next().unwrap();
                    show_layer_canvas(&mut ui, &tokens, view, &tx, screen, preview, false);
                    if ctx.current_pass_index() == 0 {
                        ctx.request_discard("multipass");
                    }
                },
            );
            output.textures_delta.clear();
        };
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let inside = egui::pos2(125., 120.);
        frame(&mut view, vec![button(inside, true)]);
        assert_eq!(view.selected_layer.as_deref(), Some("capture-background"));
        assert!(rx.try_recv().is_err());
        frame(
            &mut view,
            vec![
                button(inside, false),
                egui::Event::PointerMoved(egui::pos2(190., 145.)),
            ],
        );
        assert_eq!(view.selected_layer.as_deref(), Some(id.as_str()));
        assert!(rx.try_recv().is_err() && view.output.is_some());

        let empty = egui::pos2(190., 145.);
        frame(&mut view, vec![button(empty, true), button(empty, false)]);
        assert!(view.selected_layer.is_none() && view.annotation.is_none());
        assert!(rx.try_recv().is_err() && view.output.is_some());
        frame(&mut view, vec![button(inside, true)]);
        view.cancel_layer_gesture();
        frame(&mut view, vec![button(egui::pos2(120., 110.), false)]);
        assert!(rx.try_recv().is_err() && view.selected_layer.is_none());

        frame(
            &mut view,
            vec![
                button(inside, true),
                egui::Event::Key {
                    key: egui::Key::Escape,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                },
                button(egui::pos2(140., 130.), false),
            ],
        );
        assert!(rx.try_recv().is_err() && view.selected_layer.is_none() && !view.pending);

        frame(&mut view, vec![button(inside, true)]);
        frame(
            &mut view,
            vec![egui::Event::PointerMoved(egui::pos2(123., 120.))],
        );
        match &view.layer_gesture.as_ref().unwrap().kind {
            LayerGestureKind::Move {
                outline, guides, ..
            } => {
                // Default ten-pixel stroke extends five pixels outside the shape.
                assert_eq!(outline.unwrap()[0], Point { x: 15., y: 15. });
                assert!(guides.is_empty(), "sub-threshold moves hide guides");
            }
            _ => unreachable!(),
        }
        frame(
            &mut view,
            vec![egui::Event::PointerMoved(egui::pos2(116., 120.))],
        );
        match &view.layer_gesture.as_ref().unwrap().kind {
            LayerGestureKind::Move {
                outline, guides, ..
            } => {
                assert_eq!(outline.unwrap()[0].x, 0., "preview snaps to canvas edge");
                assert!(guides.iter().any(|guide| {
                    guide.orientation == GuideOrientation::Vertical && guide.position == 0.
                }));
            }
            _ => unreachable!(),
        }
        assert!(rx.try_recv().is_err() && !view.pending);
        frame(
            &mut view,
            vec![
                button(egui::pos2(90., 95.), false),
                egui::Event::PointerMoved(inside),
            ],
        );
        assert!(view.pending && view.output.is_none() && view.selected_layer.is_none());
        assert!(
            matches!(rx.recv().unwrap(), Job::Apply(Request::Layer { id: target, edit: LayerEdit::DragMove { delta_x, delta_y, display_scale } }) if target == id && delta_x == -70. && delta_y == -50. && display_scale == 0.5)
        );
        assert!(rx.try_recv().is_err(), "multipass must not submit twice");
        view.receive(&ctx, Err("move failed".into()));
        assert!(view.selected_layer.is_none() && view.pending_layer_selection.is_none());
    }

    /// A layer document with one unlocked rectangle, selected in Layers.
    fn covered_canvas_view(ctx: &egui::Context) -> (View, String) {
        let mut value = presented(false);
        value.pixels = Arc::new(RgbaImage::new(200, 100));
        let document = Arc::make_mut(&mut value.document);
        document.width = 200.;
        document.height = 100.;
        let id = document
            .create_closed_shape(ClosedShapeCreate {
                shape: ClosedShapeKind::Rectangle,
                start: Point { x: 20., y: 20. },
                end: Point { x: 80., y: 60. },
                style: ElementStyle::default(),
                opacity: 100.,
            })
            .unwrap();
        let mut view = View::default();
        view.receive(ctx, Ok(value));
        view.pending = false;
        view.section = Section::Layers;
        view.output = Some((view.texture.as_ref().unwrap().clone(), 123));
        (view, id)
    }

    #[test]
    fn a_popover_over_the_layer_canvas_takes_the_press_instead_of_a_pick() {
        // The layer settings popover is a foreground area, not an egui Popup:
        // a press on it over the canvas must not pick or drag the layer below.
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400., 300.));
        let preview = egui::Rect::from_min_size(egui::pos2(100., 100.), egui::vec2(100., 50.));
        let inside = egui::pos2(125., 120.);
        for covered in [false, true] {
            let ctx = egui::Context::default();
            let (mut view, id) = covered_canvas_view(&ctx);
            view.select_layer(Some("capture-background".into()));
            let (tx, rx) = mpsc::channel();
            let frame = |view: &mut View, events| {
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(screen),
                        events,
                        focused: true,
                        ..Default::default()
                    },
                    |_| {
                        let mut ui = egui::Ui::new(
                            ctx.clone(),
                            egui::Id::unique("covered-layer-canvas"),
                            egui::UiBuilder::new().max_rect(screen),
                        );
                        let tokens = crate::tokens::load().into_values().next().unwrap();
                        let intercepted = handle_viewport_input(&ui, view, screen);
                        show_layer_canvas(
                            &mut ui,
                            &tokens,
                            view,
                            &tx,
                            screen,
                            preview,
                            intercepted,
                        );
                        if covered {
                            egui::Area::new(egui::Id::unique("popover"))
                                .order(egui::Order::Foreground)
                                .fixed_pos(inside - egui::vec2(40., 15.))
                                .show(&ctx, |ui| {
                                    ui.allocate_exact_size(
                                        egui::vec2(80., 30.),
                                        egui::Sense::click(),
                                    );
                                });
                        }
                    },
                );
                output.textures_delta.clear();
            };
            let button = |button, pressed, command| egui::Event::PointerButton {
                pos: inside,
                button,
                pressed,
                modifiers: egui::Modifiers {
                    command,
                    ctrl: command,
                    ..Default::default()
                },
            };
            frame(&mut view, vec![]);
            frame(&mut view, vec![egui::Event::PointerMoved(inside)]);
            frame(
                &mut view,
                vec![button(egui::PointerButton::Primary, true, false)],
            );
            frame(
                &mut view,
                vec![button(egui::PointerButton::Primary, false, false)],
            );
            assert!(rx.try_recv().is_err() && view.layer_gesture.is_none());
            // Middle-button pans also start only on the canvas itself.
            frame(
                &mut view,
                vec![button(egui::PointerButton::Middle, true, false)],
            );
            let panning = view.viewport_pan.is_some();
            frame(
                &mut view,
                vec![button(egui::PointerButton::Middle, false, false)],
            );
            if covered {
                assert_eq!(
                    view.selected_layer.as_deref(),
                    Some("capture-background"),
                    "the popover owns the press"
                );
                assert!(!panning);
            } else {
                assert_eq!(view.selected_layer.as_deref(), Some(id.as_str()));
                assert!(panning, "an uncovered middle press pans");
            }
        }
    }

    #[test]
    fn an_open_select_over_the_canvas_takes_presses_and_keys_like_a_popup() {
        // The export bar's selects open their listboxes over the canvas.
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400., 300.));
        let preview = egui::Rect::from_min_size(egui::pos2(100., 100.), egui::vec2(100., 50.));
        let trigger = egui::Rect::from_min_size(egui::pos2(100., 60.), egui::vec2(100., 28.));
        // The canvas sits below the bar that holds the trigger.
        let canvas = egui::Rect::from_min_max(egui::pos2(0., 95.), screen.max);
        let ctx = egui::Context::default();
        let (mut view, id) = covered_canvas_view(&ctx);
        view.select_layer(Some(id.clone()));
        let (tx, rx) = mpsc::channel();
        let options = ['a', 'b', 'c', 'd'].map(|value| {
            crate::primitives::SelectOption::new(value, if value == 'a' { "A" } else { "Other" })
        });
        let value = std::cell::Cell::new('a');
        let frame = |view: &mut View, events| {
            let mut rows = Vec::new();
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    events,
                    focused: true,
                    ..Default::default()
                },
                |_| {
                    let tokens = crate::tokens::load().into_values().next().unwrap();
                    handle_document_shortcuts(&ctx, view, &tx);
                    let mut ui = egui::Ui::new(
                        ctx.clone(),
                        egui::Id::unique("select-over-canvas"),
                        egui::UiBuilder::new().max_rect(screen),
                    );
                    ui.scope_builder(egui::UiBuilder::new().max_rect(trigger), |ui| {
                        let output = crate::primitives::Select::new("over-canvas", "Letters", 100.)
                            .show(ui, &tokens, &options, &value.get());
                        rows = output.rows;
                        if let Some(chosen) = output.chosen {
                            value.set(chosen);
                        }
                    });
                    let intercepted = handle_viewport_input(&ui, view, canvas);
                    show_layer_canvas(&mut ui, &tokens, view, &tx, canvas, preview, intercepted);
                },
            );
            output.textures_delta.clear();
            rows
        };
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let arrow = egui::Event::Key {
            key: egui::Key::ArrowRight,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        frame(&mut view, vec![]);
        let open = trigger.center();
        frame(
            &mut view,
            vec![egui::Event::PointerMoved(open), button(open, true)],
        );
        frame(&mut view, vec![button(open, false)]);
        let rows = frame(&mut view, vec![]);
        assert!(egui::Popup::is_any_open(&ctx));
        // A row over empty canvas (the rectangle spans x 110..140): uncovered,
        // this press would deselect the layer.
        let (index, at) = rows
            .iter()
            .enumerate()
            .map(|(index, row)| (index, egui::pos2(170., row.center().y)))
            .find(|(index, at)| rows[*index].contains(*at) && preview.contains(*at))
            .expect("a listbox row over the canvas");
        frame(&mut view, vec![arrow.clone()]);
        assert!(
            rx.try_recv().is_err(),
            "an open listbox's arrows must not nudge the selected layer"
        );
        frame(
            &mut view,
            vec![egui::Event::PointerMoved(at), button(at, true)],
        );
        frame(&mut view, vec![button(at, false)]);
        frame(&mut view, vec![]);
        assert_eq!(value.get(), options[index].value, "the row takes the click");
        assert_eq!(view.selected_layer.as_deref(), Some(id.as_str()));
        assert!(view.layer_gesture.is_none() && rx.try_recv().is_err());
        assert!(!egui::Popup::is_any_open(&ctx));
        frame(&mut view, vec![arrow]);
        assert!(
            matches!(
                rx.try_recv(),
                Ok(Job::Apply(Request::Layer {
                    edit: LayerEdit::Translate { .. },
                    ..
                }))
            ),
            "with the listbox closed, arrows nudge again"
        );
    }

    #[test]
    fn layer_canvas_rotation_handle_previews_cancels_and_commits_transactionally() {
        let ctx = egui::Context::default();
        let mut value = presented(false);
        value.pixels = Arc::new(RgbaImage::new(200, 100));
        let document = Arc::make_mut(&mut value.document);
        document.width = 200.;
        document.height = 100.;
        let id = document
            .create_closed_shape(ClosedShapeCreate {
                shape: ClosedShapeKind::Rectangle,
                start: Point { x: 20., y: 20. },
                end: Point { x: 80., y: 60. },
                style: ElementStyle::default(),
                opacity: 100.,
            })
            .unwrap();
        document
            .edit_layer(&id, LayerEdit::Rotate { radians: 0.2 })
            .unwrap();
        let original_outline = document
            .elements
            .last()
            .unwrap()
            .selection_outline()
            .unwrap();
        let handle = rotation_handle(original_outline, 0.2, 0.5, 200., 100.).unwrap();
        let project =
            |point: Point| egui::pos2(100. + point.x as f32 / 2., 100. + point.y as f32 / 2.);
        let grip = project(handle.handle);
        let target = egui::pos2(grip.x + 18., grip.y - 11.);
        let preview = egui::Rect::from_min_size(egui::pos2(100., 100.), egui::vec2(100., 50.));
        let button = |pos, pressed, modifiers| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers,
        };
        let escape = egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let mut view = View {
            rotation_snap_degrees: 37.,
            ..Default::default()
        };
        view.receive(&ctx, Ok(value));
        view.pending = false;
        view.section = Section::Layers;
        view.select_layer(Some(id.clone()));
        let (tx, rx) = mpsc::channel();

        // The fitted grip is inside this small image and overlaps its body, but
        // must begin rotation rather than picking/moving the layer.
        layer_frame(
            &ctx,
            &mut view,
            &tx,
            preview,
            true,
            egui::Modifiers::NONE,
            vec![button(grip, true, egui::Modifiers::NONE)],
        );
        assert!(matches!(
            view.layer_gesture.as_ref().map(|gesture| &gesture.kind),
            Some(LayerGestureKind::Rotate { .. })
        ));
        layer_frame(
            &ctx,
            &mut view,
            &tx,
            preview,
            true,
            egui::Modifiers::NONE,
            vec![button(grip, false, egui::Modifiers::NONE)],
        );
        assert!(
            rx.try_recv().is_err() && !view.pending,
            "a grip click is a no-op"
        );

        layer_frame(
            &ctx,
            &mut view,
            &tx,
            preview,
            true,
            egui::Modifiers::NONE,
            vec![
                button(grip, true, egui::Modifiers::NONE),
                egui::Event::PointerMoved(target),
            ],
        );
        let free = match &view.layer_gesture.as_ref().unwrap().kind {
            LayerGestureKind::Rotate { snap, .. } => {
                assert!(!snap);
                preview_rotation(
                    original_outline,
                    0.2,
                    handle.handle,
                    image_point(
                        target,
                        preview,
                        Rect {
                            x: 0.,
                            y: 0.,
                            width: 200.,
                            height: 100.,
                        },
                    ),
                    None,
                )
                .unwrap()
            }
            _ => panic!("grip must beat body picking"),
        };
        layer_frame(
            &ctx,
            &mut view,
            &tx,
            preview,
            true,
            egui::Modifiers {
                shift: true,
                ..Default::default()
            },
            vec![],
        );
        match &view.layer_gesture.as_ref().unwrap().kind {
            LayerGestureKind::Rotate { snap, .. } => assert!(*snap),
            _ => unreachable!(),
        }
        let snapped = preview_rotation(
            original_outline,
            0.2,
            handle.handle,
            image_point(
                target,
                preview,
                Rect {
                    x: 0.,
                    y: 0.,
                    width: 200.,
                    height: 100.,
                },
            ),
            Some(view.rotation_snap_degrees),
        )
        .unwrap();
        // This grip movement is about -74.3°, so custom 37° stops give -74°,
        // not the old hard-coded -75° stop.
        assert!((snapped.radians + 74. * std::f64::consts::PI / 180.).abs() < 1e-12);
        assert_ne!(
            free.radians, snapped.radians,
            "stationary Shift changes the preview"
        );
        layer_frame(
            &ctx,
            &mut view,
            &tx,
            preview,
            true,
            egui::Modifiers::NONE,
            vec![escape.clone(), button(target, false, egui::Modifiers::NONE)],
        );
        assert!(rx.try_recv().is_err() && view.layer_gesture.is_none());

        // Same-frame Escape and host cancellation paths cannot leak a command.
        layer_frame(
            &ctx,
            &mut view,
            &tx,
            preview,
            true,
            egui::Modifiers::NONE,
            vec![
                button(grip, true, egui::Modifiers::NONE),
                escape.clone(),
                button(target, false, egui::Modifiers::NONE),
            ],
        );
        layer_frame(
            &ctx,
            &mut view,
            &tx,
            preview,
            true,
            egui::Modifiers::NONE,
            vec![button(grip, true, egui::Modifiers::NONE)],
        );
        layer_frame(
            &ctx,
            &mut view,
            &tx,
            egui::Rect::from_min_size(preview.min, egui::vec2(99., 50.)),
            true,
            egui::Modifiers::NONE,
            vec![],
        );
        assert!(view.layer_gesture.is_none() && rx.try_recv().is_err());
        layer_frame(
            &ctx,
            &mut view,
            &tx,
            preview,
            true,
            egui::Modifiers::NONE,
            vec![button(grip, true, egui::Modifiers::NONE)],
        );
        layer_frame(
            &ctx,
            &mut view,
            &tx,
            preview,
            false,
            egui::Modifiers::NONE,
            vec![],
        );
        assert!(view.layer_gesture.is_none() && rx.try_recv().is_err());

        layer_frame(
            &ctx,
            &mut view,
            &tx,
            preview,
            true,
            egui::Modifiers::NONE,
            vec![
                button(grip, true, egui::Modifiers::NONE),
                egui::Event::PointerMoved(target),
            ],
        );
        layer_frame(
            &ctx,
            &mut view,
            &tx,
            preview,
            true,
            egui::Modifiers {
                shift: true,
                ..Default::default()
            },
            vec![
                button(
                    target,
                    false,
                    egui::Modifiers {
                        shift: true,
                        ..Default::default()
                    },
                ),
                egui::Event::PointerMoved(grip),
            ],
        );
        assert!(view.pending && view.layer_gesture.is_none());
        assert!(
            matches!(rx.recv().unwrap(), Job::Apply(Request::Layer { id: target_id, edit: LayerEdit::Rotate { radians } })
            if target_id == id && radians == snapped.radians)
        );
        assert!(
            rx.try_recv().is_err(),
            "multipass submits exactly one rotation"
        );
        view.receive(&ctx, Err("rotation failed".into()));
        assert_eq!(view.selected_layer.as_deref(), Some(id.as_str()));
        assert!(view.pending_layer_selection.is_none());
    }

    #[test]
    fn layer_canvas_resize_refreshes_modifiers_and_commits_once_after_threshold() {
        let ctx = egui::Context::default();
        let mut value = presented(false);
        value.pixels = Arc::new(RgbaImage::new(200, 100));
        let document = Arc::make_mut(&mut value.document);
        document.width = 200.;
        document.height = 100.;
        let id = document
            .create_closed_shape(ClosedShapeCreate {
                shape: ClosedShapeKind::Rectangle,
                start: Point { x: 20., y: 20. },
                end: Point { x: 80., y: 60. },
                style: ElementStyle::default(),
                opacity: 100.,
            })
            .unwrap();
        let preview = egui::Rect::from_min_size(egui::pos2(100., 100.), egui::vec2(100., 50.));
        let grip = egui::pos2(110., 110.);
        let target = egui::pos2(100., 105.);
        let button = |pos, pressed, modifiers| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers,
        };
        let escape = egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let mut view = View::default();
        view.receive(&ctx, Ok(value));
        view.pending = false;
        view.select_layer(Some(id.clone()));
        view.output = Some((view.texture.as_ref().unwrap().clone(), 123));
        let (tx, rx) = mpsc::channel();

        layer_frame(
            &ctx,
            &mut view,
            &tx,
            preview,
            true,
            egui::Modifiers::NONE,
            vec![button(grip, true, egui::Modifiers::NONE)],
        );
        assert!(matches!(
            view.layer_gesture.as_ref().map(|gesture| &gesture.kind),
            Some(LayerGestureKind::Resize {
                handle: ResizeHandle::Nw,
                ..
            })
        ));
        layer_frame(
            &ctx,
            &mut view,
            &tx,
            preview,
            true,
            egui::Modifiers::NONE,
            vec![button(grip, false, egui::Modifiers::NONE)],
        );
        assert!(rx.try_recv().is_err() && view.output.is_some() && !view.pending);

        layer_frame(
            &ctx,
            &mut view,
            &tx,
            preview,
            true,
            egui::Modifiers::NONE,
            vec![
                button(grip, true, egui::Modifiers::NONE),
                egui::Event::PointerMoved(target),
            ],
        );
        let free = match &view.layer_gesture.as_ref().unwrap().kind {
            LayerGestureKind::Resize {
                outline,
                lock_aspect,
                ..
            } => {
                assert!(!lock_aspect);
                *outline
            }
            _ => panic!("resize grip must beat body movement"),
        };
        layer_frame(
            &ctx,
            &mut view,
            &tx,
            preview,
            true,
            egui::Modifiers {
                shift: true,
                ..Default::default()
            },
            vec![],
        );
        match &view.layer_gesture.as_ref().unwrap().kind {
            LayerGestureKind::Resize {
                outline,
                lock_aspect,
                ..
            } => {
                assert!(*lock_aspect);
                assert_ne!(*outline, free, "stationary Shift refreshes resize preview");
            }
            _ => unreachable!(),
        }
        layer_frame(
            &ctx,
            &mut view,
            &tx,
            preview,
            true,
            egui::Modifiers::NONE,
            vec![escape.clone(), button(target, false, egui::Modifiers::NONE)],
        );
        assert!(rx.try_recv().is_err() && view.layer_gesture.is_none());

        layer_frame(
            &ctx,
            &mut view,
            &tx,
            preview,
            true,
            egui::Modifiers::NONE,
            vec![
                button(grip, true, egui::Modifiers::NONE),
                egui::Event::PointerMoved(target),
                button(target, false, egui::Modifiers::NONE),
            ],
        );
        assert!(view.pending && view.output.is_none());
        assert!(matches!(
            rx.recv().unwrap(),
            Job::Apply(Request::Layer {
                id: target_id,
                edit: LayerEdit::Resize {
                    handle: ResizeHandle::Nw,
                    current: Point { x: 0., y: 10. },
                    display_scale: 0.5,
                    lock_aspect: false,
                },
            }) if target_id == id
        ));
        assert!(rx.try_recv().is_err(), "multipass commits one resize");
        view.receive(&ctx, Err("resize failed".into()));
        assert_eq!(view.selected_layer.as_deref(), Some(id.as_str()));
        assert!(view.pending_layer_selection.is_none());
    }

    #[test]
    fn freehand_samples_all_frame_moves_once_and_ignores_hover_and_release_positions() {
        let ctx = egui::Context::default();
        let mut view = View::default();
        let mut value = presented(false);
        value.pixels = Arc::new(RgbaImage::new(1280, 640));
        view.receive(&ctx, Ok(value));
        view.draw_shape = DrawShape::Freehand;
        view.new_annotation_style = ElementStyle {
            color: "#123456".into(),
            fill: Some("#abcdef".into()),
            stroke_width: 13.,
            stroke_enabled: Some(false),
            ..ElementStyle::default()
        };
        view.new_annotation_opacity = 37.;
        view.canvas = [9., 7.]; // Unapplied fields must not affect the 0.5× mapping.
        let pixels = view.presented.as_ref().unwrap().pixels.clone();
        let (tx, rx) = mpsc::channel();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000., 800.));
        let preview = egui::Rect::from_min_size(egui::pos2(100., 100.), egui::vec2(640., 320.));
        let frame = |view: &mut View, events| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    events,
                    ..Default::default()
                },
                |_| {
                    let mut ui = egui::Ui::new(
                        ctx.clone(),
                        egui::Id::unique("freehand-test"),
                        egui::UiBuilder::new().max_rect(screen),
                    );
                    show_shape(&mut ui, test_tokens(), view, &tx, screen, preview, false);
                    if ctx.current_pass_index() == 0 {
                        ctx.request_discard("exercise multipass event replay");
                    }
                },
            );
            assert!(output.platform_output.num_completed_passes >= 2);
            output.textures_delta.clear();
        };
        let moved = |x, y| egui::Event::PointerMoved(egui::pos2(x, y));
        let button = |x, y, pressed| egui::Event::PointerButton {
            pos: egui::pos2(x, y),
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        frame(&mut view, vec![]);
        frame(
            &mut view,
            vec![
                moved(650., 430.),
                moved(120., 130.),
                button(120., 130., true),
            ],
        );
        frame(
            &mut view,
            vec![
                moved(121., 130.),
                moved(121.5, 130.),
                moved(121.5, 131.),
                moved(126., 136.),
            ],
        );
        assert_eq!(
            view.freehand_points,
            vec![
                Point { x: 40., y: 60. },
                Point { x: 43., y: 60. },
                Point { x: 52., y: 72. }
            ]
        );
        assert!(rx.try_recv().is_err() && !view.unsaved());
        assert!(Arc::ptr_eq(
            &pixels,
            &view.presented.as_ref().unwrap().pixels
        ));
        frame(
            &mut view,
            vec![
                moved(96., 144.),
                button(700., 400., false),
                moved(800., 500.),
            ],
        );
        let Job::Apply(Request::CreateFreehandPath { create }) = rx.try_recv().unwrap() else {
            panic!()
        };
        assert_eq!(
            create.points,
            vec![
                Point { x: 40., y: 60. },
                Point { x: 43., y: 60. },
                Point { x: 52., y: 72. },
                Point { x: -8., y: 88. }
            ]
        );
        assert_eq!(create.style, view.new_annotation_style);
        assert_eq!(create.opacity, 37.);
        assert!(
            rx.try_recv().is_err() && view.freehand_points.is_empty() && view.shape_drag.is_none()
        );

        view.pending = false;
        view.new_annotation_opacity = 0.;
        frame(&mut view, vec![moved(120., 130.), button(120., 130., true)]);
        frame(&mut view, vec![button(120., 130., false)]);
        let Job::Apply(Request::CreateFreehandPath { create }) = rx.try_recv().unwrap() else {
            panic!()
        };
        assert_eq!(create.points, vec![Point { x: 40., y: 60. }]);
        assert_eq!(create.style, view.new_annotation_style);
        assert_eq!(create.opacity, 0.);
        view.pending = false;
        frame(&mut view, vec![moved(140., 150.), button(140., 150., true)]);
        view.request_close();
        assert!(view.freehand_points.is_empty() && view.shape_drag.is_none());
        frame(&mut view, vec![button(140., 150., false)]);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn open_shape_requests_keep_axis_lines_and_cancel_scale_dependent_arrow_stubs() {
        let start = Point { x: -7.25, y: 13.5 };
        for end in [start, Point { x: 4., y: 13.5 }, Point { x: -7.25, y: -2. }] {
            let Some(Request::CreateOpenShape { create }) =
                DrawShape::Line.request(start, end, 0.5, &ElementStyle::default(), 100.)
            else {
                panic!("lines must not use closed-shape or arrow minimum rules")
            };
            assert_eq!(create.shape, OpenShapeKind::Line);
            assert_eq!((create.start, create.end), (start, end));
        }
        for (scale, threshold) in [(0.5, 6.), (1., 3.), (4., 1.5)] {
            let below = Point {
                x: start.x - threshold + 0.001,
                y: start.y,
            };
            let at = Point {
                x: start.x - threshold,
                y: start.y,
            };
            assert!(
                DrawShape::Arrow
                    .request(start, below, scale, &ElementStyle::default(), 100.)
                    .is_none()
            );
            let Some(Request::CreateOpenShape { create }) =
                DrawShape::Arrow.request(start, at, scale, &ElementStyle::default(), 100.)
            else {
                panic!("an arrow at the inclusive boundary must commit")
            };
            assert_eq!(create.shape, OpenShapeKind::Arrow);
            assert_eq!((create.start, create.end), (start, at));
        }
    }

    #[test]
    fn new_annotation_defaults_are_host_local_and_survive_tools_and_failures() {
        let ctx = egui::Context::default();
        let mut view = View::default();
        view.receive(&ctx, Ok(presented(false)));
        let pixels = view.presented.as_ref().unwrap().pixels.clone();
        let document = view.presented.as_ref().unwrap().document.clone();
        let texture = ctx.load_texture(
            "defaults-output",
            egui::ColorImage::new([1, 1], vec![egui::Color32::WHITE]),
            egui::TextureOptions::default(),
        );
        view.output = Some((texture, 4));
        let (tx, rx) = mpsc::channel::<Job>();

        view.new_annotation_style = ElementStyle {
            color: "#123456".into(),
            fill: None,
            stroke_width: 13.,
            stroke_enabled: Some(true),
            drop_shadow: Some(true),
            drop_shadow_style: Some(DropShadowStyle {
                color: "#2468ac".into(),
                opacity: 61.,
                blur: 9.,
                offset_x: -23.,
                offset_y: 17.,
                extra: Default::default(),
            }),
            ..ElementStyle::default()
        };
        view.new_annotation_opacity = 37.;
        assert!(rx.try_recv().is_err());
        assert!(view.output.is_some() && !view.unsaved());
        assert!(Arc::ptr_eq(
            &pixels,
            &view.presented.as_ref().unwrap().pixels
        ));
        assert!(Arc::ptr_eq(
            &document,
            &view.presented.as_ref().unwrap().document
        ));

        view.activate_tool(Section::Draw, Some(DrawShape::Line));
        view.activate_tool(Section::Draw, Some(DrawShape::Star));
        view.pending = true;
        view.receive(&ctx, Err("creation failed".into()));
        assert_eq!(view.new_annotation_style.color, "#123456");
        assert_eq!(view.new_annotation_style.fill, None);
        assert_eq!(view.new_annotation_style.stroke_width, 13.);
        assert_eq!(view.new_annotation_opacity, 37.);
        assert_eq!(view.draw_shape, DrawShape::Star);
        assert!(rx.try_recv().is_err());

        let request = DrawShape::Line
            .request(
                Point { x: 1., y: 2. },
                Point { x: 8., y: 9. },
                1.,
                &view.new_annotation_style,
                view.new_annotation_opacity,
            )
            .unwrap();
        let Request::CreateOpenShape { create } = request else {
            panic!()
        };
        assert_eq!(create.style.fill, None);
        assert_eq!(create.style.stroke_enabled, Some(true));
        assert_eq!(create.style.drop_shadow, Some(true));
        assert_eq!(
            create.style.drop_shadow_style.as_ref().unwrap().offset_x,
            -23.
        );
        assert_eq!(
            create.style.drop_shadow_style.as_ref().unwrap().offset_y,
            17.
        );
        assert_eq!(create.opacity, 37.);
        drop(tx);
    }

    #[test]
    fn wand_click_maps_once_and_rejects_busy_intercepted_and_off_image_input() {
        let ctx = egui::Context::default();
        let mut view = View::default();
        let mut value = presented(false);
        value.pixels = Arc::new(RgbaImage::new(1200, 600));
        view.receive(&ctx, Ok(value));
        view.draw_shape = DrawShape::Wand;
        view.wand_tolerance = 47.;
        view.wand_contiguous = false;
        let (tx, rx) = mpsc::channel();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000., 700.));
        // This is the already zoomed and panned viewport result. Mapping must use
        // it directly rather than duplicating shared viewport or image math.
        let preview = egui::Rect::from_min_size(egui::pos2(220., 140.), egui::vec2(600., 300.));
        let frame = |view: &mut View, events, intercepted| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    events,
                    ..Default::default()
                },
                |_| {
                    let mut ui = egui::Ui::new(
                        ctx.clone(),
                        egui::Id::unique("wand-test"),
                        egui::UiBuilder::new().max_rect(screen),
                    );
                    show_shape(
                        &mut ui,
                        test_tokens(),
                        view,
                        &tx,
                        screen,
                        preview,
                        intercepted,
                    );
                    if ctx.current_pass_index() == 0 {
                        ctx.request_discard("exercise wand multipass event replay");
                    }
                },
            );
            assert!(output.platform_output.num_completed_passes >= 2);
            output.textures_delta.clear();
        };
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let click = egui::pos2(370., 215.);
        frame(&mut view, vec![], false);
        frame(
            &mut view,
            vec![egui::Event::PointerMoved(click), button(click, true)],
            false,
        );
        assert!(rx.try_recv().is_err());
        frame(&mut view, vec![button(click, false)], false);
        assert!(matches!(
            rx.try_recv().unwrap(),
            Job::Apply(Request::RemoveImageBackground {
                point: Point { x: 300., y: 150. },
                tolerance: 47.,
                contiguous: false,
            })
        ));
        assert!(view.pending && rx.try_recv().is_err());

        // A stale extra click while the worker is busy cannot enqueue a second edit.
        frame(
            &mut view,
            vec![button(click, true), button(click, false)],
            false,
        );
        assert!(rx.try_recv().is_err());
        view.pending = false;
        let outside = egui::pos2(100., 100.);
        frame(
            &mut view,
            vec![
                egui::Event::PointerMoved(outside),
                button(outside, true),
                button(outside, false),
            ],
            false,
        );
        frame(
            &mut view,
            vec![button(click, true), button(click, false)],
            true,
        );
        assert!(rx.try_recv().is_err() && !view.pending);
    }

    #[test]
    fn background_brush_samples_frames_release_and_click_once_and_cancels_invalid_gestures() {
        let ctx = egui::Context::default();
        let mut view = View::default();
        let mut value = presented(false);
        value.pixels = Arc::new(RgbaImage::new(1200, 600));
        value.document = Arc::new(Document::new_capture("fixture", 1200., 600., None));
        view.receive(&ctx, Ok(value));
        view.draw_shape = DrawShape::Erase;
        view.brush_size = 28.;
        view.brush_softness = 18.;
        view.drawing_preview = Some(drawing_preview::State::new(
            ctx.clone(),
            egui::ViewportId::ROOT,
        ));
        let (tx, rx) = mpsc::channel();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000., 700.));
        let preview = egui::Rect::from_min_size(egui::pos2(220., 140.), egui::vec2(600., 300.));
        let frame = |view: &mut View, events, intercepted| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    events,
                    ..Default::default()
                },
                |_| {
                    let mut ui = egui::Ui::new(
                        ctx.clone(),
                        egui::Id::unique("background-brush-test"),
                        egui::UiBuilder::new().max_rect(screen),
                    );
                    show_shape(
                        &mut ui,
                        test_tokens(),
                        view,
                        &tx,
                        screen,
                        preview,
                        intercepted,
                    );
                    if ctx.current_pass_index() == 0 {
                        ctx.request_discard("exercise brush multipass event replay");
                    }
                },
            );
            assert!(output.platform_output.num_completed_passes >= 2);
            output.textures_delta.clear();
        };
        let moved = |x, y| egui::Event::PointerMoved(egui::pos2(x, y));
        let button = |x, y, pressed| egui::Event::PointerButton {
            pos: egui::pos2(x, y),
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };

        frame(&mut view, vec![], false);
        frame(
            &mut view,
            vec![moved(250., 170.), button(250., 170., true)],
            false,
        );
        let Job::DrawingPreview {
            request:
                Request::PaintImageBackground {
                    points,
                    size,
                    softness,
                    mode,
                },
            epoch,
            reply,
        } = rx.try_recv().unwrap()
        else {
            panic!("expected live brush preview")
        };
        assert_eq!(points, vec![Point { x: 60., y: 60. }]);
        assert_eq!((size, softness, mode), (28., 18., BrushMode::Erase));
        assert!(
            !view.pending,
            "preview does not publish an edit or block gesture input"
        );
        frame(&mut view, vec![moved(260., 180.), moved(100., 100.)], false);
        frame(
            &mut view,
            vec![moved(280., 190.), button(290., 200., false)],
            false,
        );
        let Job::Apply(Request::PaintImageBackground {
            points,
            size,
            softness,
            mode,
        }) = rx.try_recv().unwrap()
        else {
            panic!()
        };
        assert_eq!(
            points,
            vec![
                Point { x: 60., y: 60. },
                Point { x: 80., y: 80. },
                Point { x: 140., y: 120. },
            ]
        );
        assert_eq!((size, softness, mode), (28., 18., BrushMode::Erase));
        assert!(
            view.pending && rx.try_recv().is_err(),
            "multipass submits once"
        );
        reply
            .send(drawing_preview::Reply {
                epoch,
                pixels: Ok(Arc::new(RgbaImage::new(1200, 600))),
            })
            .unwrap();
        view.drawing_preview.as_mut().unwrap().receive(&tx);
        assert!(
            view.drawing_preview.as_ref().unwrap().texture.is_none(),
            "late pre-release pixels cannot replace committed state"
        );
        assert!(
            rx.try_recv().is_err(),
            "release discards the queued preview"
        );
        view.drawing_preview = None;

        view.pending = false;
        view.draw_shape = DrawShape::Restore;
        frame(&mut view, vec![button(250., 170., true)], false);
        frame(&mut view, vec![button(250., 170., false)], false);
        assert!(matches!(
            rx.try_recv().unwrap(),
            Job::Apply(Request::PaintImageBackground {
                points,
                mode: BrushMode::Restore,
                ..
            }) if points == vec![Point { x: 60., y: 60. }; 2]
        ));

        view.pending = false;
        frame(
            &mut view,
            vec![button(250., 170., true), button(250., 170., false)],
            false,
        );
        assert!(
            matches!(rx.try_recv().unwrap(), Job::Apply(Request::PaintImageBackground { points, .. })
            if points == vec![Point { x: 60., y: 60. }; 2])
        );

        view.pending = false;
        for (events, intercepted) in [
            (
                vec![button(100., 100., true), button(100., 100., false)],
                false,
            ),
            (
                vec![button(250., 170., true), button(250., 170., false)],
                true,
            ),
        ] {
            frame(&mut view, events, intercepted);
            assert!(rx.try_recv().is_err() && view.brush_points.is_empty());
        }
        view.pending = true;
        frame(
            &mut view,
            vec![button(250., 170., true), button(250., 170., false)],
            false,
        );
        assert!(rx.try_recv().is_err() && view.brush_points.is_empty());
        view.pending = false;
        frame(&mut view, vec![button(250., 170., true)], false);
        assert!(!view.brush_points.is_empty());
        view.cancel_drawing();
        frame(&mut view, vec![button(250., 170., false)], false);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn brush_ring_replaces_the_cursor_over_images_like_shipping() {
        let ctx = egui::Context::default();
        let mut view = View::default();
        let mut value = presented(false);
        value.pixels = Arc::new(RgbaImage::new(1200, 600));
        value.document = Arc::new(Document::new_capture("fixture", 1200., 600., None));
        view.receive(&ctx, Ok(value));
        view.draw_shape = DrawShape::Erase;
        view.brush_size = 28.;
        let (tx, _rx) = mpsc::channel();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000., 700.));
        let preview = egui::Rect::from_min_size(egui::pos2(220., 140.), egui::vec2(600., 300.));
        let frame = |view: &mut View, x: f32, y: f32, modifiers: egui::Modifiers| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    events: vec![
                        egui::Event::ModifiersChanged(modifiers),
                        egui::Event::PointerMoved(egui::pos2(x, y)),
                    ],
                    ..Default::default()
                },
                |_| {
                    let mut ui = egui::Ui::new(
                        ctx.clone(),
                        egui::Id::unique("brush-ring-test"),
                        egui::UiBuilder::new().max_rect(screen),
                    );
                    show_shape(&mut ui, test_tokens(), view, &tx, screen, preview, false);
                },
            );
            output.textures_delta.clear();
            // Filled disc radius: 28 document px at 0.5× is a 14 pt ring.
            let rings = output
                .shapes
                .iter()
                .filter(|shape| {
                    matches!(&shape.shape, egui::epaint::Shape::Circle(circle)
                        if circle.center == egui::pos2(x, y) && circle.radius == 7.)
                })
                .count();
            (
                output.platform_output.cursor_icon,
                rings,
                output.shapes.len(),
            )
        };
        // egui hovers a widget from the previous pass's layout.
        frame(&mut view, 250., 170., Default::default());
        // Over the capture image: no system cursor, the ring at the pointer
        // (fill, halo, inset and border for Erase).
        let (cursor, rings, _) = frame(&mut view, 250., 170., Default::default());
        assert_eq!((cursor, rings), (egui::CursorIcon::None, 1));
        // Off the image, still on the canvas: `not-allowed`, no ring.
        let (cursor, rings, _) = frame(&mut view, 100., 100., Default::default());
        assert_eq!((cursor, rings), (egui::CursorIcon::NotAllowed, 0));
        // Pan-ready (Cmd/Ctrl) hides the ring.
        let (cursor, rings, _) = frame(&mut view, 250., 170., egui::Modifiers::COMMAND);
        assert_ne!(cursor, egui::CursorIcon::None);
        assert_eq!(rings, 0);
        // Restore dashes the border into many segments over the accent fill.
        let (_, erase_rings, erase_shapes) = frame(&mut view, 250., 170., Default::default());
        view.draw_shape = DrawShape::Restore;
        let (cursor, rings, restore_shapes) = frame(&mut view, 250., 170., Default::default());
        assert_eq!((cursor, rings, erase_rings), (egui::CursorIcon::None, 1, 1));
        assert!(restore_shapes > erase_shapes + 4);
    }

    #[test]
    fn preview_mesh_triangulates_concave_notches_without_painting_across_them() {
        // C shape: 6×5 outer rectangle minus a 4×3 notch. A convex fan overfills it.
        let points = [
            (0., 0.),
            (6., 0.),
            (6., 1.),
            (2., 1.),
            (2., 4.),
            (6., 4.),
            (6., 5.),
            (0., 5.),
        ];
        let mesh = polygon_mesh(
            points.map(|(x, y)| egui::pos2(x, y)).to_vec(),
            egui::Color32::RED,
        );
        let mut area = 0.;
        for indices in mesh.indices.chunks_exact(3) {
            let [a, b, c] = indices
                .try_into()
                .map(|ids: [u32; 3]| ids.map(|id| mesh.vertices[id as usize].pos))
                .unwrap();
            area += ((b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)).abs() / 2.;
            let center = (a.to_vec2() + b.to_vec2() + c.to_vec2()) / 3.;
            assert!(!(center.x > 2. && center.y > 1. && center.y < 4.));
        }
        assert_eq!(area, 18.);
        assert!(
            polygon_mesh(Vec::new(), egui::Color32::RED)
                .indices
                .is_empty()
        );
    }

    #[test]
    fn degenerate_polygon_previews_have_no_orphan_gpu_vertices() {
        for kind in [
            ClosedShapeKind::Triangle,
            ClosedShapeKind::Diamond,
            ClosedShapeKind::Star,
        ] {
            let start = Point { x: 50., y: 30. };
            for end in [start, Point { x: 50., y: 90. }, Point { x: 120., y: 30. }] {
                let points = kind
                    .polygon(start, end)
                    .unwrap()
                    .into_iter()
                    .map(|point| egui::pos2(point.x as f32, point.y as f32))
                    .collect();
                let mesh = polygon_mesh(points, egui::Color32::RED);
                assert!(mesh.indices.is_empty());
                assert!(mesh.vertices.is_empty());
            }
        }
    }

    #[test]
    fn shape_drag_maps_reverse_and_outside_points_without_committing_until_release() {
        let ctx = egui::Context::default();
        let mut view = View::default();
        let mut value = presented(false);
        value.pixels = Arc::new(RgbaImage::new(1280, 640));
        view.receive(&ctx, Ok(value));
        view.new_annotation_style = ElementStyle {
            color: "#123456".into(),
            fill: Some("#abcdef".into()),
            stroke_width: 13.,
            stroke_enabled: Some(true),
            ..ElementStyle::default()
        };
        view.new_annotation_opacity = 37.;
        let pixels = view.presented.as_ref().unwrap().pixels.clone();
        let document = view.presented.as_ref().unwrap().document.clone();
        view.canvas = [999., 777.]; // Never use unapplied canvas fields for mapping.
        let (tx, rx) = mpsc::channel();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000., 800.));
        let preview = egui::Rect::from_min_size(egui::pos2(100., 100.), egui::vec2(640., 320.));
        let frame = |view: &mut View, events| {
            ctx.begin_pass(egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..Default::default()
            });
            let mut ui = egui::Ui::new(
                ctx.clone(),
                egui::Id::unique("shape-test"),
                egui::UiBuilder::new().max_rect(screen),
            );
            show_shape(&mut ui, test_tokens(), view, &tx, screen, preview, false);
            let mut output = ctx.end_pass();
            output.textures_delta.clear();
        };
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        frame(&mut view, vec![]);
        for kind in [
            DrawShape::Rectangle,
            DrawShape::Ellipse,
            DrawShape::Triangle,
            DrawShape::Diamond,
            DrawShape::Star,
            DrawShape::Line,
            DrawShape::Arrow,
        ] {
            view.draw_shape = kind;
            let start = egui::pos2(500., 300.);
            let end = egui::pos2(60., 150.); // Outside preview, not clamped like crop.
            frame(
                &mut view,
                vec![egui::Event::PointerMoved(start), button(start, true)],
            );
            frame(&mut view, vec![egui::Event::PointerMoved(end)]);
            assert_eq!(
                view.shape_drag,
                Some((Point { x: 800., y: 400. }, Point { x: -80., y: 100. }))
            );
            assert!(rx.try_recv().is_err() && !view.unsaved() && !view.pending);
            assert!(Arc::ptr_eq(
                &pixels,
                &view.presented.as_ref().unwrap().pixels
            ));
            assert!(Arc::ptr_eq(
                &document,
                &view.presented.as_ref().unwrap().document
            ));
            frame(&mut view, vec![button(end, false)]);
            let (start, end, style, opacity) = match rx.try_recv().unwrap() {
                Job::Apply(Request::CreateClosedShape { create }) => {
                    assert_eq!(kind.closed_kind(), Some(create.shape));
                    (create.start, create.end, create.style, create.opacity)
                }
                Job::Apply(Request::CreateOpenShape { create }) => {
                    assert_eq!(
                        kind,
                        if create.shape == OpenShapeKind::Line {
                            DrawShape::Line
                        } else {
                            DrawShape::Arrow
                        }
                    );
                    (create.start, create.end, create.style, create.opacity)
                }
                _ => panic!("expected exactly one creation command"),
            };
            assert_eq!(start, Point { x: 800., y: 400. });
            assert_eq!(end, Point { x: -80., y: 100. });
            assert_eq!(style, view.new_annotation_style);
            assert_eq!(opacity, 37.);
            assert!(rx.try_recv().is_err() && view.shape_drag.is_none() && view.pending);
            assert_eq!(view.draw_shape, kind); // Creation does not switch back to another tool.
            view.pending = false;
        }
        view.draw_shape = DrawShape::Line;
        let point = egui::pos2(400., 250.);
        frame(
            &mut view,
            vec![egui::Event::PointerMoved(point), button(point, true)],
        );
        frame(&mut view, vec![button(point, false)]);
        let Job::Apply(Request::CreateOpenShape { create }) = rx.try_recv().unwrap() else {
            panic!("a line click retains the shipping zero-length layer")
        };
        assert_eq!(create.start, create.end);
        assert_eq!(create.shape, OpenShapeKind::Line);
        view.pending = false;
        view.draw_shape = DrawShape::Arrow;
        let short = egui::pos2(402., 250.); // Four document pixels at 0.5× is below six.
        frame(
            &mut view,
            vec![egui::Event::PointerMoved(point), button(point, true)],
        );
        frame(&mut view, vec![egui::Event::PointerMoved(short)]);
        frame(&mut view, vec![button(short, false)]);
        assert!(rx.try_recv().is_err() && !view.pending && view.shape_drag.is_none());
        view.draw_shape = DrawShape::Ellipse;
        let start = egui::pos2(300., 200.);
        let end = egui::pos2(300., 350.);
        frame(
            &mut view,
            vec![egui::Event::PointerMoved(start), button(start, true)],
        );
        frame(&mut view, vec![egui::Event::PointerMoved(end)]);
        frame(&mut view, vec![button(end, false)]);
        assert!(rx.try_recv().is_err() && !view.pending); // Zero width must not create pixels.
        frame(
            &mut view,
            vec![egui::Event::PointerMoved(start), button(start, true)],
        );
        frame(
            &mut view,
            vec![egui::Event::PointerMoved(egui::pos2(400., 350.))],
        );
        view.request_close();
        assert!(view.shape_drag.is_none());
        frame(&mut view, vec![button(end, false)]);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn shape_drag_keeps_press_mapping_when_live_preview_grows_the_canvas() {
        // A drawing preview that extends past the image grows the rendered
        // canvas, so the viewport re-lays out taller and re-centered while the
        // committed document keeps its size. The stationary pointer must keep
        // mapping into the committed document, not the grown preview rect.
        let ctx = egui::Context::default();
        let mut view = View::default();
        let mut value = presented(false);
        value.pixels = Arc::new(RgbaImage::new(640, 360));
        view.receive(&ctx, Ok(value));
        let (tx, rx) = mpsc::channel();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000., 800.));
        let committed = egui::Rect::from_min_size(egui::pos2(64., 211.), egui::vec2(640., 360.));
        let grown = egui::Rect::from_min_size(egui::pos2(64., 148.), egui::vec2(640., 486.));
        let frame = |view: &mut View, preview, events| {
            ctx.begin_pass(egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..Default::default()
            });
            let mut ui = egui::Ui::new(
                ctx.clone(),
                egui::Id::unique("shape-grow-test"),
                egui::UiBuilder::new().max_rect(screen),
            );
            show_shape(&mut ui, test_tokens(), view, &tx, screen, preview, false);
            let mut output = ctx.end_pass();
            output.textures_delta.clear();
        };
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        frame(&mut view, committed, vec![]);
        for kind in [DrawShape::Rectangle, DrawShape::Freehand] {
            view.draw_shape = kind;
            let start = egui::pos2(124., 622.);
            let end = egui::pos2(184., 692.);
            frame(
                &mut view,
                committed,
                vec![egui::Event::PointerMoved(start), button(start, true)],
            );
            frame(&mut view, committed, vec![egui::Event::PointerMoved(end)]);
            let expected = view.shape_drag.unwrap();
            let near = |point: Point, x: f64, y: f64| (point.x - x).hypot(point.y - y) < 1e-6;
            assert!(near(expected.0, 60., 411.) && near(expected.1, 120., 481.));
            // Pointer holds still while the grown preview re-centers the canvas.
            frame(&mut view, grown, vec![]);
            frame(&mut view, grown, vec![egui::Event::PointerMoved(end)]);
            assert_eq!(view.shape_drag, Some(expected));
            frame(&mut view, grown, vec![button(end, false)]);
            match rx.try_recv().unwrap() {
                Job::Apply(Request::CreateClosedShape { create }) => {
                    assert_eq!((create.start, create.end), expected);
                }
                Job::Apply(Request::CreateFreehandPath { create }) => {
                    assert_eq!(create.points, vec![expected.0, expected.1]);
                }
                _ => panic!("expected exactly one creation command"),
            }
            assert!(rx.try_recv().is_err() && view.shape_drag.is_none());
            view.pending = false;
        }
    }

    #[test]
    fn shape_worker_selects_new_id_invalidates_output_and_preserves_draft_pixels() {
        let (data, id) = fixture();
        let ctx = egui::Context::default();
        let editor = Editor::open(
            &ctx,
            data.path().join("history"),
            id.clone(),
            data.path().join("exports"),
            CaptureMode::Region,
            |_| unreachable!("copy was not requested"),
        );
        receive(&editor, &ctx);
        for request in [
            DrawShape::Line
                .request(
                    Point { x: 6., y: 2. },
                    Point { x: 0., y: 2. },
                    1.,
                    &ElementStyle::default(),
                    100.,
                )
                .unwrap(),
            DrawShape::Arrow
                .request(
                    Point { x: 6., y: 2. },
                    Point { x: 0., y: 2. },
                    1.,
                    &ElementStyle::default(),
                    100.,
                )
                .unwrap(),
            Request::CreateFreehandPath {
                create: FreehandPathCreate {
                    points: vec![Point { x: 6., y: 2. }, Point { x: 0., y: 2. }],
                    style: ElementStyle::default(),
                    opacity: 100.,
                },
            },
        ] {
            let thin_arrow = matches!(&request, Request::CreateOpenShape { create } if create.shape == OpenShapeKind::Arrow);
            fake_output(&editor);
            editor.view.lock().unwrap().submit(&editor.tx, request);
            receive(&editor, &ctx);
            {
                let view = editor.view.lock().unwrap();
                assert!(view.error.is_none() && view.unsaved());
                assert!(view.output.is_none());
                let frame = view.presented.as_ref().unwrap();
                assert_eq!(frame.document.elements.len(), 2);
                let created = frame.document.elements.last().unwrap();
                assert_eq!(
                    view.selected_layer.as_deref(),
                    Some(created.base().id.as_str())
                );
                assert!(view.annotation.is_some());
                if !thin_arrow {
                    assert_eq!(frame.pixels.get_pixel(3, 2).0, [255, 59, 92, 255]);
                }
            }
            editor
                .view
                .lock()
                .unwrap()
                .submit(&editor.tx, Request::Undo);
            receive(&editor, &ctx);
            assert_eq!(
                editor
                    .view
                    .lock()
                    .unwrap()
                    .presented
                    .as_ref()
                    .unwrap()
                    .document
                    .elements
                    .len(),
                1
            );
        }
        fake_output(&editor);
        editor.view.lock().unwrap().submit(
            &editor.tx,
            Request::CreateClosedShape {
                create: ClosedShapeCreate {
                    shape: ClosedShapeKind::Rectangle,
                    start: Point { x: 6., y: 3. },
                    end: Point { x: 2., y: 1. },
                    style: ElementStyle {
                        drop_shadow: Some(true),
                        ..ElementStyle::default()
                    },
                    opacity: 100.,
                },
            },
        );
        receive(&editor, &ctx);
        let (layer, pixels) = {
            let view = editor.view.lock().unwrap();
            assert!(view.error.is_none() && view.unsaved());
            assert!(view.output.is_none());
            let frame = view.presented.as_ref().unwrap();
            assert_eq!(frame.document.elements.len(), 2);
            let layer = frame.document.elements.last().unwrap().base().id.clone();
            assert_eq!(view.selected_layer.as_deref(), Some(layer.as_str()));
            assert_eq!(frame.pixels.get_pixel(3, 2).0, [255, 59, 92, 255]);
            (layer, frame.pixels.clone())
        };
        assert!(!data.path().join("editor-drafts").exists());
        editor
            .view
            .lock()
            .unwrap()
            .submit(&editor.tx, Request::Undo);
        receive(&editor, &ctx);
        assert_eq!(
            editor.view.lock().unwrap().selected_layer.as_deref(),
            Some("capture-background")
        );
        editor
            .view
            .lock()
            .unwrap()
            .submit(&editor.tx, Request::Redo);
        receive(&editor, &ctx);
        editor.flush(&ctx).unwrap();
        drop(editor);
        let reopened = EditorSession::open(OpenRequest {
            history_root: data.path().join("history"),
            drafts_root: data.path().join("editor-drafts"),
            artifact_id: id,
        })
        .unwrap();
        assert_eq!(
            reopened
                .snapshot()
                .document
                .elements
                .last()
                .unwrap()
                .base()
                .id,
            layer
        );
        assert_eq!(reopened.pixels(), pixels);
        assert!(!data.path().join("exports").exists());
    }

    #[test]
    fn crop_pointer_uses_presented_pixels_and_stays_transient_until_apply() {
        let ctx = egui::Context::default();
        let mut view = View::default();
        let mut value = presented(false);
        value.pixels = Arc::new(RgbaImage::new(1280, 640));
        value.document = Arc::new(Document::new_capture("fixture", 1280., 640., None));
        view.receive(&ctx, Ok(value));
        let pixels = view.presented.as_ref().unwrap().pixels.clone();
        let document = view.presented.as_ref().unwrap().document.clone();
        view.canvas = [999., 777.]; // Unapplied canvas fields are not preview dimensions.
        view.crop_previous = Some(view.crop);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000., 800.));
        let preview = egui::Rect::from_min_size(egui::pos2(100., 100.), egui::vec2(640., 320.));
        let frame = |view: &mut View, mut events: Vec<egui::Event>, shift| {
            events.insert(
                0,
                egui::Event::ModifiersChanged(egui::Modifiers {
                    shift,
                    ..Default::default()
                }),
            );
            ctx.begin_pass(egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..Default::default()
            });
            let mut ui = egui::Ui::new(
                ctx.clone(),
                egui::Id::unique("crop-test"),
                egui::UiBuilder::new().max_rect(screen),
            );
            show_crop(
                &mut ui,
                &crate::tokens::load()["dark-mustard"],
                view,
                screen,
                preview,
                false,
            );
            let mut output = ctx.end_pass();
            output.textures_delta.clear();
        };
        let button = |position, pressed, shift| egui::Event::PointerButton {
            pos: position,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers {
                shift,
                ..Default::default()
            },
        };
        let start = egui::pos2(500., 300.);
        let end = egui::pos2(200., 150.);
        frame(&mut view, vec![], false);
        // A click (on the comparison's Hide, say) is not a selection.
        frame(
            &mut view,
            vec![egui::Event::PointerMoved(start), button(start, true, false)],
            false,
        );
        frame(&mut view, vec![button(start, false, false)], false);
        frame(&mut view, vec![], false);
        assert!(view.crop_drag.is_none());
        assert_eq!(view.crop, [0., 0., 1280., 640.]);
        frame(
            &mut view,
            vec![egui::Event::PointerMoved(start), button(start, true, false)],
            false,
        );
        frame(&mut view, vec![egui::Event::PointerMoved(end)], false);
        assert_eq!(view.crop, [200., 100., 600., 300.]);
        frame(
            &mut view,
            vec![egui::Event::PointerMoved(egui::pos2(150., 200.))],
            true,
        );
        assert_eq!(view.crop, [100., 50., 700., 350.]); // Lock the live 2:1 ratio, not 1:1.
        frame(&mut view, vec![egui::Event::PointerMoved(end)], false);
        frame(&mut view, vec![button(end, false, false)], false);
        assert_eq!(view.crop, [200., 100., 600., 300.]);
        assert!(view.crop_drag.is_none());
        assert!(Arc::ptr_eq(
            &pixels,
            &view.presented.as_ref().unwrap().pixels
        ));
        assert!(Arc::ptr_eq(
            &document,
            &view.presented.as_ref().unwrap().document
        ));
        assert!(!view.unsaved() && !view.pending);
        view.cancel_crop();
        assert_eq!(view.crop, [0., 0., 1280., 640.]);

        // Pressing Shift before the first movement must initialize a square.
        view.crop_previous = Some(view.crop);
        frame(
            &mut view,
            vec![egui::Event::PointerMoved(end), button(end, true, true)],
            true,
        );
        let destination = egui::pos2(350., 250.);
        frame(
            &mut view,
            vec![egui::Event::PointerMoved(destination)],
            true,
        );
        assert_eq!(view.crop, [200., 100., 300., 300.]);
        view.crop_aspect = 2; // An explicit 4:3 preset wins over Shift.
        frame(&mut view, vec![], true);
        assert_eq!(view.crop, [200., 100., 300., 225.]);
        frame(&mut view, vec![button(destination, false, true)], true);
        view.cancel_crop();

        view.crop_previous = Some(view.crop);
        view.crop_aspect = 0;
        let outside = egui::pos2(300., 500.);
        frame(
            &mut view,
            vec![
                egui::Event::PointerMoved(outside),
                button(outside, true, false),
            ],
            false,
        );
        frame(&mut view, vec![egui::Event::PointerMoved(end)], false);
        assert_eq!(view.crop, [200., 100., 200., 540.]);
        // A new published frame clears the gesture and all stale crop coordinates.
        view.receive(&ctx, Ok(presented(true)));
        assert!(view.crop_previous.is_none() && view.crop_drag.is_none());
        assert_eq!(view.crop, [0., 0., 7., 3.]);
    }

    #[test]
    fn import_picker_defers_while_busy_and_ignores_cancelled_or_closing_results() {
        let ctx = egui::Context::default();
        let mut view = View::default();
        view.receive(&ctx, Ok(presented(false)));
        let pixels = view.presented.as_ref().unwrap().pixels.clone();
        let (jobs, queued) = mpsc::channel();
        let (selection, picked) = mpsc::channel();
        view.import_picker = Some(picked);
        selection
            .send(Some(vec![PathBuf::from("photo.png")]))
            .unwrap();
        view.pending = true;
        assert!(!view.receive_import(&jobs));
        assert!(queued.try_recv().is_err() && view.import_picker.is_some());
        view.pending = false;
        assert!(view.receive_import(&jobs));
        assert!(
            matches!(queued.recv().unwrap(), Job::Import { path, selected_id, point: None }
            if path == Path::new("photo.png") && selected_id.is_none())
        );
        assert!(view.pending && view.import_picker.is_none());
        assert!(Arc::ptr_eq(
            &pixels,
            &view.presented.as_ref().unwrap().pixels
        ));
        view.pending = false;
        let (selection, picked) = mpsc::channel();
        view.import_picker = Some(picked);
        selection.send(None).unwrap();
        assert!(view.receive_import(&jobs));
        assert!(!view.pending && !view.unsaved() && queued.try_recv().is_err());
        for closed in [false, true] {
            let (selection, picked) = mpsc::channel();
            view.import_picker = Some(picked);
            selection
                .send(Some(vec![PathBuf::from("stale.png")]))
                .unwrap();
            view.closed = closed;
            view.close_requested = !closed;
            assert!(!view.receive_import(&jobs));
            assert!(view.import_picker.is_none() && queued.try_recv().is_err());
        }
    }

    #[test]
    fn import_picker_imports_every_selected_image_through_the_drop_queue() {
        let ctx = egui::Context::default();
        let mut view = View::default();
        view.receive(&ctx, Ok(presented(false)));
        let (jobs, queued) = mpsc::channel();
        let (selection, picked) = mpsc::channel();
        view.import_picker = Some(picked);
        selection
            .send(Some(vec![
                PathBuf::from("first.png"),
                PathBuf::from("notes.txt"),
                PathBuf::from("second.JPG"),
            ]))
            .unwrap();
        assert!(view.receive_import(&jobs));
        assert!(
            matches!(queued.try_recv().unwrap(), Job::Import { path, point: None, .. }
            if path == Path::new("first.png"))
        );
        assert!(view.pending && queued.try_recv().is_err());
        // Later images wait for the previous import, then stack below it.
        assert!(!canvas::drain_drops(&mut view, &jobs));
        view.pending = false;
        assert!(canvas::drain_drops(&mut view, &jobs));
        assert!(
            matches!(queued.try_recv().unwrap(), Job::Import { path, point: None, .. }
            if path == Path::new("second.JPG"))
        );
        view.pending = false;
        assert!(!canvas::drain_drops(&mut view, &jobs) && view.drop.queue.is_empty());

        let (selection, picked) = mpsc::channel();
        view.import_picker = Some(picked);
        selection
            .send(Some(vec![PathBuf::from("notes.txt")]))
            .unwrap();
        assert!(view.receive_import(&jobs));
        assert!(!view.pending && queued.try_recv().is_err());
        assert_eq!(
            view.error.as_deref(),
            Some(captures_app::editor_session::DROP_UNSUPPORTED)
        );
    }

    #[test]
    fn import_converts_rgb_and_gray_profiles_without_changing_straight_alpha() {
        use image::ImageEncoder;
        use moxcms::{ColorProfile, ToneReprCurve};
        let data = tempfile::tempdir().unwrap();
        let path = data.path().join("profile.data");
        let mut linear = ColorProfile::new_srgb();
        linear.cicp = None;
        linear.red_trc = Some(ToneReprCurve::Parametric(vec![1.]));
        linear.green_trc = linear.red_trc.clone();
        linear.blue_trc = linear.red_trc.clone();
        let profile = linear.encode().unwrap();
        let samples = [64, 128, 192, 73, 192, 32, 8, 0];
        fn write(mut encoder: impl ImageEncoder, profile: Vec<u8>, samples: &[u8]) {
            encoder.set_icc_profile(profile).unwrap();
            encoder
                .write_image(samples, 2, 1, image::ExtendedColorType::Rgba8)
                .unwrap();
        }
        for format in [ImageFormat::Png, ImageFormat::WebP, ImageFormat::Tiff] {
            let mut encoded = Cursor::new(Vec::new());
            match format {
                ImageFormat::Png => write(
                    image::codecs::png::PngEncoder::new(&mut encoded),
                    profile.clone(),
                    &samples,
                ),
                ImageFormat::WebP => write(
                    image::codecs::webp::WebPEncoder::new_lossless(&mut encoded),
                    profile.clone(),
                    &samples,
                ),
                ImageFormat::Tiff => write(
                    image::codecs::tiff::TiffEncoder::new(&mut encoded),
                    profile.clone(),
                    &samples,
                ),
                _ => unreachable!(),
            }
            fs::write(&path, encoded.into_inner()).unwrap();
            let decoded = decode_import(&path).unwrap();
            // Independently calculated sRGB OETF: round(255*(1.055*(v/255)^(1/2.4)-.055)).
            for (actual, expected) in decoded
                .as_raw()
                .iter()
                .zip([137u8, 188, 225, 73, 225, 99, 50, 0])
            {
                assert!(
                    actual.abs_diff(expected) <= 1,
                    "{format:?}: {actual} != {expected}"
                );
            }
            assert_eq!(decoded.get_pixel(0, 0)[3], 73);
            assert_eq!(decoded.get_pixel(1, 0)[3], 0);
        }
        let mut encoded = Vec::new();
        let mut jpeg = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut encoded, 100);
        jpeg.set_icc_profile(profile).unwrap();
        jpeg.write_image(&[128, 128, 128], 1, 1, image::ExtendedColorType::Rgb8)
            .unwrap();
        fs::write(&path, encoded).unwrap();
        assert_eq!(
            decode_import(&path).unwrap().get_pixel(0, 0).0,
            [188, 188, 188, 255]
        );

        let mut encoded = Vec::new();
        let mut png = image::codecs::png::PngEncoder::new(&mut encoded);
        png.set_icc_profile(ColorProfile::new_gray_with_gamma(1.).encode().unwrap())
            .unwrap();
        png.write_image(&[128, 73, 32, 0], 2, 1, image::ExtendedColorType::La8)
            .unwrap();
        fs::write(&path, encoded).unwrap();
        let decoded = decode_import(&path).unwrap();
        assert_eq!(decoded.as_raw(), &[188, 188, 188, 73, 99, 99, 99, 0]);
    }

    #[test]
    fn import_rejects_unusable_profiles_instead_of_relabeling_pixels() {
        use image::ImageEncoder;
        let data = tempfile::tempdir().unwrap();
        let path = data.path().join("profile.png");
        for profile in [vec![1, 2, 3], {
            let mut profile = moxcms::ColorProfile::new_srgb().encode().unwrap();
            profile[16..20].copy_from_slice(b"CMYK");
            profile
        }] {
            let mut encoded = Vec::new();
            let mut png = image::codecs::png::PngEncoder::new(&mut encoded);
            png.set_icc_profile(profile).unwrap();
            png.write_image(&[64, 128, 192, 73], 1, 1, image::ExtendedColorType::Rgba8)
                .unwrap();
            fs::write(&path, encoded).unwrap();
            assert!(decode_import(&path).unwrap_err().contains("color"));
        }
        let mut encoded = Vec::new();
        let mut png = png::Encoder::new(&mut encoded, 1, 1);
        png.set_color(png::ColorType::Rgba);
        png.set_source_gamma(png::ScaledFloat::new(1.));
        png.write_header()
            .unwrap()
            .write_image_data(&[64, 128, 192, 73])
            .unwrap();
        fs::write(&path, encoded).unwrap();
        // Gamma-only PNGs convert through their transfer, like the webview.
        let decoded = decode_import(&path).unwrap();
        for (actual, expected) in decoded.as_raw().iter().zip([137u8, 188, 225, 73]) {
            assert!(actual.abs_diff(expected) <= 1, "{actual} != {expected}");
        }
    }

    #[test]
    fn import_decoder_preserves_lossless_formats_applies_exif_and_rejects_limits() {
        let data = tempfile::tempdir().unwrap();
        let path = data.path().join("image.data"); // Sniff bytes, not the filename.
        let pixels = RgbaImage::from_fn(7, 3, |x, y| {
            image::Rgba([x as u8 * 31, y as u8 * 71, 9, 128])
        });
        for format in [ImageFormat::Png, ImageFormat::WebP, ImageFormat::Tiff] {
            pixels.save_with_format(&path, format).unwrap();
            assert_eq!(decode_import(&path).unwrap(), pixels, "{format:?}");
        }
        let rgb = image::DynamicImage::ImageRgba8(pixels).into_rgb8();
        rgb.save_with_format(&path, ImageFormat::Jpeg).unwrap();
        let decoded = decode_import(&path).unwrap();
        assert_eq!(decoded.dimensions(), (7, 3));
        let jpeg = fs::read(&path).unwrap();
        // APP1 Exif, little-endian TIFF IFD with orientation tag 6 (90° clockwise).
        let exif = b"Exif\0\0II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x06\0\0\0\0\0\0\0";
        let mut oriented = vec![0xff, 0xd8, 0xff, 0xe1];
        oriented.extend_from_slice(&((exif.len() + 2) as u16).to_be_bytes());
        oriented.extend_from_slice(exif);
        oriented.extend_from_slice(&jpeg[2..]);
        fs::write(&path, oriented).unwrap();
        let rotated = decode_import(&path).unwrap();
        assert_eq!(rotated.dimensions(), (3, 7));
        for y in 0..7 {
            for x in 0..3 {
                assert_eq!(rotated.get_pixel(x, y), decoded.get_pixel(y, 2 - x));
            }
        }
        fs::write(&path, b"GIF89a").unwrap();
        assert_eq!(
            decode_import(&path).unwrap_err(),
            "image.data could not be loaded."
        );
        fs::write(&path, b"not an image").unwrap();
        assert!(decode_import(&path).is_err());
        RgbaImage::new(MAX_RENDER_DIMENSION + 1, 1)
            .save_with_format(&path, ImageFormat::Png)
            .unwrap();
        assert!(
            decode_import(&path)
                .unwrap_err()
                .contains("pixels per side")
        );
        File::create(&path)
            .unwrap()
            .set_len(captures_history::editor_draft::MAX_IMAGE_BYTES as u64 + 1)
            .unwrap();
        assert_eq!(
            decode_import(&path).unwrap_err(),
            "This image is too large to open in Captures."
        );
    }

    #[test]
    fn quit_drops_unaccepted_picker_results_but_saves_already_queued_imports() {
        let (data, id) = fixture();
        let path = data.path().join("new.png");
        RgbaImage::new(3, 2).save(&path).unwrap();
        let ctx = egui::Context::default();
        let editor = Editor::open(
            &ctx,
            data.path().join("history"),
            id.clone(),
            data.path().join("exports"),
            CaptureMode::Region,
            |_| unreachable!("copy was not requested"),
        );
        receive(&editor, &ctx);
        let (selection, picked) = mpsc::channel();
        editor.view.lock().unwrap().import_picker = Some(picked);
        selection.send(Some(vec![path.clone()])).unwrap();
        editor.flush(&ctx).unwrap();
        {
            let view = editor.view.lock().unwrap();
            assert!(!view.pending && !view.unsaved() && view.import_picker.is_none());
            assert_eq!(view.presented.as_ref().unwrap().document.elements.len(), 1);
        }
        assert!(!data.path().join("editor-drafts").exists());
        editor.view.lock().unwrap().submit_job(
            &editor.tx,
            Job::Import {
                path,
                selected_id: None,
                point: None,
            },
        );
        editor.flush(&ctx).unwrap();
        let view = editor.view.lock().unwrap();
        // Flush acknowledges durable saving before its final UI snapshot may
        // arrive. Verify persisted contents below, not that asynchronous label.
        assert!(!view.pending);
        assert_eq!(view.presented.as_ref().unwrap().document.elements.len(), 2);
        let manifest: serde_json::Value = serde_json::from_slice(
            &fs::read(
                data.path()
                    .join("editor-drafts")
                    .join(id)
                    .join("manifest.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            manifest["document"]["elements"].as_array().unwrap().len(),
            2
        );
    }

    #[test]
    fn imported_file_is_owned_undoable_and_reopens_after_source_deletion() {
        let (data, id) = fixture();
        let original = fs::read(data.path().join("history").join(&id).join("capture.png")).unwrap();
        let path = data.path().join("Imported é.png");
        let imported = RgbaImage::from_fn(3, 2, |x, y| {
            image::Rgba([x as u8 * 80, y as u8 * 90, 255, 255])
        });
        imported.save(&path).unwrap();
        let ctx = egui::Context::default();
        let open = || {
            Editor::open(
                &ctx,
                data.path().join("history"),
                id.clone(),
                data.path().join("exports"),
                CaptureMode::Region,
                |_| unreachable!("copy was not requested"),
            )
        };
        let editor = open();
        receive(&editor, &ctx);
        editor.view.lock().unwrap().submit_job(
            &editor.tx,
            Job::Import {
                path: path.clone(),
                selected_id: Some("capture-background".into()),
                point: None,
            },
        );
        receive(&editor, &ctx);
        let (layer_id, edited) = {
            let view = editor.view.lock().unwrap();
            let frame = view.presented.as_ref().unwrap();
            let layer = frame.document.elements.last().unwrap();
            assert_eq!(layer_label(layer), "Imported é.png");
            assert_eq!(
                view.selected_layer.as_deref(),
                Some(layer.base().id.as_str())
            );
            assert_eq!((layer.base().x, layer.base().y), (2., 3.));
            assert_eq!(frame.pixels.dimensions(), (7, 5));
            for y in 0..2 {
                for x in 0..3 {
                    assert_eq!(
                        frame.pixels.get_pixel(x + 2, y + 3),
                        imported.get_pixel(x, y)
                    );
                }
            }
            assert!(view.unsaved());
            (layer.base().id.clone(), frame.pixels.clone())
        };
        assert!(!data.path().join("editor-drafts").exists());
        fs::remove_file(&path).unwrap();
        editor
            .view
            .lock()
            .unwrap()
            .submit(&editor.tx, Request::Undo);
        receive(&editor, &ctx);
        let before = editor
            .view
            .lock()
            .unwrap()
            .presented
            .as_ref()
            .unwrap()
            .pixels
            .clone();
        editor.view.lock().unwrap().submit_job(
            &editor.tx,
            Job::Import {
                path,
                selected_id: None,
                point: None,
            },
        );
        receive(&editor, &ctx);
        {
            let view = editor.view.lock().unwrap();
            assert!(view.error.is_some() && !view.pending);
            assert!(view.presented.as_ref().unwrap().can_redo);
            assert!(Arc::ptr_eq(
                &before,
                &view.presented.as_ref().unwrap().pixels
            ));
        }
        editor
            .view
            .lock()
            .unwrap()
            .submit(&editor.tx, Request::Redo);
        receive(&editor, &ctx);
        editor
            .view
            .lock()
            .unwrap()
            .submit(&editor.tx, Request::SaveDraft { updated_at_ms: 123 });
        receive(&editor, &ctx);
        drop(editor);
        let reopened = open();
        receive(&reopened, &ctx);
        let view = reopened.view.lock().unwrap();
        let frame = view.presented.as_ref().unwrap();
        assert_eq!(frame.document.elements.last().unwrap().base().id, layer_id);
        assert_eq!(frame.pixels.as_ref(), edited.as_ref());
        assert!(!view.unsaved() && frame.has_draft);
        assert_eq!(
            fs::read(data.path().join("history").join(id).join("capture.png")).unwrap(),
            original
        );
        assert!(!data.path().join("exports").exists());
    }

    #[test]
    fn layer_selection_follows_ids_and_failed_commands_restore_published_fields() {
        let ctx = egui::Context::default();
        let mut view = View::default();
        let mut value = presented(true);
        Arc::make_mut(&mut value.document)
            .edit_layer(
                "capture-background",
                LayerEdit::Duplicate {
                    new_id: "copy".into(),
                },
            )
            .unwrap();
        view.receive(&ctx, Ok(value));
        view.select_layer_exact(Some("copy".into()));
        assert_eq!(view.selected_layer.as_deref(), Some("copy"));
        assert_eq!(view.layer_geometry, [7., 3., 24., 24.]);
        let (tx, rx) = mpsc::channel();
        view.submit(
            &tx,
            Request::Layer {
                id: "copy".into(),
                edit: LayerEdit::Visibility { visible: false },
            },
        );
        assert!(
            matches!(rx.recv().unwrap(), Job::Apply(Request::Layer { id, edit: LayerEdit::Visibility { visible: false } }) if id == "copy")
        );
        assert!(view.pending);
        view.selected_layer = Some("rejected-duplicate".into());
        view.layer_geometry = [999., 999., 999., 999.];
        view.receive(&ctx, Err("unsupported layer".into()));
        assert_eq!(view.selected_layer.as_deref(), Some("copy"));
        assert_eq!(view.layer_geometry, [7., 3., 24., 24.]);
        assert!(!view.pending);
        view.receive(&ctx, Ok(presented(false))); // Undo removed the selected copy.
        assert_eq!(view.selected_layer.as_deref(), Some("capture-background"));
        assert_eq!(view.layer_geometry, [7., 3., 0., 0.]);
        let mut empty = presented(true);
        Arc::make_mut(&mut empty.document).elements.clear();
        view.receive(&ctx, Ok(empty));
        assert!(view.selected_layer.is_none());
    }

    #[test]
    fn folder_selection_preserves_filename_and_cancelled_or_stale_results_preserve_state() {
        let ctx = egui::Context::default();
        let mut view = View {
            default_directory: "old folder".into(),
            default_stem: "capture.é".into(),
            ..View::default()
        };
        view.receive(&ctx, Ok(presented(true)));
        view.export_options.format = ExportFormat::Webp;
        let frame = view.presented.as_ref().unwrap().pixels.clone();
        let document = view.presented.as_ref().unwrap().document.clone();
        let destination = |view: &View| {
            view.export_target
                .as_ref()
                .unwrap()
                .destination(ExportFormat::Webp)
        };
        let old_path = PathBuf::from("old folder").join("capture.é.webp");
        assert_eq!(destination(&view), old_path);
        view.output_notice = Some("Previous result".into());
        let (tx, rx) = mpsc::channel();
        view.folder_picker = Some(rx);
        view.receive_folder();
        assert!(view.folder_picker.is_some() && !view.pending);
        tx.send(None).unwrap();
        view.receive_folder();
        assert!(view.folder_picker.is_none());
        assert_eq!(destination(&view), old_path);
        assert_eq!(view.output_notice.as_deref(), Some("Previous result"));

        let (tx, rx) = mpsc::channel();
        view.folder_picker = Some(rx);
        drop(tx);
        view.receive_folder();
        assert!(view.error.is_some() && view.folder_picker.is_none());
        assert_eq!(destination(&view), old_path);
        let (tx, rx) = mpsc::channel();
        view.folder_picker = Some(rx);
        tx.send(Some(PathBuf::from("chosen folder"))).unwrap();
        view.receive_folder();
        let chosen = PathBuf::from("chosen folder").join("capture.é.webp");
        assert_eq!(destination(&view), chosen);
        assert_eq!(view.filename, "capture.é");
        assert!(view.output_notice.is_none() && view.error.is_none() && !view.pending);
        assert!(view.unsaved() && view.presented.as_ref().unwrap().can_undo);
        assert!(Arc::ptr_eq(
            &view.presented.as_ref().unwrap().pixels,
            &frame
        ));
        assert!(Arc::ptr_eq(
            &view.presented.as_ref().unwrap().document,
            &document
        ));

        let (tx, rx) = mpsc::channel();
        view.folder_picker = Some(rx);
        view.closed = true;
        tx.send(Some(PathBuf::from("stale folder"))).unwrap();
        view.receive_folder();
        assert_eq!(destination(&view), chosen);
        assert!(view.closed && view.folder_picker.is_none());
    }

    #[test]
    fn close_waits_for_pending_edits_then_flushes_the_draft_without_asking() {
        let ctx = egui::Context::default();
        let (tx, rx) = mpsc::channel();
        let mut view = View::default();
        view.request_close();
        view.drive_close(&tx);
        assert!(
            !view.closed && rx.try_recv().is_err(),
            "the open finishes first"
        );
        view.receive(&ctx, Ok(presented(true)));
        assert!(view.close_requested && !view.closed && view.unsaved());
        // Shipping closes without a prompt and flushes the draft first.
        view.drive_close(&tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Job::Apply(Request::AutosaveDraft { .. }))
        ));
        assert!(view.pending && !view.closed);
        view.drive_close(&tx);
        assert!(rx.try_recv().is_err(), "one flush per close");
        // The flush is best-effort, as in shipping: a failure still closes.
        view.receive(&ctx, Err("disk full".into()));
        assert!(view.closed);
        // A stale completion cannot reopen a closed controller.
        view.receive(&ctx, Ok(presented(true)));
        assert!(view.closed);

        // Nothing to save closes at once.
        let mut saved = View::default();
        saved.receive(&ctx, Ok(presented(false)));
        saved.request_close();
        saved.drive_close(&tx);
        assert!(saved.closed && rx.try_recv().is_err());
        // A pending autosave still flushes.
        let mut timed = View::default();
        timed.receive(&ctx, Ok(presented(false)));
        timed.autosave.edited(Instant::now());
        timed.request_close();
        timed.drive_close(&tx);
        assert!(matches!(
            rx.try_recv(),
            Ok(Job::Apply(Request::AutosaveDraft { .. }))
        ));
        timed.receive(&ctx, Ok(presented(false)));
        assert!(timed.closed);
        let mut failed_open = View::default();
        failed_open.request_close();
        failed_open.receive(&ctx, Err("missing screenshot".into()));
        failed_open.drive_close(&tx);
        assert!(failed_open.closed);
    }

    #[test]
    fn edits_autosave_the_draft_after_700ms_in_the_background() {
        let ctx = egui::Context::default();
        let (tx, rx) = mpsc::channel();
        let mut view = View {
            autosaves: true,
            ..View::default()
        };
        view.receive(&ctx, Ok(presented(false)));
        assert_eq!(view.autosave.deadline(), None, "opening is not an edit");
        let before = Instant::now();
        view.receive(&ctx, Ok(presented(true)));
        let due = view.autosave.deadline().expect("an edit arms the autosave");
        assert!(due >= before + DraftAutosave::DELAY);
        view.drive_autosave(&ctx, &tx);
        assert!(rx.try_recv().is_err(), "not before 700 ms");
        view.autosave = DraftAutosave::default();
        view.autosave.edited(Instant::now() - DraftAutosave::DELAY);
        view.pending = true;
        view.drive_autosave(&ctx, &tx);
        assert!(rx.try_recv().is_err(), "waits behind a running edit");
        view.pending = false;
        view.drive_autosave(&ctx, &tx);
        let Ok(Job::Autosave { reply }) = rx.try_recv() else {
            panic!("a due autosave runs on the worker");
        };
        // It never blocks editing or retitles the window.
        assert!(!view.pending && view.autosave_rx.is_some());
        reply.send(Ok(true)).unwrap();
        assert!(view.receive_autosave());
        let presented = view.presented.as_ref().unwrap();
        assert!(!presented.unsaved && presented.has_draft);
        assert!(view.autosave_rx.is_none() && view.autosave.deadline().is_none());
        // Discarding the restored draft cancels a pending save.
        view.autosave.edited(Instant::now());
        view.discard_draft(&tx);
        assert!(view.autosave.deadline().is_none());
        assert!(matches!(
            rx.try_recv(),
            Ok(Job::Apply(Request::DiscardDraft))
        ));
    }

    fn canvas_view(
        ctx: &egui::Context,
        start: Point,
        end: Point,
        shape: OpenShapeKind,
    ) -> (View, String) {
        let mut value = presented(false);
        value.pixels = Arc::new(RgbaImage::new(200, 100));
        let document = Arc::make_mut(&mut value.document);
        document.width = 200.;
        document.height = 100.;
        if let Element::Image(image) = &mut document.elements[0] {
            image.width = 200.;
            image.height = 100.;
        }
        let id = document
            .create_open_shape(OpenShapeCreate {
                shape,
                start,
                end,
                style: ElementStyle::default(),
                opacity: 100.,
            })
            .unwrap();
        let mut view = View::default();
        view.receive(ctx, Ok(value));
        view.pending = false;
        view.section = Section::Layers;
        view.select_layer(Some(id.clone()));
        (view, id)
    }

    fn run_canvas(
        ctx: &egui::Context,
        view: &mut View,
        tx: &Sender<Job>,
        raw: egui::RawInput,
        preview: egui::Rect,
    ) {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400., 300.));
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(screen),
                focused: true,
                ..raw
            },
            |_| {
                let mut ui = egui::Ui::new(
                    ctx.clone(),
                    egui::Id::unique("canvas-interaction-test"),
                    egui::UiBuilder::new().max_rect(screen),
                );
                let tokens = crate::tokens::load().into_values().next().unwrap();
                canvas::receive_drops(ctx, view, Some(preview));
                let owned = view.shape_drag.is_none()
                    && show_layer_canvas(&mut ui, &tokens, view, tx, screen, preview, false);
                if view.section == Section::Draw && !view.pending {
                    show_shape(&mut ui, &tokens, view, tx, screen, preview, owned);
                }
                canvas::show_expand(&mut ui, &tokens, view, tx, screen, preview);
                canvas::paint_drop_guide(&ui, &tokens, view, screen, preview);
            },
        );
        output.textures_delta.clear();
    }

    #[test]
    fn active_shape_handles_and_body_own_the_whole_gesture_but_empty_space_draws() {
        for action in ["click", "resize", "rotate", "move", "draw", "cancel"] {
            let ctx = egui::Context::default();
            let (mut view, _) = canvas_view(
                &ctx,
                Point { x: 20., y: 50. },
                Point { x: 180., y: 50. },
                OpenShapeKind::Line,
            );
            let document = Arc::make_mut(&mut view.presented.as_mut().unwrap().document);
            document.elements.pop();
            let id = document
                .create_closed_shape(ClosedShapeCreate {
                    shape: ClosedShapeKind::Rectangle,
                    start: Point { x: 20., y: 20. },
                    end: Point { x: 100., y: 70. },
                    style: ElementStyle::default(),
                    opacity: 100.,
                })
                .unwrap();
            let outline = document
                .elements
                .last()
                .unwrap()
                .selection_outline()
                .unwrap();
            view.section = Section::Draw;
            view.draw_shape = DrawShape::Rectangle;
            view.select_layer_exact(Some(id.clone()));
            let (tx, rx) = mpsc::channel();
            let preview = egui::Rect::from_min_size(egui::pos2(100., 100.), egui::vec2(100., 50.));
            let project =
                |point: Point| egui::pos2(100. + point.x as f32 / 2., 100. + point.y as f32 / 2.);
            let start = project(match action {
                "move" => Point { x: 60., y: 45. },
                "draw" => Point { x: 180., y: 85. },
                "rotate" => {
                    rotation_handle(outline, 0., 0.5, 200., 100.)
                        .unwrap()
                        .handle
                }
                _ => outline[0],
            });
            // A new drawing crosses the old selected body; hover cannot steal it.
            let end = if action == "draw" {
                project(Point { x: 60., y: 45. })
            } else {
                start + egui::vec2(23., 7.)
            };
            let button = |pos, pressed| egui::Event::PointerButton {
                pos,
                pressed,
                button: egui::PointerButton::Primary,
                modifiers: egui::Modifiers::NONE,
            };
            let frame = |view: &mut View, events| {
                run_canvas(
                    &ctx,
                    view,
                    &tx,
                    egui::RawInput {
                        events,
                        ..Default::default()
                    },
                    preview,
                )
            };
            frame(&mut view, vec![]);
            frame(&mut view, vec![egui::Event::PointerMoved(start)]);
            if action == "click" {
                frame(&mut view, vec![button(start, true), button(start, false)]);
                assert!(rx.try_recv().is_err() && !view.pending && view.shape_drag.is_none());
                assert_eq!(view.selected_layer.as_deref(), Some(id.as_str()));
                continue;
            }
            frame(&mut view, vec![button(start, true)]);
            frame(&mut view, vec![egui::Event::PointerMoved(end)]);
            if action == "draw" {
                assert_eq!(
                    view.shape_drag,
                    Some((Point { x: 180., y: 85. }, Point { x: 60., y: 45. }))
                );
                assert!(view.layer_gesture.is_none());
            }
            assert!(rx.try_recv().is_err(), "{action}: only release commits");
            if action == "cancel" {
                frame(
                    &mut view,
                    vec![egui::Event::Key {
                        key: egui::Key::Escape,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    }],
                );
            }
            frame(
                &mut view,
                vec![button(end, false), egui::Event::PointerMoved(start)],
            );
            if action == "cancel" {
                assert!(rx.try_recv().is_err() && !view.pending);
            } else {
                let job = rx.try_recv().expect(action);
                match (action, job) {
                    (
                        "resize",
                        Job::Apply(Request::Layer {
                            id: target,
                            edit:
                                LayerEdit::Resize {
                                    handle: ResizeHandle::Nw,
                                    current,
                                    ..
                                },
                        }),
                    ) => {
                        assert_eq!(target, id);
                        assert!((current.x - (outline[0].x + 46.)).abs() < 1e-12);
                        assert!((current.y - (outline[0].y + 14.)).abs() < 1e-12);
                    }
                    (
                        "rotate",
                        Job::Apply(Request::Layer {
                            id: target,
                            edit: LayerEdit::Rotate { radians },
                        }),
                    ) => {
                        assert_eq!(target, id);
                        assert_ne!(radians, 0.);
                    }
                    (
                        "move",
                        Job::Apply(Request::Layer {
                            id: target,
                            edit:
                                LayerEdit::DragMove {
                                    delta_x,
                                    delta_y,
                                    display_scale,
                                },
                        }),
                    ) => {
                        assert_eq!(target, id);
                        assert_eq!((delta_x, delta_y, display_scale), (46., 14., 0.5));
                    }
                    ("draw", Job::Apply(Request::CreateClosedShape { create })) => {
                        assert_eq!(create.shape, ClosedShapeKind::Rectangle);
                        assert_eq!(create.start, Point { x: 180., y: 85. });
                        assert_eq!(create.end, Point { x: 60., y: 45. });
                        assert!(view.selected_layer.is_none());
                    }
                    _ => panic!("{action}: wrong gesture owner"),
                }
                assert!(rx.try_recv().is_err(), "{action}: exactly one request");
            }
            assert_eq!(
                (view.section, view.draw_shape),
                (Section::Draw, DrawShape::Rectangle)
            );
            assert!(view.layer_gesture.is_none() && view.shape_drag.is_none());
        }
        for (kind, tool) in [
            (OpenShapeKind::Line, DrawShape::Line),
            (OpenShapeKind::Arrow, DrawShape::Arrow),
        ] {
            let ctx = egui::Context::default();
            let (mut view, id) = canvas_view(
                &ctx,
                Point { x: 20., y: 50. },
                Point { x: 180., y: 50. },
                kind,
            );
            view.section = Section::Draw;
            view.draw_shape = tool;
            let (tx, rx) = mpsc::channel();
            let preview = egui::Rect::from_min_size(egui::pos2(100., 100.), egui::vec2(100., 50.));
            let start = egui::pos2(150., 125.);
            let end = egui::pos2(150., 140.);
            let button = |pos, pressed| egui::Event::PointerButton {
                pos,
                pressed,
                button: egui::PointerButton::Primary,
                modifiers: egui::Modifiers::NONE,
            };
            for events in [
                vec![],
                vec![egui::Event::PointerMoved(start), button(start, true)],
                vec![egui::Event::PointerMoved(end)],
                vec![button(end, false)],
            ] {
                run_canvas(
                    &ctx,
                    &mut view,
                    &tx,
                    egui::RawInput {
                        events,
                        ..Default::default()
                    },
                    preview,
                );
            }
            assert!(
                matches!(rx.try_recv(), Ok(Job::Apply(Request::Layer { id: target, edit: LayerEdit::Curve { edit: captures_app::editor_canvas::CurveEdit::Move { handle: captures_app::editor_canvas::CurveHandle::StarterControl { index: 1 }, point } } })) if target == id && point == Point { x: 100., y: 80. })
            );
            assert!(rx.try_recv().is_err() && view.shape_drag.is_none());
            assert_eq!((view.section, view.draw_shape), (Section::Draw, tool));
        }
    }

    #[test]
    fn trim_preview_paints_only_while_trim_edges_is_hovered() {
        let ctx = egui::Context::default();
        let mut value = presented(false);
        // The 7×3 capture on a 10×3 canvas: Trim edges would cut 3 px on the right.
        Arc::make_mut(&mut value.document).width = 10.;
        value.pixels = Arc::new(RgbaImage::new(10, 3));
        let mut view = View::default();
        view.receive(&ctx, Ok(value));
        view.pending = false;
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400., 300.));
        let preview = egui::Rect::from_min_size(egui::pos2(50., 100.), egui::vec2(300., 90.));
        let frame = |view: &View, reduced: bool| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |_| {
                    let ui = egui::Ui::new(
                        ctx.clone(),
                        egui::Id::unique("trim-preview-test"),
                        egui::UiBuilder::new().max_rect(screen),
                    );
                    crate::motion::set_reduced(ui.ctx(), reduced);
                    canvas::paint_trim_preview(&ui, test_tokens(), view, screen, preview);
                },
            );
            let repaint = output
                .viewport_output
                .get(&egui::ViewportId::ROOT)
                .is_some_and(|viewport| viewport.repaint_delay.is_zero());
            output.textures_delta.clear();
            (output.shapes.len(), repaint)
        };
        let (idle, _) = frame(&view, false);
        // Mid-loop, so the particle stream is under way.
        view.trim_hover_since = Some(-0.6);
        let (hovered, repaint) = frame(&view, false);
        assert!(hovered > idle + 10, "{idle} → {hovered}");
        assert!(repaint, "the breathing hint and particles animate");
        let (reduced, _) = frame(&view, true);
        assert!(
            reduced > idle && reduced < hovered,
            "reduced motion keeps the static hint without particles ({reduced} of {hovered})"
        );
        // A tight canvas has nothing to preview even while hovered.
        Arc::make_mut(&mut view.presented.as_mut().unwrap().document).width = 7.;
        assert_eq!(frame(&view, false).0, idle);
    }

    #[test]
    fn wand_loupe_follows_image_pixels_and_hides_off_image() {
        use captures_app::editor_image_background as background;
        let ctx = egui::Context::default();
        let mut value = presented(false);
        let mut source = RgbaImage::from_pixel(7, 3, image::Rgba([40, 110, 166, 255]));
        source.put_pixel(3, 1, image::Rgba([229, 179, 68, 255]));
        value.pixels = Arc::new(RgbaImage::new(7, 3));
        value
            .image_assets
            .insert("fixture".into(), Arc::new(source.clone()));
        let mut view = View::default();
        view.receive(&ctx, Ok(value));
        view.pending = false;
        view.section = Section::Draw;
        view.draw_shape = DrawShape::Wand;
        let (tx, _rx) = mpsc::channel();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400., 300.));
        let preview = egui::Rect::from_min_size(egui::pos2(50., 100.), egui::vec2(70., 30.));
        let hover_once = |view: &mut View, pos: egui::Pos2| {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(screen),
                    events: vec![egui::Event::PointerMoved(pos)],
                    focused: true,
                    ..Default::default()
                },
                |_| {
                    let mut ui = egui::Ui::new(
                        ctx.clone(),
                        egui::Id::unique("wand-loupe-test"),
                        egui::UiBuilder::new().max_rect(screen),
                    );
                    show_shape(&mut ui, test_tokens(), view, &tx, screen, preview, false);
                },
            );
            output.textures_delta.clear();
        };
        // egui hit-tests against the previous frame, so hover twice.
        let hover = |view: &mut View, pos: egui::Pos2| {
            for _ in 0..2 {
                hover_once(view, pos);
            }
        };
        // Document pixel (3, 1) is the yellow sample.
        hover(&mut view, egui::pos2(85., 115.));
        assert!(view.wand_loupe.is_some(), "the loupe shows over the image");
        hover(&mut view, egui::pos2(300., 250.));
        assert!(view.wand_loupe.is_none(), "off the image the loupe hides");
        view.draw_shape = DrawShape::Erase;
        hover(&mut view, egui::pos2(85., 115.));
        assert!(view.wand_loupe.is_none(), "only the Wand has a loupe");

        let document = Document::new_capture("fixture", 7., 3., None);
        let loupe =
            background::wand_loupe(&document, |_| Some(&source), Point { x: 3.5, y: 1.5 }).unwrap();
        let image = canvas::loupe_image(&loupe, 2.);
        let side = image.size[0];
        assert_eq!(side, 168, "84 pt at 2× device pixels");
        let centre = image.pixels[side / 2 * side + side / 2];
        assert_eq!(
            centre,
            egui::Color32::from_rgb(229, 179, 68),
            "the keyed sample is the centre tile"
        );
        assert_eq!(
            image.pixels[0],
            egui::Color32::TRANSPARENT,
            "clipped to a circle"
        );
    }

    #[test]
    fn locked_curve_inspector_submits_bend_and_straighten_without_canvas_handles() {
        use captures_app::editor_canvas::CurveEdit;
        for kind in [OpenShapeKind::Line, OpenShapeKind::Arrow] {
            for multipoint in [false, true] {
                let ctx = egui::Context::default();
                ctx.enable_accesskit();
                let tokens = crate::tokens::load().remove("light-mustard").unwrap();
                let (mut view, id) = canvas_view(
                    &ctx,
                    Point { x: 20., y: 50. },
                    Point { x: 180., y: 50. },
                    kind,
                );
                let document = Arc::make_mut(&mut view.presented.as_mut().unwrap().document);
                let Element::Shape(shape) = document.elements.last_mut().unwrap() else {
                    panic!()
                };
                shape.base.locked = true;
                if multipoint {
                    shape.controls = vec![Point { x: 60., y: 75. }, Point { x: 130., y: 25. }];
                }
                let label = if multipoint {
                    captures_app::editor_canvas::straighten_label(shape)
                } else {
                    "Curve"
                };
                let document = document.clone();
                assert!(
                    canvas::selected_curve(&view).is_none(),
                    "lock still hides canvas dots"
                );
                let (tx, rx) = mpsc::channel();
                assert!(!canvas::double_click(
                    &mut view,
                    &tx,
                    &document,
                    Point { x: 100., y: 50. },
                    6.
                ));
                let frame = |view: &mut View, events| {
                    let mut output = ctx.run_ui(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(320., 1600.),
                            )),
                            events,
                            ..Default::default()
                        },
                        |ui| show_layer_properties(ui, &tokens, view, &tx),
                    );
                    output.textures_delta.clear();
                    output
                };
                frame(&mut view, vec![]);
                let output = frame(&mut view, vec![]);
                let node = &output
                    .platform_output
                    .accesskit_update
                    .as_ref()
                    .unwrap()
                    .nodes
                    .iter()
                    .find(|(_, node)| node.label() == Some(label))
                    .expect("locked curve property stays present")
                    .1;
                assert!(!node.is_disabled());
                let bounds = node.bounds().unwrap();
                let pos = egui::pos2(
                    ((bounds.x0 + bounds.x1) / 2.) as f32,
                    ((bounds.y0 + bounds.y1) / 2.) as f32,
                );
                assert!(
                    rx.try_recv().is_err(),
                    "showing locked Properties is not an edit"
                );
                frame(&mut view, vec![egui::Event::PointerMoved(pos)]);
                for pressed in [true, false] {
                    frame(
                        &mut view,
                        vec![egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        }],
                    );
                }
                if !multipoint {
                    assert!(
                        rx.try_recv().is_err(),
                        "clicking the zero midpoint is not an edit"
                    );
                    frame(
                        &mut view,
                        vec![egui::Event::Key {
                            key: egui::Key::End,
                            physical_key: None,
                            pressed: true,
                            repeat: false,
                            modifiers: egui::Modifiers::NONE,
                        }],
                    );
                }
                let Ok(Job::Apply(Request::Layer {
                    id: target,
                    edit: LayerEdit::Curve { edit },
                })) = rx.try_recv()
                else {
                    panic!("locked inspector applies one curve edit")
                };
                assert_eq!(target, id);
                assert_eq!(
                    edit,
                    if multipoint {
                        CurveEdit::Straighten
                    } else {
                        CurveEdit::Bend { bend: 1. }
                    }
                );
                assert!(rx.try_recv().is_err());
                assert_eq!(view.pending_layer_selection.as_deref(), Some(id.as_str()));
                assert!(canvas::selected_curve(&view).is_none());
            }
        }
    }

    #[test]
    fn curve_dots_drag_once_on_release_and_double_clicks_add_or_remove_points() {
        use captures_app::editor_canvas::{CurveEdit, CurveHandle};
        let ctx = egui::Context::default();
        let (mut view, id) = canvas_view(
            &ctx,
            Point { x: 20., y: 50. },
            Point { x: 180., y: 50. },
            OpenShapeKind::Line,
        );
        let (tx, rx) = mpsc::channel();
        // 200×100 document shown at half scale.
        let preview = egui::Rect::from_min_size(egui::pos2(100., 100.), egui::vec2(100., 50.));
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let events = |events| egui::RawInput {
            events,
            ..Default::default()
        };
        // The middle starter dot sits at document (100, 50).
        let starter = egui::pos2(150., 125.);
        run_canvas(
            &ctx,
            &mut view,
            &tx,
            events(vec![button(starter, true)]),
            preview,
        );
        assert!(matches!(
            view.layer_gesture.as_ref().map(|gesture| &gesture.kind),
            Some(LayerGestureKind::Curve {
                handle: CurveHandle::StarterControl { index: 1 },
                ..
            })
        ));
        run_canvas(
            &ctx,
            &mut view,
            &tx,
            events(vec![egui::Event::PointerMoved(egui::pos2(150., 140.))]),
            preview,
        );
        assert!(
            rx.try_recv().is_err(),
            "dragging previews without committing"
        );
        run_canvas(
            &ctx,
            &mut view,
            &tx,
            events(vec![button(egui::pos2(150., 140.), false)]),
            preview,
        );
        match rx.try_recv().unwrap() {
            Job::Apply(Request::Layer {
                id: layer,
                edit:
                    LayerEdit::Curve {
                        edit: CurveEdit::Move { handle, point },
                    },
            }) => {
                assert_eq!(layer, id);
                assert_eq!(handle, CurveHandle::StarterControl { index: 1 });
                assert_eq!(point, Point { x: 100., y: 80. });
            }
            _ => panic!("expected one curve move"),
        }
        assert!(rx.try_recv().is_err());
        assert_eq!(view.pending_layer_selection.as_deref(), Some(id.as_str()));

        // A click on a starter without movement never edits.
        view.pending = false;
        run_canvas(
            &ctx,
            &mut view,
            &tx,
            events(vec![button(starter, true)]),
            preview,
        );
        run_canvas(
            &ctx,
            &mut view,
            &tx,
            events(vec![button(starter, false)]),
            preview,
        );
        assert!(rx.try_recv().is_err());

        // Double-click on the path inserts a point; on a control removes it.
        view.pending = false;
        let document = view.presented.as_ref().unwrap().document.clone();
        assert!(canvas::double_click(
            &mut view,
            &tx,
            &document,
            Point { x: 80., y: 51. },
            6.
        ));
        assert!(matches!(
            rx.try_recv().unwrap(),
            Job::Apply(Request::Layer {
                edit: LayerEdit::Curve {
                    edit: CurveEdit::Insert { .. }
                },
                ..
            })
        ));
        view.pending = false;
        assert!(!canvas::double_click(
            &mut view,
            &tx,
            &document,
            Point { x: 60., y: 95. },
            6.
        ));
        let mut curved = (*document).clone();
        curved
            .edit_layer(
                &id,
                LayerEdit::Curve {
                    edit: CurveEdit::Bend { bend: 0.2 },
                },
            )
            .unwrap();
        Arc::make_mut(&mut view.presented.as_mut().unwrap().document).elements =
            curved.elements.clone();
        assert!(canvas::double_click(
            &mut view,
            &tx,
            &curved,
            Point { x: 100., y: 82. },
            6.
        ));
        assert!(matches!(
            rx.try_recv().unwrap(),
            Job::Apply(Request::Layer {
                edit: LayerEdit::Curve {
                    edit: CurveEdit::Remove { index: 0 }
                },
                ..
            })
        ));
        assert_eq!(
            canvas::hover_hint(&view, &curved, Point { x: 100., y: 82. }, 6.),
            Some("Double-click to remove curve point")
        );
    }

    #[test]
    fn expand_canvas_action_submits_one_edit_and_blocks_the_canvas_press() {
        let ctx = egui::Context::default();
        // The arrow hangs past the right edge of the 200×100 canvas.
        let (mut view, id) = canvas_view(
            &ctx,
            Point { x: 120., y: 50. },
            Point { x: 260., y: 50. },
            OpenShapeKind::Arrow,
        );
        let (tx, rx) = mpsc::channel();
        let preview = egui::Rect::from_min_size(egui::pos2(100., 100.), egui::vec2(100., 50.));
        let expand = canvas::expand_preview(&view).unwrap();
        assert_eq!(expand.0, id);
        assert_eq!(
            expand.1.anchor_edge,
            captures_app::editor_canvas::CanvasEdge::Right
        );
        let mut button = None;
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400., 300.));
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let tokens = crate::tokens::load().into_values().next().unwrap();
            button = canvas::expand_button_rect(ui, &tokens, &view, screen, preview);
        });
        output.textures_delta.clear();
        let (_, _, rect) = button.unwrap();
        // Right of the canvas, 22 px outside its edge.
        assert!((rect.center().x - 222.).abs() < 0.5, "{rect:?}");
        let click = |pressed| egui::Event::PointerButton {
            pos: rect.center(),
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let raw = |events| egui::RawInput {
            events,
            ..Default::default()
        };
        run_canvas(
            &ctx,
            &mut view,
            &tx,
            raw(vec![egui::Event::PointerMoved(rect.center())]),
            preview,
        );
        run_canvas(&ctx, &mut view, &tx, raw(vec![click(true)]), preview);
        run_canvas(&ctx, &mut view, &tx, raw(vec![click(false)]), preview);
        match rx.try_recv().unwrap() {
            Job::Apply(Request::Layer {
                id: layer,
                edit: LayerEdit::ExpandCanvas,
            }) => assert_eq!(layer, id),
            _ => panic!("expected expand_canvas"),
        }
        assert!(rx.try_recv().is_err() && view.layer_gesture.is_none());
    }

    #[derive(Debug)]
    struct TestDrop(PathBuf);

    impl egui::DroppedFile for TestDrop {
        fn path(&self) -> &Path {
            &self.0
        }

        fn bytes(&self) -> Result<Vec<u8>, String> {
            Err("tests never read dropped bytes".into())
        }
    }

    #[test]
    fn file_drops_queue_images_at_the_pointer_and_import_one_at_a_time() {
        let ctx = egui::Context::default();
        let (mut view, id) = canvas_view(
            &ctx,
            Point { x: 20., y: 50. },
            Point { x: 60., y: 50. },
            OpenShapeKind::Line,
        );
        let (tx, rx) = mpsc::channel();
        let preview = egui::Rect::from_min_size(egui::pos2(100., 100.), egui::vec2(100., 50.));
        let over = egui::pos2(150., 101.);
        run_canvas(
            &ctx,
            &mut view,
            &tx,
            egui::RawInput {
                events: vec![egui::Event::PointerMoved(over)],
                hovered_files: vec![egui::HoveredFile {
                    path: Some("shot.png".into()),
                    ..Default::default()
                }],
                ..Default::default()
            },
            preview,
        );
        assert!(view.drop.hovering);
        let guide = canvas::drop_guide(&view, Some(Point { x: 100., y: 2. })).unwrap();
        assert_eq!(guide.label, "Place above");
        let drop = |paths: &[&str]| egui::RawInput {
            dropped_files: paths
                .iter()
                .map(|path| Arc::new(TestDrop(PathBuf::from(path))) as egui::DroppedFileHandle)
                .collect(),
            ..Default::default()
        };
        run_canvas(&ctx, &mut view, &tx, drop(&["notes.txt"]), preview);
        assert_eq!(
            view.error.as_deref(),
            Some(captures_app::editor_session::DROP_UNSUPPORTED)
        );
        assert!(view.drop.queue.is_empty() && !view.drop.hovering);
        run_canvas(
            &ctx,
            &mut view,
            &tx,
            drop(&["a.PNG", "skip.gif", "b.webp"]),
            preview,
        );
        assert_eq!(view.drop.queue.len(), 2);
        assert_eq!(view.drop.queue[0].1, Some(Point { x: 100., y: 2. }));
        assert_eq!(view.drop.queue[1].1, None);
        view.pending = true;
        assert!(!canvas::drain_drops(&mut view, &tx));
        view.pending = false;
        assert!(canvas::drain_drops(&mut view, &tx));
        match rx.try_recv().unwrap() {
            Job::Import {
                path,
                selected_id,
                point,
            } => {
                assert_eq!(path, Path::new("a.PNG"));
                assert_eq!(selected_id.as_deref(), Some(id.as_str()));
                assert_eq!(point, Some(Point { x: 100., y: 2. }));
            }
            _ => panic!("expected an import"),
        }
        // One edit in flight: the second file waits for the first response.
        assert!(!canvas::drain_drops(&mut view, &tx));
        view.pending = false;
        assert!(canvas::drain_drops(&mut view, &tx));
        assert!(matches!(
            rx.try_recv().unwrap(),
            Job::Import { point: None, .. }
        ));
        // Closing drops any queued files.
        view.drop.queue.push_back(("c.png".into(), None));
        view.pending = false;
        view.close_requested = true;
        assert!(!canvas::drain_drops(&mut view, &tx) && view.drop.queue.is_empty());
    }

    fn fixture() -> (tempfile::TempDir, String) {
        let data = tempfile::tempdir().unwrap();
        let image = RgbaImage::from_fn(7, 3, |x, y| {
            image::Rgba([x as u8 * 31, y as u8 * 71, 9, 255])
        });
        let artifact = captures_app::persist_screenshot(
            &data.path().join("history"),
            &image,
            captures_capture::CaptureMode::Region,
        )
        .unwrap();
        (data, artifact.entry.id)
    }

    /// Stand in for an encoded comparison without running an encode.
    fn fake_output(editor: &Editor) {
        let mut view = editor.view.lock().unwrap();
        let texture = view.texture.clone().unwrap();
        view.output = Some((texture, 1));
    }

    /// Drive the automatic comparison (skipping its refresh delay) until its
    /// off-worker encode settles.
    fn settle_compare(editor: &Editor, ctx: &egui::Context) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            {
                let mut view = editor.view.lock().unwrap();
                if view.compare_due.is_some() {
                    view.compare_due = Some(Instant::now());
                }
                view.drive_compare(ctx);
                if !view.compare_pending && view.compare_rx.is_none() {
                    return;
                }
            }
            assert!(Instant::now() < deadline, "comparison encode timed out");
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn receive(editor: &Editor, ctx: &egui::Context) {
        let reply = editor
            .rx
            .recv_timeout(Duration::from_secs(10))
            .expect("editor reply");
        editor.view.lock().unwrap().receive(ctx, reply);
    }

    fn crop() -> Request {
        Request::Crop {
            rect: Rect {
                x: 2.,
                y: 1.,
                width: 4.,
                height: 2.,
            },
        }
    }

    #[test]
    fn comparison_encodes_each_format_off_the_worker_and_reports_failures_in_the_frame() {
        let (data, id) = fixture();
        let ctx = egui::Context::default();
        let editor = Editor::open(
            &ctx,
            data.path().join("history"),
            id,
            data.path().join("exports"),
            CaptureMode::Region,
            |_| unreachable!("copy was not requested"),
        );
        receive(&editor, &ctx);
        editor.view.lock().unwrap().submit(&editor.tx, crop());
        receive(&editor, &ctx);
        let pixels = editor
            .view
            .lock()
            .unwrap()
            .presented
            .as_ref()
            .unwrap()
            .pixels
            .clone();
        {
            // Preserve and closed settings never encode a comparison.
            let mut view = editor.view.lock().unwrap();
            view.export_settings_open = true;
            assert!(!view.compare_visible());
            view.export_options.quality = ExportQuality::Compress;
            view.export_settings_open = false;
            assert!(!view.compare_visible());
            view.export_settings_open = true;
            assert!(view.compare_visible());
        }
        for format in [ExportFormat::Png, ExportFormat::Jpeg, ExportFormat::Webp] {
            {
                let mut view = editor.view.lock().unwrap();
                view.export_options.format = format;
                view.export_options.quality_value = 98;
                view.invalidate_output();
            }
            settle_compare(&editor, &ctx);
            let view = editor.view.lock().unwrap();
            let (texture, length) = view.output.as_ref().unwrap();
            assert_eq!(texture.size(), [4, 2]);
            assert!(*length > 0 && view.compare_error.is_none());
            let badges = view.compare_badges();
            assert!(badges.after.starts_with("After · "), "{badges:?}");
            assert!(view.unsaved() && !view.pending && !view.closed);
            assert!(Arc::ptr_eq(
                &pixels,
                &view.presented.as_ref().unwrap().pixels
            ));
            assert!(
                editor.rx.try_recv().is_err(),
                "the session worker never encodes it"
            );
        }
        {
            let mut view = editor.view.lock().unwrap();
            view.export_options.quality = ExportQuality::Maximum;
            view.export_options.max_size_bytes = Some(0);
            view.invalidate_output();
        }
        settle_compare(&editor, &ctx);
        {
            let mut view = editor.view.lock().unwrap();
            assert!(
                view.compare_error
                    .as_deref()
                    .is_some_and(|error| error.contains("10 KB"))
            );
            assert!(view.output.is_none() && view.error.is_none());
            assert!(view.unsaved() && !view.pending && !view.closed);
            view.export_options.max_size_bytes = Some(export::DEFAULT_MAX_SIZE_BYTES);
        }
        settle_compare(&editor, &ctx);
        assert!(editor.view.lock().unwrap().output.is_some());
        editor
            .view
            .lock()
            .unwrap()
            .submit(&editor.tx, Request::Undo);
        receive(&editor, &ctx);
        let view = editor.view.lock().unwrap();
        assert!(view.output.is_none());
        assert!(view.presented.as_ref().unwrap().can_redo);
        assert!(!data.path().join("editor-drafts").exists());
    }

    #[test]
    fn output_worker_previews_and_saves_asymmetric_size_without_resizing_session_pixels() {
        let (data, id) = fixture();
        let ctx = egui::Context::default();
        let editor = Editor::open(
            &ctx,
            data.path().join("history"),
            id,
            data.path().join("exports"),
            CaptureMode::Region,
            |_| unreachable!("copy was not requested"),
        );
        receive(&editor, &ctx);
        editor.view.lock().unwrap().submit(&editor.tx, crop());
        receive(&editor, &ctx);
        let session_pixels = editor
            .view
            .lock()
            .unwrap()
            .presented
            .as_ref()
            .unwrap()
            .pixels
            .clone();
        {
            let mut view = editor.view.lock().unwrap();
            view.export_options.size = ExportSize::Custom {
                width: 3,
                height: 1,
            };
            view.export_options.quality = ExportQuality::Compress;
            view.export_options.quality_value = 98;
            view.export_settings_open = true;
        }
        settle_compare(&editor, &ctx);
        {
            let view = editor.view.lock().unwrap();
            assert_eq!(view.output.as_ref().unwrap().0.size(), [3, 1]);
            let pixels = &view.presented.as_ref().unwrap().pixels;
            assert_eq!(pixels.dimensions(), (4, 2));
            assert!(Arc::ptr_eq(&session_pixels, pixels));
        }

        let destination = data.path().join("exports").join("asymmetric.png");
        {
            let mut view = editor.view.lock().unwrap();
            view.update_export_target(|target, _| target.set_stem("asymmetric"));
            view.save(&editor.tx);
        }
        let saved = editor
            .rx
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .unwrap();
        assert_eq!(
            image::load_from_memory(&fs::read(destination).unwrap())
                .unwrap()
                .into_rgba8()
                .dimensions(),
            (3, 1)
        );
        assert_eq!(saved.pixels.dimensions(), (4, 2));
        assert!(Arc::ptr_eq(&session_pixels, &saved.pixels));
    }

    #[test]
    fn quit_flush_drains_queued_edits_and_save_failure_can_retry() {
        let (data, id) = fixture();
        let ctx = egui::Context::default();
        let editor = Editor::open(
            &ctx,
            data.path().join("history"),
            id.clone(),
            data.path().join("exports"),
            CaptureMode::Region,
            |_| unreachable!("copy was not requested"),
        );
        receive(&editor, &ctx);
        fs::write(data.path().join("editor-drafts"), b"blocked").unwrap();
        editor.view.lock().unwrap().submit(&editor.tx, crop());
        assert!(editor.flush(&ctx).is_err());
        assert!(!editor.closed());
        fs::remove_file(data.path().join("editor-drafts")).unwrap();
        editor.flush(&ctx).unwrap();
        drop(editor);
        let reopened = EditorSession::open(OpenRequest {
            history_root: data.path().join("history"),
            drafts_root: data.path().join("editor-drafts"),
            artifact_id: id,
        })
        .unwrap();
        assert_eq!(reopened.pixels().dimensions(), (4, 2));
        assert_eq!(reopened.pixels().get_pixel(0, 0).0, [62, 71, 9, 255]);
        assert!(reopened.snapshot().has_draft);
    }

    #[test]
    fn close_without_saving_retains_the_previous_draft_not_the_latest_edit() {
        let (data, id) = fixture();
        let ctx = egui::Context::default();
        let editor = Editor::open(
            &ctx,
            data.path().join("history"),
            id.clone(),
            data.path().join("exports"),
            CaptureMode::Region,
            |_| unreachable!("copy was not requested"),
        );
        receive(&editor, &ctx);
        editor.view.lock().unwrap().submit(&editor.tx, crop());
        receive(&editor, &ctx);
        editor
            .view
            .lock()
            .unwrap()
            .submit(&editor.tx, Request::SaveDraft { updated_at_ms: 42 });
        receive(&editor, &ctx);
        editor.view.lock().unwrap().submit(
            &editor.tx,
            Request::ResizeCanvas {
                width: 8.,
                height: 5.,
            },
        );
        receive(&editor, &ctx);
        assert!(editor.view.lock().unwrap().unsaved());
        editor.view.lock().unwrap().closed = true;
        editor.flush(&ctx).unwrap(); // Application quit must respect the close decision.
        drop(editor);
        let reopened = EditorSession::open(OpenRequest {
            history_root: data.path().join("history"),
            drafts_root: data.path().join("editor-drafts"),
            artifact_id: id,
        })
        .unwrap();
        assert_eq!(reopened.pixels().dimensions(), (4, 2));
    }

    #[test]
    fn sans_only_draft_offers_every_bundled_family_and_pins_one_on_use() {
        let (data, id) = fixture();
        let request = || OpenRequest {
            history_root: data.path().join("history"),
            drafts_root: data.path().join("editor-drafts"),
            artifact_id: id.clone(),
        };
        let bundled = captures_app::editor_fonts::bundled();
        let mut legacy = bundled.clone();
        legacy.families.retain(|key, _| key == "sans");
        legacy
            .files
            .retain(|key, _| key.starts_with("liberation-sans-"));
        let mut author = EditorSession::open_with_fonts(request(), Some(legacy.clone())).unwrap();
        author
            .execute(Request::ResizeCanvas {
                width: 320.,
                height: 120.,
            })
            .unwrap();
        author
            .execute(Request::CreateText {
                create: TextCreate {
                    point: Point { x: 20., y: 30. },
                    text: "Native".into(),
                    font_size: 40.,
                    font_family: "sans".into(),
                    color: "#111111".into(),
                    style_preset: None,
                    drop_shadow: None,
                    drop_shadow_style: None,
                },
            })
            .unwrap();
        author
            .execute(Request::SaveDraft { updated_at_ms: 90 })
            .unwrap();
        let sans_pixels = author.pixels();
        drop(author);

        let ctx = egui::Context::default();
        let editor = Editor::open(
            &ctx,
            data.path().join("history"),
            id.clone(),
            data.path().join("exports"),
            CaptureMode::Region,
            |_| unreachable!("copy was not requested"),
        );
        receive(&editor, &ctx);
        let layer = {
            let view = editor.view.lock().unwrap();
            let presented = view.presented.as_ref().unwrap();
            // Shipping's Font menu offers all four families on every document.
            assert_eq!(presented.font_families, bundled.families);
            assert_eq!(
                font_family_options(&presented.font_families)
                    .into_iter()
                    .map(|(_, label)| label)
                    .collect::<Vec<_>>(),
                ["Sans serif", "Serif", "Monospace", "Rounded"]
            );
            assert_eq!(view.new_text_preset.as_deref(), Some("rounded-box"));
            assert_eq!(*presented.pixels, *sans_pixels);
            presented
                .document
                .elements
                .last()
                .unwrap()
                .base()
                .id
                .clone()
        };
        editor.view.lock().unwrap().submit(
            &editor.tx,
            Request::EditText {
                id: layer,
                patch: TextPatch {
                    font_family: Some("rounded".into()),
                    ..TextPatch::default()
                },
            },
        );
        receive(&editor, &ctx);
        {
            let view = editor.view.lock().unwrap();
            assert!(view.error.is_none(), "{:?}", view.error);
            assert_ne!(*view.presented.as_ref().unwrap().pixels, *sans_pixels);
        }
        editor
            .view
            .lock()
            .unwrap()
            .submit(&editor.tx, Request::SaveDraft { updated_at_ms: 91 });
        receive(&editor, &ctx);
        let saved = captures_history::editor_draft::load(
            &data.path().join("editor-drafts"),
            &id,
            |_, id| format!("draft-asset:{id}"),
        )
        .unwrap()
        .unwrap()
        .fonts
        .unwrap();
        assert_eq!(
            saved
                .families
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["rounded", "sans"]
        );
        assert_eq!(
            saved
                .files
                .keys()
                .filter(|key| key.starts_with("nunito-rounded-"))
                .count(),
            4
        );
    }

    #[test]
    fn replace_original_worker_preserves_editor_and_refreshes_same_history_item() {
        let (data, id) = fixture();
        let root = data.path().join("history");
        let mut entry = captures_history::load(&root, chrono::Utc::now())
            .unwrap()
            .remove(0);
        let destination = data.path().join("original.png");
        fs::copy(root.join(&id).join("capture.png"), &destination).unwrap();
        entry.saved_path = Some(destination.to_string_lossy().into_owned());
        captures_history::update_metadata(&root, &entry).unwrap();
        let ctx = egui::Context::default();
        let editor = Editor::open(
            &ctx,
            root.clone(),
            id.clone(),
            data.path().into(),
            CaptureMode::Region,
            |_| Ok(()),
        );
        receive(&editor, &ctx);
        editor.view.lock().unwrap().submit(&editor.tx, crop());
        receive(&editor, &ctx);
        editor
            .view
            .lock()
            .unwrap()
            .submit(&editor.tx, Request::SaveDraft { updated_at_ms: 44 });
        receive(&editor, &ctx);
        fake_output(&editor);
        let draft_path = data
            .path()
            .join("editor-drafts")
            .join(&id)
            .join("manifest.json");
        let draft = fs::read(&draft_path).unwrap();
        let document = editor
            .view
            .lock()
            .unwrap()
            .presented
            .as_ref()
            .unwrap()
            .document
            .clone();
        {
            let mut view = editor.view.lock().unwrap();
            assert!(view.output.is_some());
            // Save overwrites the saved original by default, without a second step.
            view.save(&editor.tx);
        }
        receive(&editor, &ctx);
        assert!(editor.take_history_changed() && editor.take_original_replaced());
        assert!(!editor.take_original_replaced());
        let output = image::open(&destination).unwrap().to_rgba8();
        assert_eq!(output.dimensions(), (4, 2));
        assert_eq!(output.get_pixel(0, 0).0, [62, 71, 9, 255]);
        assert_eq!(output.get_pixel(3, 1).0, [155, 142, 9, 255]);
        let history = captures_history::load(&root, chrono::Utc::now()).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].id, id);
        assert_eq!(history[0].created_at, entry.created_at);
        assert_eq!(fs::read(draft_path).unwrap(), draft);
        let view = editor.view.lock().unwrap();
        assert!(view.output.is_some() && view.presented.as_ref().unwrap().can_undo);
        assert_eq!(view.presented.as_ref().unwrap().document, document);
        assert_eq!(
            view.output_notice.as_deref(),
            Some("Saved changes to the original")
        );
        assert_eq!(view.last_saved.as_deref(), Some(destination.as_path()));
        assert_eq!(
            view.original_bytes,
            Some(fs::metadata(&destination).unwrap().len())
        );
    }

    #[test]
    fn every_save_reveals_the_saved_file_and_a_failed_handoff_names_it() {
        let (data, id) = fixture();
        let ctx = egui::Context::default();
        let root = data.path().join("history");
        let editor = Editor::open(
            &ctx,
            root,
            id,
            data.path().join("exports"),
            CaptureMode::Region,
            |_| unreachable!("copy was not requested"),
        );
        receive(&editor, &ctx);
        let revealed = Arc::new(Mutex::new(Vec::<PathBuf>::new()));
        let fail = Arc::new(std::sync::atomic::AtomicBool::new(false));
        {
            let (revealed, fail) = (revealed.clone(), fail.clone());
            editor.view.lock().unwrap().reveal_file = Some(Arc::new(move |path: &Path| {
                revealed.lock().unwrap().push(path.to_owned());
                if fail.load(std::sync::atomic::Ordering::SeqCst) {
                    Err(std::io::Error::other("no file manager"))
                } else {
                    Ok(())
                }
            }));
        }
        let settle = |editor: &Editor| {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let mut view = editor.view.lock().unwrap();
                view.drive_reveal();
                if view.reveal_rx.is_none() {
                    break;
                }
                drop(view);
                assert!(Instant::now() < deadline, "reveal never finished");
                thread::sleep(Duration::from_millis(5));
            }
        };
        editor.view.lock().unwrap().submit(&editor.tx, crop());
        receive(&editor, &ctx);
        let destination = data.path().join("exports").join("edited.png");
        {
            let mut view = editor.view.lock().unwrap();
            view.update_export_target(|target, _| target.set_stem("edited"));
            assert!(view.reveal_rx.is_none(), "editing never reveals anything");
            view.save(&editor.tx);
        }
        receive(&editor, &ctx);
        settle(&editor);
        // A new file: its folder opens and the notice stays as saved.
        assert_eq!(
            revealed.lock().unwrap().as_slice(),
            std::slice::from_ref(&destination)
        );
        assert_eq!(
            editor.view.lock().unwrap().output_notice.as_deref(),
            Some(format!("Saved {}", destination.display()).as_str())
        );
        // The adopted file is overwritten next; a failed handoff keeps the
        // file and says the folder could not be opened.
        fail.store(true, std::sync::atomic::Ordering::SeqCst);
        editor.view.lock().unwrap().save(&editor.tx);
        receive(&editor, &ctx);
        settle(&editor);
        assert_eq!(
            *revealed.lock().unwrap(),
            [destination.clone(), destination.clone()]
        );
        assert!(destination.is_file());
        let view = editor.view.lock().unwrap();
        assert_eq!(
            view.output_notice.as_deref(),
            Some(export::reveal_failed_notice(&destination).as_str())
        );
        assert!(view.export_error.is_none());
        assert_eq!(view.last_saved.as_deref(), Some(destination.as_path()));
    }

    #[test]
    fn save_copy_preserves_edits_and_original_and_recovers_from_collision_and_history_failure() {
        let (data, id) = fixture();
        let ctx = egui::Context::default();
        let root = data.path().join("history");
        let original_path = root.join(&id).join("capture.png");
        let original = fs::read(&original_path).unwrap();
        let editor = Editor::open(
            &ctx,
            root.clone(),
            id,
            data.path().join("exports"),
            CaptureMode::Region,
            |_| unreachable!("copy was not requested"),
        );
        receive(&editor, &ctx);
        editor.view.lock().unwrap().submit(&editor.tx, crop());
        receive(&editor, &ctx);
        let destination = data.path().join("exports").join("edited.png");
        {
            let mut view = editor.view.lock().unwrap();
            view.update_export_target(|target, _| target.set_stem("edited"));
            view.save(&editor.tx);
        }
        receive(&editor, &ctx);
        assert!(editor.take_history_changed());
        assert!(!editor.take_history_changed());
        let bytes = fs::read(&destination).unwrap();
        let image = image::load_from_memory(&bytes).unwrap().into_rgba8();
        assert_eq!(image.dimensions(), (4, 2));
        assert_eq!(image.get_pixel(0, 0).0, [62, 71, 9, 255]);
        assert_eq!(image.get_pixel(3, 1).0, [155, 142, 9, 255]);
        assert_eq!(fs::read_dir(&root).unwrap().count(), 2);
        assert!(!data.path().join("editor-drafts").exists());
        {
            let mut view = editor.view.lock().unwrap();
            assert!(view.unsaved() && view.presented.as_ref().unwrap().can_undo);
            assert_eq!(
                view.output_notice.as_deref(),
                Some(format!("Saved {}", destination.display()).as_str())
            );
            assert_eq!(view.last_saved.as_deref(), Some(destination.as_path()));
            // The saved file becomes the original: the next Save overwrites it.
            let Some(ExportSource { path, .. }) = &view.export_target.as_ref().unwrap().source
            else {
                panic!("saved file was not adopted")
            };
            assert_eq!(path, &destination);
            view.save(&editor.tx);
        }
        receive(&editor, &ctx);
        assert!(editor.take_history_changed());
        assert!(
            !editor.take_original_replaced(),
            "the editor's own original was not touched"
        );
        assert_eq!(fs::read(&destination).unwrap(), bytes);
        assert_eq!(fs::read_dir(&root).unwrap().count(), 2);
        {
            let mut view = editor.view.lock().unwrap();
            assert_eq!(
                view.output_notice.as_deref(),
                Some("Saved changes to the original")
            );
            // Save as new file with the same name must fail, not silently overwrite.
            view.update_export_target(|target, format| {
                target.set_save_as_new(true, format);
                target.set_stem("edited");
            });
            view.save(&editor.tx);
        }
        receive(&editor, &ctx);
        {
            let view = editor.view.lock().unwrap();
            assert_eq!(
                view.export_error.as_deref(),
                Some("edited.png already exists. Choose another filename.")
            );
            assert!(view.error.is_none() && view.unsaved() && !view.pending && !view.closed);
            assert!(view.output_notice.is_none());
        }
        assert!(!editor.take_history_changed());
        assert_eq!(fs::read(&destination).unwrap(), bytes);
        assert_eq!(fs::read(&original_path).unwrap(), original);
        assert_eq!(fs::read_dir(&root).unwrap().count(), 2);

        // The retained session can publish even if History becomes unavailable.
        fs::rename(&root, data.path().join("previous-history")).unwrap();
        fs::write(&root, b"blocked").unwrap();
        let recovered = data.path().join("exports").join("recovered.png");
        {
            let mut view = editor.view.lock().unwrap();
            view.update_export_target(|target, _| target.set_stem("recovered"));
            assert!(
                view.export_error.is_none(),
                "editing the filename clears the error"
            );
            view.save(&editor.tx);
            view.request_close(); // A pending export must complete before close handling.
        }
        receive(&editor, &ctx);
        {
            let view = editor.view.lock().unwrap();
            assert!(view.error.is_none() && view.unsaved() && !view.closed && view.close_requested);
            let notice = view.output_notice.as_ref().unwrap();
            assert!(notice.contains("recovered.png") && notice.contains("History was not updated"));
            assert!(
                view.export_target.as_ref().unwrap().save_as_new,
                "a file without History is not adopted as the overwrite target"
            );
        }
        assert_eq!(fs::read(recovered).unwrap(), bytes);
        assert!(!editor.take_history_changed());
        assert!(!data.path().join("editor-drafts").exists());
    }

    #[test]
    fn copy_uses_edited_rgba_ignores_export_limits_and_retries_without_persisting() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let (data, id) = fixture();
        let ctx = egui::Context::default();
        let (copied, rx) = mpsc::channel();
        let fail_once = AtomicBool::new(true);
        let editor = Editor::open(
            &ctx,
            data.path().join("history"),
            id,
            data.path().join("exports"),
            CaptureMode::Region,
            move |pixels| {
                copied.send(pixels).unwrap();
                if fail_once.swap(false, Ordering::Relaxed) {
                    Err("Clipboard unavailable".into())
                } else {
                    Ok(())
                }
            },
        );
        receive(&editor, &ctx);
        editor.view.lock().unwrap().submit(&editor.tx, crop());
        receive(&editor, &ctx);
        editor.view.lock().unwrap().submit(
            &editor.tx,
            Request::ResizeCanvas {
                width: 8.,
                height: 5.,
            },
        );
        receive(&editor, &ctx);
        let mut document = editor
            .view
            .lock()
            .unwrap()
            .presented
            .as_ref()
            .unwrap()
            .document
            .as_ref()
            .clone();
        document.background = None; // Exercise alpha, not the default off-white canvas.
        editor
            .view
            .lock()
            .unwrap()
            .submit(&editor.tx, Request::Commit { document });
        receive(&editor, &ctx);
        let before = editor
            .view
            .lock()
            .unwrap()
            .presented
            .as_ref()
            .unwrap()
            .document
            .clone();
        {
            let mut view = editor.view.lock().unwrap();
            view.export_options.format = ExportFormat::Jpeg;
            view.export_options.max_size_bytes = Some(0); // Would reject any export, never a copy.
            view.copy(&editor.tx);
            assert!(view.pending);
        }
        receive(&editor, &ctx);
        {
            let mut view = editor.view.lock().unwrap();
            assert_eq!(view.export_error.as_deref(), Some("Clipboard unavailable"));
            assert!(view.error.is_none() && view.copied_until.is_none());
            assert!(
                view.output_notice.is_none() && view.unsaved() && !view.pending && !view.closed
            );
            assert_eq!(view.presented.as_ref().unwrap().document, before);
            view.copy(&editor.tx);
            view.request_close();
        }
        receive(&editor, &ctx);
        for _ in 0..2 {
            let pixels = rx.recv_timeout(Duration::from_secs(1)).unwrap();
            assert_eq!(pixels.dimensions(), (8, 5));
            assert_eq!(pixels.get_pixel(0, 0).0, [62, 71, 9, 255]);
            assert_eq!(pixels.get_pixel(3, 1).0, [155, 142, 9, 255]);
            assert_eq!(pixels.get_pixel(7, 4).0, [0, 0, 0, 0]);
        }
        assert!(!editor.take_history_changed());
        let view = editor.view.lock().unwrap();
        assert!(view.error.is_none() && view.unsaved() && view.close_requested && !view.closed);
        assert!(
            view.export_error.is_none(),
            "a successful retry clears the failure"
        );
        assert!(
            view.copied_until.is_some() && view.output_notice.is_none(),
            "Copy confirms on its own button, not in the save status"
        );
        assert_eq!(view.presented.as_ref().unwrap().document, before);
        assert!(view.presented.as_ref().unwrap().can_undo);
        assert!(!data.path().join("editor-drafts").exists());
        assert!(!data.path().join("exports").exists());
        assert_eq!(
            fs::read_dir(data.path().join("history")).unwrap().count(),
            1
        );
        drop(view);
        {
            let mut view = editor.view.lock().unwrap();
            view.close_requested = false;
            view.submit(&editor.tx, Request::Undo);
        }
        receive(&editor, &ctx);
        assert!(editor.view.lock().unwrap().output_notice.is_none());
    }

    #[test]
    fn quit_drains_accepted_output_before_saving_the_dirty_draft() {
        let (data, id) = fixture();
        let ctx = egui::Context::default();
        let (copied, rx) = mpsc::channel();
        let editor = Editor::open(
            &ctx,
            data.path().join("history"),
            id.clone(),
            data.path().join("exports"),
            CaptureMode::Region,
            move |pixels| {
                copied.send(pixels).unwrap();
                Ok(())
            },
        );
        receive(&editor, &ctx);
        editor.view.lock().unwrap().submit(&editor.tx, crop());
        let destination = data.path().join("exports").join("queued.png");
        editor
            .tx
            .send(Job::Save {
                plan: SavePlan::NewFile {
                    path: destination.clone(),
                },
                options: View::default().export_options,
            })
            .unwrap();
        editor.tx.send(Job::Copy).unwrap();
        let (folder_reply, folder_result) = mpsc::channel();
        editor.view.lock().unwrap().folder_picker = Some(folder_result);
        editor.flush(&ctx).unwrap();
        assert!(editor.view.lock().unwrap().folder_picker.is_some());
        drop(folder_reply); // Quit drained output without waiting for the native dialog.
        let pixels = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(pixels.dimensions(), (4, 2));
        assert_eq!(pixels.get_pixel(0, 0).0, [62, 71, 9, 255]);
        assert_eq!(
            image::open(destination)
                .unwrap()
                .into_rgba8()
                .get_pixel(0, 0)
                .0,
            [62, 71, 9, 255]
        );
        let reopened = EditorSession::open(OpenRequest {
            history_root: data.path().join("history"),
            drafts_root: data.path().join("editor-drafts"),
            artifact_id: id,
        })
        .unwrap();
        assert_eq!(reopened.pixels().dimensions(), (4, 2));
        assert!(editor.take_history_changed());
    }
}
