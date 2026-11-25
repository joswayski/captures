//! The screenshot editor on GTK4, following shipping `ScreenshotEditor.tsx`
//! (`styles/editor-image.css`) and the shared `captures_app::editor_*`
//! modules.
//!
//! One worker thread owns the `EditorSession`: rendering, undo, drafts and
//! export never run on the UI thread. The canvas is a retained render node
//! that draws the session's last frame as one texture, scaled and moved by
//! the viewport, so zoom, pan and pointer motion never re-rasterize the
//! image. Only a committed edit replaces the texture. Gesture previews (shape
//! outlines, move offsets, crop) are drawn as light vector nodes on top.
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc;

use captures_app::editor::{
    AlignmentGuide, ClosedShapeCreate, ClosedShapeKind, CropDrag, Document, Element, ElementStyle,
    FreehandPathCreate, GuideOrientation, LayerEdit, LayerPlacement, OpenShapeCreate,
    OpenShapeKind, Point, Rect, ResizeHandle, TextBounds,
};
use captures_app::editor_canvas::{CanvasExpandPreview, CanvasTrimPreview, CurveEdit, CurveHandle};
use captures_app::editor_chrome::{self, RAIL_TOOLS, SHAPE_TOOLS, brush_cursor, header};
use captures_app::editor_export::{self, EstimateState, ExportSource, ExportTarget};
use captures_app::editor_image_background::BrushMode;
use captures_app::editor_session::{
    DraftAutosave, EditorSession, ExportFormat, ExportOptions, ExportQuality, ExportSize,
    ImageDropGuide, OpenRequest, PngOptions, Request,
};
use captures_app::editor_viewport::{self, Viewport};
use gtk::prelude::*;
use gtk::{gdk, glib, graphene, gsk};
use image::RgbaImage;

use crate::switch::TokenSwitch;
use crate::tokens::{self, LiveTokens, Tokens};
use crate::wake::{Outbox, Wake};
use crate::widgets::{Icon, MaxWidth, SnapshotArea, WidthWatch, icon_button};

mod canvas_bar;
mod canvas_chrome;
pub(crate) mod color_picker;
pub(crate) mod compare;
pub(crate) mod controls;
mod export_bar;
mod inspector;
mod layer_menu;
pub(crate) mod properties;
mod sidebar_resize;
mod text;
mod text_quick_bar;
pub use text::register_fonts;

/// Shipping's layer clipboard marker type.
const LAYER_CLIPBOARD_TYPE: &str = "application/x-captures-editor-layer";

/// Shipping Fit: `(clientWidth - 56) / width`, so 28 px a side.
const FIT_INSET: f64 = 28.;

pub struct Options {
    pub history_root: PathBuf,
    pub artifact_id: String,
    pub output_directory: PathBuf,
    /// The entry's `createdAt`, for the shipping new-file name.
    pub created_at: String,
}

/// What the worker publishes after every edit.
struct Presented {
    pixels: Arc<RgbaImage>,
    document: Document,
    layers: Vec<LayerItem>,
    outlines: BTreeMap<String, [Point; 4]>,
    can_undo: bool,
    can_redo: bool,
    can_trim: bool,
    /// Layers Merge down applies to (each above an unlocked neighbour).
    merge_down_ids: Vec<String>,
    can_merge_visible: bool,
    can_flatten: bool,
    style: ElementStyle,
    has_draft: bool,
    source: Option<ExportSource>,
    initial_text_size: f64,
    /// Document family keys to installed face names (`"rounded"` → Nunito).
    font_families: BTreeMap<String, String>,
    active_input: Option<ActiveInput>,
    /// Font-measured text geometry, for grip hit tests.
    text_bounds: TextBounds,
    /// Overflow preview per visible layer hanging past the canvas.
    canvas_expand: BTreeMap<String, CanvasExpandPreview>,
    /// What Trim edges would cut, for its hover preview.
    trim_preview: Option<CanvasTrimPreview>,
}

/// A layer gesture's uncommitted frame: pixels, outlines and snap guides.
struct GestureFrame {
    pixels: Arc<RgbaImage>,
    outlines: BTreeMap<String, [Point; 4]>,
    guides: Vec<AlignmentGuide>,
}

/// The session's live text input, when one is active.
#[derive(Clone)]
struct ActiveInput {
    input_id: String,
    layer_id: String,
    layout: Option<captures_app::editor_text::InlineEditorLayout>,
}

struct LayerItem {
    id: String,
    name: String,
    kind: &'static str,
    icon: &'static str,
    visible: bool,
    locked: bool,
    /// The session's cached ≤64 px row preview; the same `Arc` until the
    /// layer changes, so the host uploads each thumbnail once.
    thumbnail: Option<Arc<RgbaImage>>,
}

enum Job {
    Apply(Request),
    /// An uncommitted brush stroke, rendered without touching history.
    Preview(Request),
    /// Decode image files (or one pasted PNG) and import them as one edit.
    Import {
        source: ImportSource,
        selected_id: Option<String>,
        point: Option<Point>,
    },
    Save {
        plan: editor_export::SavePlan,
        options: ExportOptions,
        mode: captures_capture::CaptureMode,
    },
    /// Write the draft in the background, `DraftAutosave::DELAY` after an
    /// edit, as the Windows and macOS editors do.
    Autosave,
}

enum ImportSource {
    Files(Vec<PathBuf>),
    Pasted(Vec<u8>),
}

enum Outcome {
    Presented(Box<Presented>),
    /// Images were imported; select the last new layer.
    Imported(Box<Presented>, String),
    Preview(Result<GestureFrame, String>),
    Saved(
        Result<
            (
                editor_export::SavePlan,
                captures_app::editor_output::SavedExport,
            ),
            String,
        >,
    ),
    Failed(String),
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

/// Refresh the row thumbnails (only changed layers re-render), then present.
fn present(session: &mut EditorSession, artifact_id: &str) -> Presented {
    session.refresh_layer_thumbnails();
    let session = &*session;
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
                thumbnail: session.layer_thumbnail(&base.id),
            }
        })
        .collect();
    Presented {
        pixels: session.pixels(),
        document: snapshot.document.clone(),
        layers,
        outlines: snapshot
            .selection_outlines
            .iter()
            .map(|(id, outline)| ((*id).to_owned(), *outline))
            .collect(),
        can_undo: snapshot.can_undo,
        can_redo: snapshot.can_redo,
        can_trim: snapshot.can_trim,
        merge_down_ids: snapshot
            .merge_down_ids
            .iter()
            .map(|id| (*id).to_owned())
            .collect(),
        can_merge_visible: snapshot.can_merge_visible,
        can_flatten: snapshot.can_flatten,
        style: snapshot.initial_annotation_style.clone(),
        has_draft: snapshot.has_draft,
        initial_text_size: snapshot.initial_text_size,
        font_families: snapshot.font_families.cloned().unwrap_or_default(),
        active_input: snapshot
            .active_text_input
            .as_ref()
            .map(|input| ActiveInput {
                input_id: input.input_id.to_owned(),
                layer_id: input.layer_id.to_owned(),
                layout: input.layout.clone(),
            }),
        text_bounds: snapshot.text_bounds.clone(),
        canvas_expand: snapshot
            .canvas_expand
            .iter()
            .map(|(id, preview)| ((*id).to_owned(), preview.clone()))
            .collect(),
        trim_preview: snapshot.trim_preview.clone(),
        source: snapshot.original_export_path.map(|path| ExportSource {
            artifact_id: snapshot
                .source_artifact_id
                .unwrap_or(artifact_id)
                .to_owned(),
            path: path.to_owned(),
        }),
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Gesture {
    /// Drawing a shape, line, arrow or crop from `start`.
    Shape {
        tool: String,
        start: Point,
        end: Point,
    },
    Freehand(Vec<Point>),
    /// Staging a crop rectangle through the shared drag geometry.
    Crop(CropDrag),
    /// An Eraser Erase/Restore stroke.
    Brush(Vec<Point>),
    /// Moving the selected layer by the pointer's document delta.
    Move {
        id: String,
        start: Point,
        end: Point,
    },
    /// Dragging a grip on the selected layer: resize, rotate or a curve dot.
    Layer {
        id: String,
        drag: LayerDrag,
        start: Point,
        current: Point,
        shift: bool,
    },
    /// Panning from the viewport it started with.
    Pan {
        from: Viewport,
        dx: f64,
        dy: f64,
    },
}

#[derive(Clone, Debug, PartialEq)]
enum LayerDrag {
    Resize(ResizeHandle),
    /// The outline and rotation the drag started from.
    Rotate {
        outline: [Point; 4],
        initial: f64,
    },
    Curve(CurveHandle),
}

/// A layer drop target: the row id and which side of it.
type DropTarget = (String, LayerPlacement);
/// What a layer row shows: id, name, kind, icon, visible, locked,
/// thumbnail image and whether its name is being edited.
type LayerRowKey = (
    String,
    String,
    &'static str,
    &'static str,
    bool,
    bool,
    Option<usize>,
    bool,
);
/// Set after construction, once the editor state can be captured.
type PresetHandler = Rc<RefCell<Option<Box<dyn Fn(String)>>>>;

/// Takes each new canvas texture (`Editor::connect_canvas_changed`).
type CanvasChanged = Box<dyn Fn(&gdk::Texture)>;

pub struct Editor {
    pub window: gtk::ApplicationWindow,
    state: Rc<State>,
}

struct State {
    tokens: LiveTokens,
    output_directory: PathBuf,
    default_stem: String,
    mode: captures_capture::CaptureMode,
    jobs: mpsc::Sender<Job>,
    session: Arc<std::sync::Mutex<Option<EditorSession>>>,
    presented: RefCell<Option<Presented>>,
    texture: RefCell<Option<gdk::Texture>>,
    /// Takes each new canvas texture, for the capture's mini preview.
    canvas_changed: RefCell<CanvasChanged>,
    viewport: Cell<Viewport>,
    tool: RefCell<String>,
    last_shape: RefCell<String>,
    selected: RefCell<Option<String>>,
    gesture: RefCell<Option<Gesture>>,
    /// Shipping's `cropSelection`: staged until Apply crop.
    crop: Cell<Option<Rect>>,
    target: RefCell<Option<ExportTarget>>,
    options: Cell<ExportOptions>,
    notice: RefCell<Option<(String, bool)>>,
    busy: Cell<bool>,
    /// A Save is running. Only the export bar waits for it (shipping
    /// `busy === "saving"`); edits in flight leave the bar as it is.
    saving: Cell<bool>,
    canvas: SnapshotArea,
    /// The Text tool's inline field over the canvas.
    text_overlay: gtk::Overlay,
    text_field: text::TextField,
    inline: RefCell<Option<text::Inline>>,
    input_counter: Cell<u64>,
    /// The quick format bar over a selected text layer under Select.
    quick_bar: text_quick_bar::QuickBar,
    /// The layer the quick bar shows, while it is shown.
    quick_for: RefCell<Option<String>>,
    properties: properties::Properties,
    /// The pointer over the canvas, for the brush ring and wheel zoom.
    pointer: Cell<Option<(f64, f64)>>,
    /// A brush preview is rendering; newer strokes wait for it.
    preview_busy: Cell<bool>,
    queued: RefCell<std::collections::VecDeque<(Option<String>, Request)>>,
    /// The open Add images chooser; GtkNativeDialog does not keep itself alive.
    #[allow(deprecated)]
    chooser: RefCell<Option<gtk::FileChooserNative>>,
    /// The Est. size shown, the inputs it was (or is being) encoded for,
    /// its generation and the debounce before the next encode.
    estimate: Cell<EstimateState>,
    estimate_key: Cell<Option<(usize, ExportOptions, bool)>>,
    estimate_generation: Cell<u64>,
    estimate_timer: RefCell<Option<glib::SourceId>>,
    /// The History entry's file size, the Preserve baseline.
    original_bytes: Option<u64>,
    me: std::rc::Weak<State>,
    /// The image layer whose name is being edited in its row.
    renaming: RefCell<Option<String>>,
    /// A layer row drag: the moved id, and the drop target once it moves.
    layer_drag: RefCell<Option<(String, Option<DropTarget>)>>,
    /// The open layer settings card, and the layer a menu action selects.
    layer_menu: RefCell<Option<Rc<layer_menu::LayerMenu>>>,
    pending_select: RefCell<Option<String>>,
    /// Uploaded row thumbnails by layer id, keyed by the image they show.
    thumbnails: RefCell<std::collections::HashMap<String, (usize, gdk::Texture)>>,
    /// The layer rows shown, each with what it was built from and its ⋯
    /// button, so an edit replaces only the rows it changed.
    layer_rows: RefCell<Vec<(LayerRowKey, gtk::ListBoxRow, gtk::Button)>>,
    /// The Compress before/after comparison over the canvas.
    compare: Rc<compare::Compare>,
    /// Whether the last canvas frame suppressed it (a gesture or inline text).
    compare_suppressed: Cell<bool>,
    /// Set while the zoom readouts follow the viewport, so the slider's
    /// change handler ignores it.
    zoom_syncing: Cell<bool>,
    /// The percent the zoom readouts last showed (Fit follows the window).
    zoom_synced: Cell<f64>,
    preview_dirty: Cell<bool>,
    /// The previous primary press, for double-click editing.
    last_press: Cell<Option<(std::time::Instant, f64, f64)>>,
    /// The marker this editor last put on the clipboard with a copied layer.
    layer_marker: RefCell<Option<String>>,
    /// The latest preview of a grip drag on the worker.
    gesture_frame: RefCell<Option<GestureFrame>>,
    /// Shipping's Expand canvas action, the layer it grows the canvas for,
    /// and whether the pointer is on it (the ghost shows then).
    expand_action: gtk::Button,
    expand_for: RefCell<Option<String>>,
    expand_armed: Cell<bool>,
    /// The live drop guide while image files are dragged over the window,
    /// and the toast that names it.
    drop_guide: RefCell<Option<ImageDropGuide>>,
    drop_toast: gtk::Box,
    drop_label: gtk::Label,
    /// Shipping `trimEdgesHover`: Trim edges is hovered or focused.
    trim_hover: Cell<bool>,
    /// Shipping `panReady`: Ctrl is held outside a control, so a drag pans
    /// with any tool.
    pan_ready: Cell<bool>,
    /// The pending background draft save and its wake-up.
    autosave: RefCell<DraftAutosave>,
    autosave_timer: RefCell<Option<glib::SourceId>>,
    widgets: Widgets,
}

struct Widgets {
    undo: gtk::Button,
    redo: gtk::Button,
    canvas_bar: Rc<canvas_bar::CanvasBar>,
    /// Fit, Zoom out and Zoom in.
    zoom_buttons: [gtk::Button; 3],
    zoom_slider: gtk::Scale,
    zoom_preset: Rc<controls::Choice>,
    /// The preset select's handler, bound once the state exists.
    zoom_preset_changed: PresetHandler,
    rail: Vec<(String, gtk::Button)>,
    shapes: gtk::Button,
    shapes_icon: Icon,
    layer_count: gtk::Label,
    layers: gtk::ListBox,
    banner: gtk::Box,
    filename: gtk::Entry,
    export: Rc<export_bar::ExportBar>,
    change_folder: gtk::Button,
    folder: gtk::Label,
    hint: gtk::Label,
    notice: gtk::Label,
    save: gtk::Button,
    save_as_new: TokenSwitch,
    copy: gtk::Button,
    add_images: gtk::Button,
    add_layer: gtk::Button,
}

/// Hide empty status lines. The status box itself stays, so a compact
/// footer keeps its 20 px row and a notice never moves the canvas.
fn show_status(notice: &gtk::Label, hint: &gtk::Label) {
    notice.set_visible(!notice.label().is_empty());
    hint.set_visible(!hint.label().is_empty());
}

/// A folder under the home folder reads as `~/…`.
fn home_relative(directory: &std::path::Path) -> String {
    match directory.strip_prefix(glib::home_dir()) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => directory.display().to_string(),
    }
}

fn label(text: &str, class: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.add_css_class(class);
    label.set_xalign(0.);
    label
}

/// Shipping's `title` naming a control's keys, and its `aria-keyshortcuts`.
fn shortcut(widget: &impl IsA<gtk::Widget>, tooltip: &str, keys: &str) {
    let widget = widget.upcast_ref::<gtk::Widget>();
    widget.set_tooltip_text(Some(tooltip));
    widget.update_property(&[gtk::accessible::Property::KeyShortcuts(keys)]);
}

fn icon_only(icon: &str, side: i32, name: &str, class: &str) -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class(class);
    button.set_child(Some(&Icon::new(icon, side).with_stroke(1.75)));
    button.set_tooltip_text(Some(name));
    button.update_property(&[gtk::accessible::Property::Label(name)]);
    button
}

pub fn build(
    app: &gtk::Application,
    tokens: Rc<Tokens>,
    options: Options,
    mode: captures_capture::CaptureMode,
) -> Editor {
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title(captures_app::app_windows::SCREENSHOT_EDITOR_TITLE)
        .default_width(1280)
        .default_height(840)
        .build();
    window.add_css_class("captures");
    let live = LiveTokens::new(tokens.clone());
    window.set_size_request(860, 600);

    // The worker thread owns the session; the UI thread reads results.
    let session: Arc<std::sync::Mutex<Option<EditorSession>>> = Arc::default();
    let (jobs, inbox) = mpsc::channel::<Job>();
    let (results, outcomes) = std::sync::mpsc::channel::<Outcome>();
    // Worker results wake the UI thread; nothing polls while the editor idles.
    let late: Rc<RefCell<std::rc::Weak<State>>> = Rc::default();
    let wake = {
        let late = late.clone();
        Wake::new(move || {
            let Some(state) = late.borrow().upgrade() else {
                return;
            };
            while let Ok(outcome) = outcomes.try_recv() {
                state.receive(outcome);
            }
        })
    };
    let results = Outbox::new(results, wake.clone());
    {
        let session = session.clone();
        let (root, id) = (options.history_root.clone(), options.artifact_id.clone());
        std::thread::Builder::new()
            .name("captures-editor".into())
            .spawn(move || {
                let opened = EditorSession::open_with_fonts(
                    OpenRequest {
                        drafts_root: root.with_file_name("editor-drafts"),
                        history_root: root.clone(),
                        artifact_id: id.clone(),
                        source_artifact_id: None,
                    },
                    Some(captures_app::editor_fonts::bundled()),
                );
                match opened {
                    Ok(mut opened) => {
                        let _ =
                            results.send(Outcome::Presented(Box::new(present(&mut opened, &id))));
                        *session.lock().expect("session") = Some(opened);
                    }
                    Err(error) => {
                        let _ = results.send(Outcome::Failed(error));
                        return;
                    }
                }
                while let Ok(job) = inbox.recv() {
                    let mut guard = session.lock().expect("session");
                    let Some(session) = guard.as_mut() else { break };
                    let outcome = match job {
                        Job::Apply(request) => match session.execute(request) {
                            Ok(()) => Outcome::Presented(Box::new(present(session, &id))),
                            Err(error) => Outcome::Failed(error),
                        },
                        Job::Preview(request) => {
                            Outcome::Preview(session.preview_drawing_with_geometry(request).map(
                                |frame| GestureFrame {
                                    pixels: frame.pixels,
                                    outlines: frame.selection_outlines,
                                    guides: frame.guides,
                                },
                            ))
                        }
                        Job::Import {
                            source,
                            selected_id,
                            point,
                        } => match match source {
                            ImportSource::Files(paths) => {
                                captures_app::editor_image_decode::decode_imports(paths)
                            }
                            ImportSource::Pasted(bytes) => {
                                captures_app::editor_image_decode::decode_import_bytes(
                                    &bytes,
                                    "Pasted image",
                                )
                                .map(|image| vec![("Pasted image".to_owned(), image)])
                            }
                        }
                        .and_then(|images| session.import_images(images, selected_id, point))
                        {
                            Ok(layer) => Outcome::Imported(Box::new(present(session, &id)), layer),
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
                        Job::Autosave => {
                            if let Err(error) = session.execute(Request::AutosaveDraft {
                                updated_at_ms: now_ms(),
                            }) {
                                eprintln!("draft autosave failed: {error}");
                            }
                            continue;
                        }
                    };
                    if results.send(outcome).is_err() {
                        break;
                    }
                }
            })
            .expect("editor worker");
    }

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.add_css_class("screenshot-editor");
    // Shipping's `@media (max-width: 1040px)` layout, bound once the chrome
    // exists below.
    type Relayout = Rc<RefCell<Option<Rc<dyn Fn(i32)>>>>;
    let relayout: Relayout = Rc::default();
    let watch = WidthWatch::new(&root, {
        let relayout = relayout.clone();
        move |width| {
            if let Some(relayout) = relayout.borrow().clone() {
                // Layout may not change during allocation.
                glib::idle_add_local_once(move || relayout(width));
            }
        }
    });
    window.set_child(Some(&watch));
    window.connect_realize(|w| crate::capture_bar::point_at_buttons(w.upcast_ref()));

    // `.screenshot-editor-header`.
    let header_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    header_box.add_css_class("editor-header");
    let banner = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    banner.add_css_class("editor-draft-banner");
    let banner_text = label(header::DRAFT_RESTORED, "editor-banner-text");
    banner_text.set_hexpand(true);
    banner.append(&banner_text);
    let dismiss = gtk::Button::with_label(header::DRAFT_DISMISS);
    dismiss.add_css_class("ui-btn");
    dismiss.add_css_class("ghost");
    dismiss.update_property(&[gtk::accessible::Property::Label(
        header::DRAFT_DISMISS_LABEL,
    )]);
    banner.append(&dismiss);
    banner.set_visible(false);
    {
        let banner = banner.clone();
        dismiss.connect_clicked(move |_| banner.set_visible(false));
    }
    let main = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    main.add_css_class("editor-header-main");
    let left = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    left.set_hexpand(true);
    // Shipping clips the canvas toolbar in a narrow window instead of
    // widening the window: no scrollbar, and no minimum width.
    let left_clip = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::External)
        .vscrollbar_policy(gtk::PolicyType::Never)
        .propagate_natural_width(true)
        .propagate_natural_height(true)
        .hexpand(true)
        .child(&left)
        .build();
    // Requests from the canvas toolbar reach the worker once State exists.
    let canvas_jobs = jobs.clone();
    let canvas_bar = canvas_bar::CanvasBar::new(&live, move |request| {
        let _ = canvas_jobs.send(Job::Apply(request));
    });
    left.append(&canvas_bar.root);
    main.append(&left_clip);
    let right = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let undo = icon_only("undo", 16, header::UNDO, "editor-icon-button");
    let redo = icon_only("redo", 16, header::REDO, "editor-icon-button");
    shortcut(&undo, "Undo (Ctrl+Z)", "Control+Z");
    shortcut(
        &redo,
        "Redo (Ctrl+Shift+Z or Ctrl+Y)",
        "Control+Shift+Z Control+Y",
    );
    right.set_valign(gtk::Align::Center);
    for button in [&undo, &redo] {
        button.set_valign(gtk::Align::Center);
    }
    right.append(&undo);
    right.append(&redo);
    let zoom = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    zoom.add_css_class("editor-group");
    zoom.add_css_class("editor-zoom");
    zoom.set_valign(gtk::Align::Center);
    zoom.update_property(&[gtk::accessible::Property::Label(header::ZOOM_GROUP)]);
    let fit = icon_only("fit", 14, header::FIT_TOOLTIP, "editor-zoom-button");
    fit.update_property(&[gtk::accessible::Property::Label(header::FIT)]);
    let zoom_out = icon_only("minus", 14, header::ZOOM_OUT, "editor-zoom-button");
    shortcut(&zoom_out, "Zoom out (Ctrl+−)", "Control+Minus");
    // `.screenshot-editor-zoom-slider`: shipping's logarithmic 5–800% range.
    let zoom_slider = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0., 1., 0.001);
    zoom_slider.add_css_class("editor-zoom-slider");
    zoom_slider.set_draw_value(false);
    zoom_slider.set_tooltip_text(Some(header::ZOOM_SLIDER_TOOLTIP));
    zoom_slider.update_property(&[gtk::accessible::Property::Label(header::ZOOM_SLIDER)]);
    let slider_wrap = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    slider_wrap.add_css_class("editor-zoom-slider-wrap");
    // The slider fills `zoom_slider_width` less the wrap's `--s-4` padding.
    zoom_slider.set_size_request(
        editor_chrome::header_layout(1280.).zoom_slider_width as i32 - 16,
        -1,
    );
    slider_wrap.append(&zoom_slider);
    let zoom_in = icon_only("plus", 14, header::ZOOM_IN, "editor-zoom-button");
    shortcut(&zoom_in, "Zoom in (Ctrl++)", "Control+Plus");
    let zoom_preset_changed: PresetHandler = Rc::default();
    let zoom_preset = {
        let changed = zoom_preset_changed.clone();
        controls::Choice::new(None, header::ZOOM_PRESET, false, move |value| {
            if let Some(changed) = changed.borrow().as_ref() {
                changed(value);
            }
        })
    };
    zoom_preset.root.add_css_class("editor-zoom-presets");
    zoom_preset.root.set_size_request(
        editor_chrome::header_layout(1280.).zoom_preset_width as i32,
        -1,
    );
    zoom_preset.fill_height();
    zoom_preset.set_tooltip(header::ZOOM_PRESET_TOOLTIP);
    for widget in [
        fit.upcast_ref::<gtk::Widget>(),
        zoom_out.upcast_ref(),
        slider_wrap.upcast_ref(),
        zoom_in.upcast_ref(),
        zoom_preset.root.upcast_ref(),
    ] {
        zoom.append(widget);
    }
    right.append(&zoom);
    let (add_images, _, _) = icon_button("image", header::ADD_IMAGES, &["editor-add-image"]);
    add_images.set_valign(gtk::Align::Center);
    right.append(&add_images);
    main.append(&right);
    header_box.append(&banner);
    header_box.append(&main);
    root.append(&header_box);

    // Rail, canvas and Layers.
    let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    body.set_vexpand(true);
    let rail = gtk::Box::new(gtk::Orientation::Vertical, 2);
    rail.add_css_class("editor-rail");
    rail.update_property(&[gtk::accessible::Property::Label("Tools")]);
    let mut rail_buttons = Vec::new();
    let mut shapes = None;
    let mut shapes_icon = None;
    for tool in RAIL_TOOLS {
        let button = gtk::Button::new();
        button.add_css_class("editor-rail-button");
        // Shipping's 38 px squares, centred in the rail instead of stretched.
        button.set_halign(gtk::Align::Center);
        let icon = Icon::new(tool.icon, 18).with_stroke(1.75);
        if tool.key == editor_chrome::SHAPES_KEY {
            // Shipping's flyout cue: a 7 px corner wedge in the text colour.
            let overlay = gtk::Overlay::new();
            // The button's 36 px content box, so the cue sits in its corner
            // (expanding would stretch the rail instead).
            overlay.set_size_request(36, 36);
            icon.set_halign(gtk::Align::Center);
            icon.set_valign(gtk::Align::Center);
            overlay.set_child(Some(&icon));
            let wedge = SnapshotArea::new(|_, _, _| {});
            let weak = wedge.downgrade();
            wedge.set_draw(move |snapshot, w, h| {
                let Some(wedge) = weak.upgrade() else { return };
                #[allow(deprecated)] // `gtk_widget_get_color` needs GTK 4.10.
                let color = wedge.style_context().color();
                let cr = snapshot.append_cairo(&graphene::Rect::new(0., 0., w, h));
                cr.set_source_rgba(
                    f64::from(color.red()),
                    f64::from(color.green()),
                    f64::from(color.blue()),
                    f64::from(color.alpha()),
                );
                cr.move_to(f64::from(w), 0.);
                cr.line_to(f64::from(w), f64::from(h));
                cr.line_to(0., f64::from(h));
                cr.close_path();
                let _ = cr.fill();
            });
            wedge.set_margin_end(4);
            wedge.set_margin_bottom(4);
            wedge.add_css_class("editor-flyout-cue");
            wedge.set_size_request(7, 7);
            wedge.set_halign(gtk::Align::End);
            wedge.set_valign(gtk::Align::End);
            wedge.set_can_target(false);
            overlay.add_overlay(&wedge);
            button.set_child(Some(&overlay));
        } else {
            button.set_child(Some(&icon));
        }
        let name = if tool.key == editor_chrome::SHAPES_KEY {
            editor_chrome::shapes_tooltip("rectangle")
        } else {
            tool.name()
        };
        button.set_tooltip_text(Some(&name));
        button.update_property(&[gtk::accessible::Property::Label(&name)]);
        rail.append(&button);
        if tool.key == editor_chrome::SHAPES_KEY {
            shapes = Some(button.clone());
            shapes_icon = Some(icon);
        }
        rail_buttons.push((tool.key.to_owned(), button));
    }
    body.append(&rail);
    let canvas = SnapshotArea::new(|_, _, _| {});
    canvas.set_hexpand(true);
    canvas.set_vexpand(true);
    canvas.set_focusable(true);
    canvas.add_css_class("editor-canvas");
    canvas.set_accessible_role(gtk::AccessibleRole::Img);
    canvas.update_property(&[gtk::accessible::Property::Label("Screenshot canvas")]);
    let text_overlay = gtk::Overlay::new();
    text_overlay.set_child(Some(&canvas));
    let text_field = text::TextField::new();
    text_overlay.add_overlay(&text_field.view);
    let compare = {
        let (canvas, overlay) = (canvas.clone(), text_overlay.clone());
        compare::Compare::new(move || {
            canvas.queue_draw();
            overlay.queue_allocate();
        })
    };
    for widget in [
        compare.before.upcast_ref::<gtk::Widget>(),
        compare.after.upcast_ref(),
        compare.hide.upcast_ref(),
        compare.range.upcast_ref(),
        compare.handle.upcast_ref(),
    ] {
        widget.set_visible(false);
        text_overlay.add_overlay(widget);
    }
    // `.screenshot-canvas-expand-action`.
    let expand_action = gtk::Button::with_label(captures_app::editor_canvas::EXPAND_CANVAS);
    expand_action.add_css_class("editor-expand-action");
    expand_action.set_visible(false);
    text_overlay.add_overlay(&expand_action);
    let quick_bar = text_quick_bar::QuickBar::new();
    text_overlay.add_overlay(&quick_bar.root);
    text_overlay.set_hexpand(true);
    body.append(&text_overlay);
    let panel = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let heading = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    heading.add_css_class("editor-layers-heading");
    heading.append(&label("Layers", "editor-layers-title"));
    let layer_count = gtk::Label::new(Some("0"));
    layer_count.add_css_class("editor-layer-count");
    layer_count.set_valign(gtk::Align::Center);
    heading.append(&layer_count);
    // `.screenshot-layers-heading button`: Add image layer.
    let add_layer = icon_only("plus", 16, editor_chrome::layers::ADD, "editor-layers-add");
    add_layer.set_hexpand(true);
    add_layer.set_halign(gtk::Align::End);
    add_layer.set_valign(gtk::Align::Center);
    heading.append(&add_layer);
    panel.append(&heading);
    let layers = gtk::ListBox::new();
    layers.add_css_class("editor-layer-list");
    layers.set_selection_mode(gtk::SelectionMode::Single);
    layers.update_property(&[gtk::accessible::Property::Label("Layers")]);
    let layer_scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&layers)
        .build();
    panel.append(&layer_scroller);
    let properties = properties::Properties::new(&live);
    let sidebar = properties::sidebar(&panel, &properties.root);
    sidebar.add_css_class("editor-layers");
    body.append(&sidebar);
    // The resize separator straddles the panel's left edge.
    let resize = sidebar_resize::SidebarResize::new(&live, &sidebar);
    let body_overlay = gtk::Overlay::new();
    body_overlay.set_child(Some(&body));
    body_overlay.add_overlay(&resize.handle);
    // `.screenshot-drop-overlay`: the glass toast naming where a dragged
    // image will land, 72 px from the window top and centred on it.
    let drop_toast = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    drop_toast.add_css_class("editor-drop-toast");
    drop_toast.set_can_target(false);
    drop_toast.set_visible(false);
    let drop_icon = Icon::new("image", 22).with_stroke(1.7);
    drop_icon.set_valign(gtk::Align::Center);
    drop_toast.append(&drop_icon);
    let drop_label = gtk::Label::new(Some(captures_app::editor_session::DROP_IMAGE));
    drop_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    drop_label.set_max_width_chars(28);
    drop_toast.append(&drop_label);
    body_overlay.add_overlay(&drop_toast);
    {
        let sidebar = sidebar.clone();
        let handle = resize.handle.clone();
        body_overlay.connect_get_child_position(move |overlay, child| {
            if child != handle.upcast_ref::<gtk::Widget>() {
                let window = overlay.root()?;
                let (_, natural) = child.preferred_size();
                let top = graphene::Point::new(window.width() as f32 / 2., 72.);
                let at = window.compute_point(overlay, &top)?;
                return Some(gdk::Rectangle::new(
                    at.x().round() as i32 - natural.width() / 2,
                    at.y().round() as i32,
                    natural.width(),
                    natural.height(),
                ));
            }
            let at = sidebar.compute_point(overlay, &graphene::Point::new(0., 0.))?;
            Some(gdk::Rectangle::new(
                at.x().round() as i32 - sidebar_resize::HANDLE / 2,
                at.y().round() as i32,
                sidebar_resize::HANDLE,
                sidebar.height(),
            ))
        });
    }
    root.append(&body_overlay);

    // `.screenshot-export-bar`: the Export settings panel over the save row.
    let footer = gtk::Box::new(gtk::Orientation::Vertical, 12);
    footer.add_css_class("editor-export-bar");
    let export = {
        let late = late.clone();
        export_bar::ExportBar::new(move |options| {
            if let Some(state) = late.borrow().upgrade() {
                state.set_options(options);
            }
        })
    };
    footer.append(&export.panel);
    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    footer.append(&bar);
    export.disclosure.set_valign(gtk::Align::End);
    // `.screenshot-save-row`: minmax(178px, 210px), or minmax(170px, 200px)
    // in a compact window (`relayout`).
    export.disclosure.set_size_request(210, -1);
    export.disclosure.set_hexpand(false);
    bar.append(&export.disclosure);
    let file = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let file_heading = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    file_heading.append(&label("Filename", "editor-field-label"));
    let folder = label("", "editor-folder");
    folder.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    // `.recording-destination`: flex 1, packed to the end.
    // The whole folder when it fits (shipping's 190 px cap hid where files
    // go); a long one keeps both ends.
    folder.set_hexpand(true);
    folder.set_xalign(1.);
    file_heading.append(&folder);
    // Shipping `aria-label="Change save location"` over "Change…". GTK 4.14
    // names a button after its label child, so the text is presentational.
    let change_folder = gtk::Button::new();
    let change_text = gtk::Label::new(Some("Change…"));
    change_text.set_accessible_role(gtk::AccessibleRole::Presentation);
    change_folder.set_child(Some(&change_text));
    change_folder.add_css_class("editor-folder-change");
    change_folder.update_property(&[gtk::accessible::Property::Label("Change save location")]);
    file_heading.append(&change_folder);
    file.append(&file_heading);
    let file_row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    file_row.add_css_class("editor-filename");
    let filename = gtk::Entry::new();
    filename.add_css_class("editor-filename-entry");
    // The column (`minmax(240px, 390px)`) sizes the field, not its text.
    filename.set_width_chars(1);
    filename.set_max_width_chars(80);
    filename.set_hexpand(true);
    filename.update_property(&[gtk::accessible::Property::Label("Filename")]);
    file_row.append(&filename);
    file_row.append(&export.format.root);
    file.append(&file_row);
    file.set_size_request(240, -1);
    let file_column = MaxWidth::new(&file, 390);
    bar.append(&file_column);
    let (copy, _, _) = icon_button("copy", "Copy image", &["ui-btn"]);
    shortcut(
        &copy,
        "Copy the edited image to the clipboard. Does not save a file.",
        "Control+Shift+C",
    );
    copy.set_valign(gtk::Align::End);
    bar.append(&copy);
    let status = gtk::Box::new(gtk::Orientation::Vertical, 2);
    status.set_hexpand(true);
    status.set_valign(gtk::Align::Center);
    let notice = label("", "editor-notice");
    notice.set_xalign(1.);
    notice.set_ellipsize(gtk::pango::EllipsizeMode::Start);
    notice.set_accessible_role(gtk::AccessibleRole::Status);
    // Only warnings reach this line (`status_hint`).
    let hint = label("", "editor-hint");
    hint.add_css_class("warning");
    hint.set_xalign(1.);
    hint.set_wrap(true);
    hint.set_justify(gtk::Justification::Right);
    status.append(&notice);
    status.append(&hint);
    bar.append(&status);
    // `.screenshot-make-copy`: a 36 px row ruled off from Save on its right.
    let as_new = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    as_new.add_css_class("editor-make-copy");
    as_new.set_tooltip_text(Some("Save as a new file and leave the original untouched"));
    as_new.set_valign(gtk::Align::Center);
    as_new.set_size_request(-1, 36);
    let save_as_new = TokenSwitch::new(tokens.clone(), false);
    save_as_new.update_property(&[gtk::accessible::Property::Label("Save as new file")]);
    as_new.append(&save_as_new);
    as_new.append(&label("Save as new file", "editor-switch-label"));
    bar.append(&as_new);
    let (save, _, _) = icon_button("save", "Save", &["ui-btn", "primary", "editor-save"]);
    save.update_property(&[gtk::accessible::Property::KeyShortcuts("Control+S")]);
    save.set_valign(gtk::Align::Center);
    bar.append(&save);
    root.append(&footer);
    *relayout.borrow_mut() = Some(Rc::new({
        let compact_now: Cell<Option<bool>> = Cell::default();
        let resize = resize.clone();
        let (undo, redo) = (undo.clone(), redo.clone());
        let (zoom_slider, zoom_preset) = (zoom_slider.clone(), zoom_preset.root.clone());
        let (footer, bar, status, copy) =
            (footer.clone(), bar.clone(), status.clone(), copy.clone());
        let (disclosure, file, file_column) =
            (export.disclosure.clone(), file.clone(), file_column.clone());
        let (notice, hint) = (notice.clone(), hint.clone());
        let export = export.clone();
        move |width: i32| {
            resize.set_editor_width(width);
            let layout = editor_chrome::header_layout(f64::from(width));
            let compact = !layout.show_history;
            if compact_now.replace(Some(compact)) == Some(compact) {
                return;
            }
            undo.set_visible(!compact);
            redo.set_visible(!compact);
            zoom_slider.set_size_request(layout.zoom_slider_width as i32 - 16, -1);
            zoom_preset.set_size_request(layout.zoom_preset_width as i32, -1);
            export.set_compact(compact);
            // `.screenshot-save-row`: the status takes its own row under
            // the controls, left-aligned, and the filename column grows.
            bar.set_spacing(if compact { 8 } else { 12 });
            disclosure.set_size_request(if compact { 200 } else { 210 }, -1);
            file.set_size_request(if compact { 230 } else { 240 }, -1);
            file_column.set_max(if compact { MaxWidth::UNBOUNDED } else { 390 });
            file_column.set_hexpand(compact);
            let xalign = if compact { 0. } else { 1. };
            notice.set_xalign(xalign);
            hint.set_xalign(xalign);
            hint.set_justify(if compact {
                gtk::Justification::Left
            } else {
                gtk::Justification::Right
            });
            let home = if compact {
                footer.upcast_ref::<gtk::Widget>()
            } else {
                bar.upcast_ref()
            };
            if status.parent().as_ref() != Some(home) {
                status.unparent();
                if compact {
                    footer.append(&status);
                } else {
                    bar.insert_child_after(&status, Some(&copy));
                }
            }
            status.set_size_request(-1, if compact { 20 } else { -1 });
            show_status(&notice, &hint);
        }
    }));
    // Also decide from the requested width before the first layout: the
    // wide layout's minimum would otherwise hold a narrow window wider.
    {
        let relayout = relayout.clone();
        let decide = move |window: &gtk::ApplicationWindow| {
            if let Some(relayout) = relayout.borrow().clone() {
                relayout(window.default_width());
            }
        };
        let decide = Rc::new(decide);
        window.connect_realize({
            let decide = decide.clone();
            move |window| decide(window)
        });
        window.connect_default_width_notify(move |window| decide(window));
    }

    let output_directory = options.output_directory.clone();
    let original_bytes =
        editor_export::original_size_bytes(&options.history_root, &options.artifact_id);
    let state = Rc::new_cyclic(|me| State {
        tokens: live,
        output_directory,
        default_stem: captures_app::recording_editor_ui::timestamp_stem(&options.created_at),
        mode,
        jobs,
        session,
        presented: RefCell::default(),
        texture: RefCell::default(),
        canvas_changed: RefCell::new(Box::new(|_| {})),
        viewport: Cell::default(),
        tool: RefCell::new("v".into()),
        last_shape: RefCell::new("rectangle".into()),
        selected: RefCell::default(),
        gesture: RefCell::default(),
        crop: Cell::default(),
        target: RefCell::default(),
        options: Cell::new(ExportOptions {
            format: ExportFormat::Png,
            quality: ExportQuality::Preserve,
            quality_value: 98,
            max_size_bytes: None,
            png: PngOptions::default(),
            size: ExportSize::Original,
        }),
        notice: RefCell::default(),
        busy: Cell::new(true),
        saving: Cell::default(),
        canvas: canvas.clone(),
        text_overlay,
        text_field,
        inline: RefCell::default(),
        input_counter: Cell::default(),
        quick_bar,
        quick_for: RefCell::default(),
        last_press: Cell::default(),
        layer_marker: RefCell::default(),
        gesture_frame: RefCell::default(),
        expand_action,
        expand_for: RefCell::default(),
        expand_armed: Cell::default(),
        drop_guide: RefCell::default(),
        drop_toast,
        drop_label,
        trim_hover: Cell::default(),
        pan_ready: Cell::default(),
        autosave: RefCell::default(),
        autosave_timer: RefCell::default(),
        properties,
        pointer: Cell::default(),
        preview_busy: Cell::default(),
        queued: RefCell::default(),
        chooser: RefCell::default(),
        estimate: Cell::default(),
        estimate_key: Cell::default(),
        estimate_generation: Cell::default(),
        estimate_timer: RefCell::default(),
        original_bytes,
        me: me.clone(),
        renaming: RefCell::default(),
        layer_drag: RefCell::default(),
        layer_menu: RefCell::default(),
        pending_select: RefCell::default(),
        thumbnails: RefCell::default(),
        layer_rows: RefCell::default(),
        compare,
        compare_suppressed: Cell::default(),
        zoom_syncing: Cell::default(),
        zoom_synced: Cell::default(),
        preview_dirty: Cell::default(),
        widgets: Widgets {
            undo,
            redo,
            canvas_bar,
            zoom_buttons: [fit, zoom_out, zoom_in],
            zoom_slider,
            zoom_preset,
            zoom_preset_changed,
            rail: rail_buttons,
            shapes: shapes.expect("shapes tool"),
            shapes_icon: shapes_icon.expect("shapes icon"),
            layer_count,
            layers,
            banner,
            filename,
            export,
            change_folder,
            folder,
            hint,
            notice,
            save,
            save_as_new,
            copy,
            add_images,
            add_layer,
        },
    });
    *late.borrow_mut() = Rc::downgrade(&state);
    state.connect(&window);

    window.connect_destroy(move |_| wake.remove());

    // Flush the draft on close, as shipping's autosave does.
    let weak = Rc::downgrade(&state);
    window.connect_close_request(move |_| {
        if let Some(state) = weak.upgrade()
            && let Ok(mut session) = state.session.lock()
            && let Some(session) = session.as_mut()
        {
            if let Some(timer) = state.autosave_timer.take() {
                timer.remove();
            }
            state.autosave.borrow_mut().cancel();
            state.commit_text_for_close(session);
            let _ = session.execute(Request::AutosaveDraft {
                updated_at_ms: now_ms(),
            });
        }
        glib::Propagation::Proceed
    });
    state.select_tool("v");
    // Shipping opens with the canvas focused, not the filename field.
    GtkWindowExt::set_focus(&window, Some(&state.canvas));
    Editor { window, state }
}

impl Editor {
    pub fn present(&self) {
        self.window.present();
    }

    /// Follow the canvas as shown, brush and layer drag previews included.
    pub fn connect_canvas_changed(&self, f: impl Fn(&gdk::Texture) + 'static) {
        *self.state.canvas_changed.borrow_mut() = Box::new(f);
    }

    /// Preferences changed the appearance or accent: draw with `tokens` from
    /// now on (the CSS provider restyles the rest).
    pub fn set_tokens(&self, tokens: Rc<Tokens>) {
        self.state.widgets.save_as_new.set_tokens(tokens.clone());
        self.state.tokens.set(tokens);
        tokens::redraw_tree(self.window.upcast_ref());
    }

    /// Manual zoom (percent of image pixels) for the profile exercise.
    pub fn set_zoom(&self, percent: f64) {
        self.state.zoom_to(percent, None);
    }
}

impl State {
    fn connect(self: &Rc<Self>, window: &gtk::ApplicationWindow) {
        let w = &self.widgets;
        let weak = Rc::downgrade(self);
        w.undo.connect_clicked(move |_| {
            if let Some(state) = weak.upgrade() {
                state.submit(Request::Undo);
            }
        });
        let weak = Rc::downgrade(self);
        w.redo.connect_clicked(move |_| {
            if let Some(state) = weak.upgrade() {
                state.submit(Request::Redo);
            }
        });
        for (index, button) in self.widgets.zoom_buttons.iter().enumerate() {
            let weak = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                let Some(state) = weak.upgrade() else { return };
                match index {
                    0 => state.set_viewport(Viewport::default()),
                    1 => state.zoom_by(1. / 1.25),
                    _ => state.zoom_by(1.25),
                }
            });
        }
        let weak = Rc::downgrade(self);
        self.widgets
            .zoom_slider
            .connect_value_changed(move |slider| {
                let Some(state) = weak.upgrade() else { return };
                if state.zoom_syncing.get() {
                    return;
                }
                if let Some(percent) = editor_viewport::zoom_from_slider(slider.value()) {
                    state.zoom_to(percent, None);
                }
            });
        let weak = Rc::downgrade(self);
        *self.widgets.zoom_preset_changed.borrow_mut() = Some(Box::new(move |value| {
            let Some(state) = weak.upgrade() else { return };
            match value.parse::<f64>() {
                Ok(percent) => state.zoom_to(percent, None),
                Err(_) => state.set_viewport(Viewport::default()),
            }
        }));
        self.sync_zoom();
        for (key, button) in &w.rail {
            let weak = Rc::downgrade(self);
            let key = key.clone();
            button.connect_clicked(move |button| {
                let Some(state) = weak.upgrade() else { return };
                if key == editor_chrome::SHAPES_KEY {
                    // Shipping: from another tool, Shapes switches to the
                    // last shape and opens the flyout.
                    let tool = state.tool.borrow().clone();
                    if !SHAPE_TOOLS.iter().any(|shape| shape.key == tool) {
                        let last = state.last_shape.borrow().clone();
                        state.select_tool(&last);
                    }
                    state.shapes_menu(button);
                } else {
                    state.select_tool(&key);
                }
            });
        }
        let weak = Rc::downgrade(self);
        w.layers.connect_row_activated(move |_, row| {
            let Some(state) = weak.upgrade() else { return };
            let id = row.widget_name().to_string();
            *state.selected.borrow_mut() = Some(id);
            state.canvas.queue_draw();
        });
        self.connect_layer_drag();
        let weak = Rc::downgrade(self);
        w.copy.connect_clicked(move |_| {
            if let Some(state) = weak.upgrade() {
                state.copy();
            }
        });
        let weak = Rc::downgrade(self);
        w.save.connect_clicked(move |_| {
            if let Some(state) = weak.upgrade() {
                state.save();
            }
        });
        let weak = Rc::downgrade(self);
        w.filename.connect_changed(move |entry| {
            let Some(state) = weak.upgrade() else { return };
            let mut target = state.target.borrow_mut();
            if let Some(target) = target.as_mut()
                && target.stem != entry.text().as_str()
            {
                target.set_stem(entry.text().as_str());
            }
            drop(target);
            state.update_export();
        });
        let weak = Rc::downgrade(self);
        w.change_folder.connect_clicked(move |_| {
            if let Some(state) = weak.upgrade() {
                state.choose_folder();
            }
        });
        let weak = Rc::downgrade(self);
        w.save_as_new.connect_toggled(move |on| {
            let Some(state) = weak.upgrade() else { return };
            let format = state.options.get().format;
            if let Some(target) = state.target.borrow_mut().as_mut() {
                target.set_save_as_new(on, format);
            }
            state.update_export();
        });
        self.connect_canvas(window);
    }
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

fn set_class(widget: &impl IsA<gtk::Widget>, class: &str, on: bool) {
    if on {
        widget.add_css_class(class);
    } else {
        widget.remove_css_class(class);
    }
}

fn texture_of(pixels: &RgbaImage) -> gdk::Texture {
    let (width, height) = pixels.dimensions();
    gdk::MemoryTexture::new(
        width as i32,
        height as i32,
        gdk::MemoryFormat::R8g8b8a8,
        &glib::Bytes::from(pixels.as_raw().as_slice()),
        width as usize * 4,
    )
    .upcast()
}

/// Image layers cover the well; annotation canvases already include padding.
fn thumbnail_area(texture: gdk::Texture, image: bool) -> SnapshotArea {
    let area = SnapshotArea::new(move |snapshot, width, height| {
        let (tw, th) = (texture.width() as f32, texture.height() as f32);
        let scale = (width / tw).max(height / th);
        let (dw, dh) = if image {
            (tw * scale, th * scale)
        } else {
            (width, height)
        };
        snapshot.push_clip(&graphene::Rect::new(0., 0., width, height));
        snapshot.append_texture(
            &texture,
            &graphene::Rect::new((width - dw) / 2., (height - dh) / 2., dw, dh),
        );
        snapshot.pop();
    });
    area.set_size_request(42, 30);
    area.set_overflow(gtk::Overflow::Hidden);
    area.set_accessible_role(gtk::AccessibleRole::Presentation);
    area
}

impl State {
    fn submit(&self, request: Request) {
        // Shipping drops a staged crop with Undo and Redo.
        if matches!(request, Request::Undo | Request::Redo) {
            self.set_crop(None);
        }
        self.busy.set(true);
        let _ = self.jobs.send(Job::Apply(request));
    }

    fn document_size(&self) -> Option<(f64, f64)> {
        self.presented
            .borrow()
            .as_ref()
            .map(|p| (p.document.width, p.document.height))
    }

    /// The fitted rect inside a `width × height` canvas (shipping Fit).
    fn fit(&self, width: f64, height: f64) -> Option<(Rect, f64, f64)> {
        let (dw, dh) = self.document_size()?;
        let scale = ((width - 2. * FIT_INSET) / dw)
            .max(0.02)
            .min(((height - 2. * FIT_INSET) / dh).max(0.02))
            .min(1.);
        Some((
            Rect {
                x: (width - dw * scale) / 2.,
                y: (height - dh * scale) / 2.,
                width: dw * scale,
                height: dh * scale,
            },
            dw,
            dh,
        ))
    }

    /// Where the document lands on the canvas now.
    fn image_rect(&self) -> Option<Rect> {
        let (fit, dw, dh) = self.fit(
            f64::from(self.canvas.width()),
            f64::from(self.canvas.height()),
        )?;
        self.viewport.get().rect(fit, dw, dh)
    }

    fn to_document(&self, x: f64, y: f64) -> Option<Point> {
        let rect = self.image_rect()?;
        let (dw, dh) = self.document_size()?;
        Some(Point {
            x: (x - rect.x) / rect.width * dw,
            y: (y - rect.y) / rect.height * dh,
        })
    }

    fn to_screen(&self, point: Point) -> Option<(f64, f64)> {
        let rect = self.image_rect()?;
        let (dw, dh) = self.document_size()?;
        Some((
            rect.x + point.x / dw * rect.width,
            rect.y + point.y / dh * rect.height,
        ))
    }

    /// Displayed percent of image pixels.
    fn zoom_percent(&self) -> Option<f64> {
        let rect = self.image_rect()?;
        let (dw, _) = self.document_size()?;
        Some(rect.width / dw * 100.)
    }

    fn set_viewport(&self, viewport: Viewport) {
        self.viewport.set(viewport);
        self.sync_zoom();
        // Only the canvas node changes; the texture is reused as is.
        self.canvas.queue_draw();
        self.text_overlay.queue_allocate();
        self.style_text_field();
    }

    /// Shipping's zoom readouts: Fit's active state, the slider at the
    /// displayed percent, the limits on − and +, and the preset select (with
    /// the manual value when it is not a preset).
    fn sync_zoom(&self) {
        let viewport = self.viewport.get();
        let fit = viewport.zoom_percent == 0.;
        let Some(shown) = self.zoom_percent() else {
            return;
        };
        self.zoom_synced.set(shown);
        let w = &self.widgets;
        if fit {
            w.zoom_buttons[0].add_css_class("active");
        } else {
            w.zoom_buttons[0].remove_css_class("active");
        }
        w.zoom_buttons[1].set_sensitive(shown > 5.05);
        w.zoom_buttons[2].set_sensitive(shown < 799.95);
        self.zoom_syncing.set(true);
        if let Some(position) = editor_viewport::zoom_slider_position(shown) {
            w.zoom_slider.set_value(position);
        }
        self.zoom_syncing.set(false);
        let label = editor_chrome::zoom_label(shown);
        w.zoom_slider
            .update_property(&[gtk::accessible::Property::ValueText(&if fit {
                format!("Fit ({label})")
            } else {
                label
            })]);
        let mut options = vec![("fit".to_owned(), "Fit".to_owned())];
        let manual = viewport.zoom_percent.to_string();
        if !fit && !header::ZOOM_PRESETS.contains(&viewport.zoom_percent) {
            options.push((
                manual.clone(),
                editor_chrome::zoom_label(viewport.zoom_percent),
            ));
        }
        options.extend(
            header::ZOOM_PRESETS
                .iter()
                .map(|preset| (preset.to_string(), format!("{preset}%"))),
        );
        w.zoom_preset.set_options(options);
        w.zoom_preset.set(if fit { "fit" } else { &manual });
    }

    fn zoom_to(&self, percent: f64, anchor: Option<(f64, f64)>) {
        let (width, height) = (
            f64::from(self.canvas.width()),
            f64::from(self.canvas.height()),
        );
        let Some((fit, dw, dh)) = self.fit(width, height) else {
            return;
        };
        let (x, y) = anchor.unwrap_or((width / 2., height / 2.));
        let mut viewport = self.viewport.get();
        if viewport.zoom_percent == 0. {
            viewport.zoom_percent = fit.width / dw * 100.;
        }
        if let Some(next) = viewport.zoom_at(fit, dw, dh, percent, Point { x, y }) {
            self.set_viewport(next);
        }
    }

    fn zoom_by(&self, factor: f64) {
        if let Some(percent) = self.zoom_percent() {
            self.zoom_to(percent * factor, None);
        }
    }

    fn select_tool(&self, key: &str) {
        if self.text_active() {
            self.finish_text_for_tool();
        }
        let shape = SHAPE_TOOLS.iter().find(|tool| tool.key == key);
        if let Some(shape) = shape {
            *self.last_shape.borrow_mut() = key.to_owned();
            self.widgets.shapes_icon.set_name(shape.icon);
            let name = editor_chrome::shapes_tooltip(key);
            self.widgets.shapes.set_tooltip_text(Some(&name));
            self.widgets
                .shapes
                .update_property(&[gtk::accessible::Property::Label(&name)]);
        }
        *self.tool.borrow_mut() = key.to_owned();
        if key != "c" {
            self.set_crop(None);
        }
        for (rail_key, button) in &self.widgets.rail {
            let active =
                rail_key == key || (rail_key == editor_chrome::SHAPES_KEY && shape.is_some());
            set_class(button, "active", active);
            button.update_state(&[gtk::accessible::State::Pressed(if active {
                gtk::AccessibleTristate::True
            } else {
                gtk::AccessibleTristate::False
            })]);
        }
        self.refresh_properties();
        self.sync_cursor();
    }

    /// The Properties heading and sections for the tool and selection.
    fn refresh_properties(&self) {
        let presented = self.presented.borrow();
        let selected = self.selected.borrow();
        let element = presented.as_ref().and_then(|presented| {
            let id = selected.as_ref()?;
            presented
                .document
                .elements
                .iter()
                .find(|element| element.base().id == *id)
        });
        let fonts = presented
            .as_ref()
            .map(|p| p.font_families.clone())
            .unwrap_or_default();
        self.properties.show(&self.tool.borrow(), element, &fonts);
        drop((presented, selected));
        self.sync_quick_bar();
    }

    /// The text layer the quick format bar is for: the selected unlocked,
    /// visible text layer under Select, while no inline input is open and
    /// nothing is being dragged.
    fn quick_bar_id(&self) -> Option<String> {
        if *self.tool.borrow() != "v"
            || self.text_active()
            || self.gesture.borrow().is_some()
            || self.gesture_frame.borrow().is_some()
        {
            return None;
        }
        let id = self.selected.borrow().clone()?;
        let presented = self.presented.borrow();
        presented
            .as_ref()?
            .document
            .elements
            .iter()
            .any(|element| {
                matches!(element, Element::Text(text)
                    if text.base.id == id && !text.base.locked && text.base.visible)
            })
            .then_some(id)
    }

    /// Show, fill or hide the quick bar (outside a snapshot).
    fn sync_quick_bar(&self) {
        let id = self.quick_bar_id();
        let target = id.as_ref().and_then(|id| {
            let presented = self.presented.borrow();
            let presented = presented.as_ref()?;
            let label = presented
                .document
                .elements
                .iter()
                .find_map(|element| match element {
                    Element::Text(text) if text.base.id == *id => Some(text.clone()),
                    _ => None,
                })?;
            Some((label, presented.font_families.clone()))
        });
        if let Some((label, fonts)) = &target {
            self.quick_bar.sync(label, fonts);
        }
        let shown = target.is_some();
        *self.quick_for.borrow_mut() = id.filter(|_| shown);
        let changed = self.quick_bar.root.is_visible() != shown;
        if changed {
            self.quick_bar.root.set_visible(shown);
        }
        if shown || changed {
            // Reallocate through `sync_expand`, which first hides an Expand
            // canvas action the new tool or selection no longer places; the
            // overlay would otherwise stretch it over the whole canvas.
            self.sync_expand();
        }
    }

    /// Where the quick bar sits: centred above the layer's selection box
    /// (and its rotation grip), below it without room, inside the canvas.
    fn quick_bar_rect(&self) -> Option<gdk::Rectangle> {
        let id = self.quick_for.borrow().clone()?;
        let presented = self.presented.borrow();
        let presented = presented.as_ref()?;
        let outline = presented.outlines.get(&id).copied()?;
        let rotation = presented
            .document
            .elements
            .iter()
            .find(|element| element.base().id == id)?
            .base()
            .rotation();
        let mut points = outline
            .iter()
            .map(|point| self.to_screen(*point))
            .collect::<Option<Vec<_>>>()?;
        if let Some(grip) = captures_app::editor::rotation_handle(
            outline,
            rotation,
            self.display_scale(),
            presented.document.width,
            presented.document.height,
        ) && let Some((x, y)) = self.to_screen(grip.handle)
        {
            // The grip's knob, so the bar never covers it.
            points.extend([(x - 8., y - 8.), (x + 8., y + 8.)]);
        }
        let (left, top, right, bottom) = points.iter().fold(
            (f64::MAX, f64::MAX, f64::MIN, f64::MIN),
            |(l, t, r, b), (x, y)| (l.min(*x), t.min(*y), r.max(*x), b.max(*y)),
        );
        let (width, height) = self.quick_bar.size();
        let [x, y] = editor_chrome::text_quick_bar::place(
            [left, top, right - left, bottom - top],
            [f64::from(width), f64::from(height)],
            [
                f64::from(self.canvas.width()),
                f64::from(self.canvas.height()),
            ],
        );
        Some(gdk::Rectangle::new(
            x.round() as i32,
            y.round() as i32,
            width,
            height,
        ))
    }

    /// A Properties edit for the selected layer. Edits made while the worker
    /// is busy queue; a newer live value for the same field replaces the
    /// queued one, so a slider drag renders at most one frame behind.
    fn property_edit(&self, edit: inspector::Edit) {
        let Some(id) = self.selected.borrow().clone() else {
            return;
        };
        let (request, live) = match edit {
            inspector::Edit::Layer { edit, live } => (
                Request::Layer {
                    id: id.clone(),
                    edit,
                },
                live,
            ),
            inspector::Edit::Text { patch, live } => (
                Request::EditText {
                    id: id.clone(),
                    patch,
                },
                live,
            ),
        };
        let (request, key) = match live {
            Some(field) => {
                let key = format!("{field}:{id}");
                (
                    Request::Live {
                        key: key.clone(),
                        request: Box::new(request),
                    },
                    Some(key),
                )
            }
            None => (request, None),
        };
        if !self.busy.get() {
            self.submit(request);
            return;
        }
        let mut queue = self.queued.borrow_mut();
        if key.is_some() && queue.back().is_some_and(|(queued, _)| *queued == key) {
            queue.pop_back();
        }
        queue.push_back((key, request));
    }

    /// Import image files as one edit, below the selection or at `point`.
    /// A drag point over the window, in document coordinates (outside the
    /// canvas too, as shipping's guide extrapolates).
    fn drop_point(&self, target: &gtk::DropTarget, x: f64, y: f64) -> Option<Point> {
        let window = target.widget()?;
        let at = window.compute_point(&self.canvas, &graphene::Point::new(x as f32, y as f32))?;
        self.to_document(at.x().into(), at.y().into())
    }

    /// Follow a file drag with shipping's drop guide and toast.
    fn drag_over(&self, target: &gtk::DropTarget, x: f64, y: f64) {
        let point = self.drop_point(target, x, y);
        let guide = self.presented.borrow().as_ref().map(|presented| {
            captures_app::editor_session::image_drop_guide(
                &presented.document,
                self.selected.borrow().as_deref(),
                point,
            )
        });
        let label = guide
            .as_ref()
            .map_or(captures_app::editor_session::DROP_IMAGE, |guide| {
                guide.label
            });
        if self.drop_label.label() != label {
            self.drop_label.set_label(label);
        }
        self.drop_toast.set_visible(true);
        *self.drop_guide.borrow_mut() = guide;
        self.canvas.queue_draw();
    }

    fn end_drag_over(&self) {
        self.drop_toast.set_visible(false);
        self.drop_guide.borrow_mut().take();
        self.canvas.queue_draw();
    }

    fn import_files(&self, paths: Vec<PathBuf>, point: Option<Point>) {
        if !paths.is_empty() {
            self.import(ImportSource::Files(paths), point);
        }
    }

    /// Import a pasted image as a new layer.
    fn import_png(&self, bytes: Vec<u8>) {
        self.import(ImportSource::Pasted(bytes), None);
    }

    fn import(&self, source: ImportSource, point: Option<Point>) {
        if self.text_active() {
            self.finish_text();
        }
        *self.notice.borrow_mut() = None;
        self.busy.set(true);
        let _ = self.jobs.send(Job::Import {
            source,
            selected_id: self.selected.borrow().clone(),
            point,
        });
    }

    /// Shipping's Add images: a multi-select image picker.
    fn choose_images(self: &Rc<Self>) {
        let window = self.canvas.root().and_downcast::<gtk::Window>();
        #[allow(deprecated)] // `GtkFileDialog` needs GTK 4.10.
        let chooser = gtk::FileChooserNative::new(
            Some(header::ADD_IMAGES),
            window.as_ref(),
            gtk::FileChooserAction::Open,
            Some("Add"),
            Some("Cancel"),
        );
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("Images"));
        filter.add_mime_type("image/*");
        #[allow(deprecated)]
        {
            chooser.add_filter(&filter);
            chooser.set_select_multiple(true);
        }
        let weak = Rc::downgrade(self);
        #[allow(deprecated)]
        chooser.connect_response(move |chooser, response| {
            let files = chooser.files();
            chooser.destroy();
            let Some(state) = weak.upgrade() else { return };
            state.chooser.borrow_mut().take();
            if response != gtk::ResponseType::Accept {
                return;
            }
            let paths = (0..files.n_items())
                .filter_map(|i| files.item(i).and_downcast::<gtk::gio::File>())
                .filter_map(|file| file.path())
                .collect();
            state.import_files(paths, None);
        });
        #[allow(deprecated)]
        chooser.show();
        *self.chooser.borrow_mut() = Some(chooser);
    }

    /// Send the next queued Properties edit once the worker is free.
    fn drain_queued(&self) {
        if let Some((_, request)) = self.queued.borrow_mut().pop_front() {
            self.submit(request);
        }
    }

    /// Erase/Restore replace the cursor with the brush ring over an image.
    fn brush_mode(&self) -> Option<BrushMode> {
        match (self.tool.borrow().as_str(), self.properties.eraser.mode()) {
            ("b", properties::EraserMode::Brush(mode)) => Some(mode),
            _ => None,
        }
    }

    /// A drag that moves the view, from the middle button or Ctrl.
    fn begin_pan(&self) {
        if self.text_active() {
            self.finish_text();
        }
        *self.gesture.borrow_mut() = Some(Gesture::Pan {
            from: self.viewport.get(),
            dx: 0.,
            dy: 0.,
        });
        self.sync_cursor();
        self.canvas.queue_draw();
    }

    fn set_pan_ready(&self, ready: bool) {
        if self.pan_ready.replace(ready) != ready {
            self.sync_cursor();
            self.canvas.queue_draw();
        }
    }

    /// Shipping's `interactiveTarget`: a field, button or slider has focus.
    fn control_focused(&self) -> bool {
        let mut widget = self.canvas.root().and_then(|root| root.focus());
        while let Some(current) = widget {
            if current.is::<gtk::Text>()
                || current.is::<gtk::TextView>()
                || current.is::<gtk::Button>()
                || current.is::<gtk::Range>()
                || current.is::<gtk::SpinButton>()
            {
                return true;
            }
            widget = current.parent();
        }
        false
    }

    fn sync_cursor(&self) {
        if matches!(*self.gesture.borrow(), Some(Gesture::Pan { .. })) {
            self.canvas.set_cursor_from_name(Some("grabbing"));
            return;
        }
        if self.pan_ready.get() {
            self.canvas.set_cursor_from_name(Some("grab"));
            return;
        }
        let key = self.tool.borrow().clone();
        let ring = self.brush_mode().is_some() && self.brush_hover() == brush_cursor::Hover::Ring;
        self.canvas.set_cursor_from_name(Some(match key.as_str() {
            _ if ring => "none",
            "b" if self.brush_mode().is_some() => "not-allowed",
            "v" => "default",
            "t" => "text",
            _ => "crosshair",
        }));
    }

    fn brush_hover(&self) -> brush_cursor::Hover {
        let over_image = self
            .pointer
            .get()
            .and_then(|(x, y)| self.to_document(x, y))
            .is_some_and(|point| {
                self.presented.borrow().as_ref().is_some_and(|presented| {
                    captures_app::editor_image_background::wand_target(&presented.document, point)
                        .is_some()
                })
            });
        let stroking = matches!(*self.gesture.borrow(), Some(Gesture::Brush(_)));
        let panning =
            matches!(*self.gesture.borrow(), Some(Gesture::Pan { .. })) || self.pan_ready.get();
        brush_cursor::hover(over_image, stroking, panning)
    }

    /// Render the brush stroke or grip drag so far on the worker, one
    /// request at a time.
    fn preview_brush(&self) {
        if self.preview_busy.get() {
            self.preview_dirty.set(true);
            return;
        }
        let gesture = self.gesture.borrow().clone();
        let request = match (gesture, self.brush_mode()) {
            (Some(Gesture::Brush(points)), Some(mode)) => {
                let eraser = &self.properties.eraser;
                Request::PaintImageBackground {
                    points,
                    size: eraser.size.value(),
                    softness: eraser.softness.value(),
                    mode,
                }
            }
            (Some(gesture @ Gesture::Layer { .. }), _) => {
                let Some(request) = self.layer_request(&gesture) else {
                    return;
                };
                request
            }
            _ => return,
        };
        self.preview_busy.set(true);
        self.preview_dirty.set(false);
        let _ = self.jobs.send(Job::Preview(request));
    }

    /// The layer edit a grip drag stands for now.
    fn layer_request(&self, gesture: &Gesture) -> Option<Request> {
        let Gesture::Layer {
            id,
            drag,
            start,
            current,
            shift,
        } = gesture
        else {
            return None;
        };
        let scale = self.zoom_percent().unwrap_or(100.) / 100.;
        let edit = match drag {
            LayerDrag::Resize(handle) => LayerEdit::Resize {
                handle: *handle,
                current: *current,
                display_scale: scale,
                lock_aspect: *shift,
            },
            LayerDrag::Rotate { outline, initial } => LayerEdit::Rotate {
                radians: canvas_chrome::rotation(*outline, *initial, *start, *current, *shift)?,
            },
            LayerDrag::Curve(handle) => LayerEdit::Curve {
                edit: CurveEdit::Move {
                    handle: *handle,
                    point: *current,
                },
            },
        };
        Some(Request::Layer {
            id: id.clone(),
            edit,
        })
    }

    /// The Shapes flyout (`repeat(3, 44px)`).
    fn shapes_menu(self: &Rc<Self>, anchor: &gtk::Button) {
        let popover = gtk::Popover::new();
        popover.add_css_class("captures-menu");
        popover.add_css_class("shape-flyout");
        popover.set_has_arrow(false);
        popover.set_position(gtk::PositionType::Right);
        // Shipping centres it on the rail button, 10 px to its right.
        popover.set_offset(8, 0);
        let grid = gtk::Grid::new();
        grid.set_row_spacing(4);
        grid.set_column_spacing(4);
        for (index, tool) in SHAPE_TOOLS.iter().enumerate() {
            let button = icon_only(tool.icon, 22, &tool.name(), "editor-shape-button");
            set_class(&button, "active", *self.last_shape.borrow() == tool.key);
            let weak = Rc::downgrade(self);
            let popover_weak = popover.downgrade();
            let key = tool.key;
            button.connect_clicked(move |_| {
                if let Some(state) = weak.upgrade() {
                    state.select_tool(key);
                }
                if let Some(popover) = popover_weak.upgrade() {
                    popover.popdown();
                }
            });
            grid.attach(
                &button,
                (index % editor_chrome::SHAPE_FLYOUT_COLUMNS) as i32,
                (index / editor_chrome::SHAPE_FLYOUT_COLUMNS) as i32,
                1,
                1,
            );
        }
        popover.set_child(Some(&grid));
        popover.set_parent(anchor);
        popover.connect_closed(|popover| {
            let popover = popover.clone();
            glib::idle_add_local_once(move || popover.unparent());
        });
        popover.popup();
    }

    /// Schedule the pending draft save. The session refuses drafts while a
    /// label is open; finishing it presents a new frame, which arms again.
    fn arm_autosave(&self) {
        if let Some(timer) = self.autosave_timer.take() {
            timer.remove();
        }
        let Some(due) = self.autosave.borrow().deadline() else {
            return;
        };
        let weak = self.me.clone();
        let delay = due.saturating_duration_since(std::time::Instant::now());
        let timer = glib::timeout_add_local_once(delay, move || {
            let Some(state) = weak.upgrade() else { return };
            // Fired: the source is gone, so drop its id without removing it.
            state.autosave_timer.take();
            if state.inline.borrow().is_some() {
                return;
            }
            if state
                .autosave
                .borrow_mut()
                .take_due(std::time::Instant::now())
            {
                let _ = state.jobs.send(Job::Autosave);
            } else {
                state.arm_autosave();
            }
        });
        *self.autosave_timer.borrow_mut() = Some(timer);
    }

    fn receive(self: &Rc<Self>, outcome: Outcome) {
        match outcome {
            Outcome::Imported(presented, layer) => {
                self.receive(Outcome::Presented(presented));
                *self.selected.borrow_mut() = Some(layer);
                self.rebuild_selection();
                self.canvas.queue_draw();
            }
            Outcome::Presented(presented) => {
                self.busy.set(false);
                self.gesture_frame.borrow_mut().take();
                self.drain_queued();
                let first = self.presented.borrow().is_none();
                let changed_pixels = self
                    .presented
                    .borrow()
                    .as_ref()
                    .is_none_or(|old| !Arc::ptr_eq(&old.pixels, &presented.pixels));
                if changed_pixels {
                    self.show_frame(&presented.pixels);
                }
                let old_count = self
                    .presented
                    .borrow()
                    .as_ref()
                    .map_or(0, |old| old.layers.len());
                let w = &self.widgets;
                w.canvas_bar.sync(
                    presented.document.width,
                    presented.document.height,
                    presented.document.background.as_deref(),
                    presented.can_trim,
                );
                w.undo.set_sensitive(presented.can_undo);
                w.redo.set_sensitive(presented.can_redo);
                w.layer_count.set_label(&presented.layers.len().to_string());
                if first {
                    let defaults = &self.properties.inspector.defaults.values;
                    defaults.text_size.set(presented.initial_text_size.round());
                    *defaults.style.borrow_mut() = presented.style.clone();
                    w.banner.set_visible(presented.has_draft);
                    *self.target.borrow_mut() = Some(ExportTarget::new(
                        presented.source.clone(),
                        &self.output_directory,
                        &self.default_stem,
                    ));
                } else {
                    self.autosave.borrow_mut().edited(std::time::Instant::now());
                    self.arm_autosave();
                }
                {
                    let mut selected = self.selected.borrow_mut();
                    if !first && presented.layers.len() > old_count {
                        // A new layer is on top; select it, as shipping does.
                        *selected = presented.layers.first().map(|layer| layer.id.clone());
                    } else if selected
                        .as_ref()
                        .is_some_and(|id| !presented.layers.iter().any(|l| l.id == *id))
                    {
                        *selected = None;
                    }
                    if let Some(id) = self.pending_select.borrow_mut().take()
                        && presented.layers.iter().any(|l| l.id == id)
                    {
                        *selected = Some(id);
                    }
                }
                *self.presented.borrow_mut() = Some(*presented);
                self.rebuild_layers();
                self.sync_layer_menu();
                if first {
                    self.sync_filename();
                }
                self.update_export();
                self.refresh_properties();
                self.received_text();
                self.canvas.queue_draw();
            }
            Outcome::Preview(result) => {
                self.preview_busy.set(false);
                let stroking = matches!(
                    *self.gesture.borrow(),
                    Some(Gesture::Brush(_) | Gesture::Layer { .. })
                );
                match result {
                    // Only a gesture still in progress shows its preview; the
                    // committed result replaces it once the worker publishes.
                    Ok(frame) if stroking => {
                        self.show_frame(&frame.pixels);
                        *self.gesture_frame.borrow_mut() = Some(frame);
                        self.canvas.queue_draw();
                    }
                    Ok(_) => {}
                    Err(error) => *self.notice.borrow_mut() = Some((error, true)),
                }
                if self.preview_dirty.get() {
                    self.preview_brush();
                }
            }
            Outcome::Saved(result) => {
                self.busy.set(false);
                self.saving.set(false);
                match result {
                    Ok((plan, saved)) => {
                        let text = editor_export::saved_notice(&plan, &saved);
                        if let Some(target) = self.target.borrow_mut().as_mut() {
                            target.record_saved(&saved);
                        }
                        *self.notice.borrow_mut() = Some((text, false));
                        self.sync_filename();
                    }
                    Err(error) => *self.notice.borrow_mut() = Some((error, true)),
                }
                self.update_export();
            }
            Outcome::Failed(error) => {
                self.busy.set(false);
                self.restore_frame();
                self.drain_queued();
                self.text_failed();
                *self.notice.borrow_mut() = Some((error, true));
                self.update_export();
                self.canvas.queue_draw();
            }
        }
    }

    fn sync_filename(&self) {
        // Release the borrow first: `set_text` re-enters `connect_changed`.
        let stem = self.target.borrow().as_ref().map(|t| t.stem.clone());
        if let Some(stem) = stem
            && self.widgets.filename.text().as_str() != stem
        {
            self.widgets.filename.set_text(&stem);
        }
    }

    /// Bring the layer rows up to date. Rows whose layer did not change stay
    /// (with their hover, focus and pressed state); only changed rows are
    /// rebuilt, and all of them only when the layer order changes.
    fn rebuild_layers(self: &Rc<Self>) {
        let list = &self.widgets.layers;
        let presented = self.presented.borrow();
        let mut rows = self.layer_rows.borrow_mut();
        let keys: Vec<LayerRowKey> = presented.as_ref().map_or_else(Vec::new, |presented| {
            let renaming = self.renaming.borrow();
            presented
                .layers
                .iter()
                .map(|layer| {
                    (
                        layer.id.clone(),
                        layer.name.clone(),
                        layer.kind,
                        layer.icon,
                        layer.visible,
                        layer.locked,
                        layer.thumbnail.as_ref().map(|t| Arc::as_ptr(t) as usize),
                        renaming.as_deref() == Some(layer.id.as_str()),
                    )
                })
                .collect()
        });
        let same_order =
            rows.len() == keys.len() && rows.iter().zip(&keys).all(|(row, key)| row.0.0 == key.0);
        if !same_order {
            while let Some(row) = list.first_child() {
                list.remove(&row);
            }
            rows.clear();
        }
        let Some(presented) = presented.as_ref() else {
            return;
        };
        self.thumbnails
            .borrow_mut()
            .retain(|id, _| presented.layers.iter().any(|layer| layer.id == *id));
        let selected = self.selected.borrow().clone();
        let renaming = self.renaming.borrow().clone();
        for (index, (layer, key)) in presented.layers.iter().zip(keys).enumerate() {
            if let Some((shown, _, more)) = rows.get(index)
                && *shown == key
            {
                let open = self
                    .layer_menu
                    .borrow()
                    .as_ref()
                    .is_some_and(|menu| menu.id == layer.id);
                set_class(more, "active", open);
                continue;
            }
            let row = gtk::ListBoxRow::new();
            row.set_widget_name(&layer.id);
            row.add_css_class("editor-layer-row");
            if layer.locked {
                row.add_css_class("locked");
            }
            let content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            // `.screenshot-layer-quick-actions`: 25 px buttons 2 px apart.
            let actions = gtk::Box::new(gtk::Orientation::Horizontal, 2);
            actions.set_valign(gtk::Align::Center);
            // `.screenshot-layer-grip`: a reorder cue, dimmed when locked.
            let grip = Icon::new("grip", 16).with_stroke(1.6);
            grip.add_css_class("editor-layer-grip");
            grip.set_size_request(12, -1);
            grip.set_tooltip_text(Some(if layer.locked {
                "Layer is locked"
            } else {
                "Drag to reorder"
            }));
            content.append(&grip);
            let preview = gtk::Box::new(gtk::Orientation::Vertical, 0);
            preview.add_css_class("editor-layer-preview");
            // `.screenshot-layer-preview`: a fixed 44 × 32 well.
            preview.set_size_request(42, 30);
            preview.set_hexpand(false);
            preview.set_valign(gtk::Align::Center);
            preview.set_overflow(gtk::Overflow::Hidden);
            match self.thumbnail_texture(&layer.id, layer.thumbnail.as_ref()) {
                Some(texture) => preview.append(&thumbnail_area(texture, layer.kind == "Image")),
                None => {
                    let icon = Icon::new(layer.icon, 16).with_stroke(1.7);
                    icon.set_vexpand(true);
                    preview.append(&icon);
                }
            }
            content.append(&preview);
            let copy = gtk::Box::new(gtk::Orientation::Vertical, 2);
            copy.set_hexpand(true);
            copy.set_valign(gtk::Align::Center);
            let image = layer.kind == "Image";
            if image && renaming.as_deref() == Some(layer.id.as_str()) {
                copy.append(&self.rename_field(&layer.id, &layer.name));
            } else {
                let name = label(&layer.name, "editor-layer-name");
                name.set_ellipsize(gtk::pango::EllipsizeMode::End);
                if image {
                    name.set_tooltip_text(Some("Double-click to rename"));
                    let click = gtk::GestureClick::new();
                    let weak = Rc::downgrade(self);
                    let id = layer.id.clone();
                    click.connect_pressed(move |_, presses, _, _| {
                        if presses == 2
                            && let Some(state) = weak.upgrade()
                        {
                            state.begin_rename(&id);
                        }
                    });
                    row.add_controller(click);
                }
                copy.append(&name);
            }
            let kind = label(layer.kind, "editor-layer-kind");
            kind.set_ellipsize(gtk::pango::EllipsizeMode::End);
            copy.append(&kind);
            content.append(&copy);
            let eye = icon_only(
                if layer.visible { "eye" } else { "eye-off" },
                15,
                if layer.visible {
                    "Hide layer"
                } else {
                    "Show layer"
                },
                "editor-layer-action",
            );
            let weak = Rc::downgrade(self);
            let (id, visible) = (layer.id.clone(), layer.visible);
            eye.connect_clicked(move |_| {
                if let Some(state) = weak.upgrade() {
                    state.submit(Request::Layer {
                        id: id.clone(),
                        edit: LayerEdit::Visibility { visible: !visible },
                    });
                }
            });
            actions.append(&eye);
            // Shipping's lock toggle: `active` (accent) while locked.
            let lock = icon_only(
                if layer.locked { "lock" } else { "unlock" },
                15,
                &format!(
                    "{} {}",
                    if layer.locked { "Unlock" } else { "Lock" },
                    layer.name
                ),
                "editor-layer-action",
            );
            lock.set_tooltip_text(Some(if layer.locked {
                "Unlock layer"
            } else {
                "Lock layer"
            }));
            set_class(&lock, "active", layer.locked);
            let weak = Rc::downgrade(self);
            let (id, locked) = (layer.id.clone(), layer.locked);
            lock.connect_clicked(move |_| {
                if let Some(state) = weak.upgrade() {
                    *state.selected.borrow_mut() = Some(id.clone());
                    state.submit(Request::Layer {
                        id: id.clone(),
                        edit: LayerEdit::Lock { locked: !locked },
                    });
                }
            });
            actions.append(&lock);
            let more = icon_only(
                "more",
                15,
                &captures_app::editor_layers::menu::settings_label(&layer.name),
                "editor-layer-action",
            );
            more.set_tooltip_text(Some("Layer settings and actions"));
            let open = self
                .layer_menu
                .borrow()
                .as_ref()
                .is_some_and(|menu| menu.id == layer.id);
            set_class(&more, "active", open);
            let weak = Rc::downgrade(self);
            let id = layer.id.clone();
            more.connect_clicked(move |button| {
                if let Some(state) = weak.upgrade() {
                    state.toggle_layer_menu(&id, button);
                }
            });
            actions.append(&more);
            content.append(&actions);
            row.set_child(Some(&content));
            row.update_property(&[gtk::accessible::Property::Label(&format!(
                "{} · {}",
                layer.name, layer.kind
            ))]);
            match rows.get_mut(index) {
                Some(slot) => {
                    list.remove(&slot.1);
                    list.insert(&row, index as i32);
                    *slot = (key, row, more);
                }
                None => {
                    list.append(&row);
                    rows.push((key, row, more));
                }
            }
        }
        let row = rows
            .iter()
            .find(|(key, _, _)| selected.as_deref() == Some(key.0.as_str()));
        match row {
            Some((_, row, _)) if !row.is_selected() => list.select_row(Some(row)),
            Some(_) => {}
            None => list.unselect_all(),
        }
    }

    /// The uploaded texture for a layer's thumbnail, reused while the session
    /// keeps the same image; textures for removed layers are dropped.
    fn thumbnail_texture(&self, id: &str, image: Option<&Arc<RgbaImage>>) -> Option<gdk::Texture> {
        let mut cache = self.thumbnails.borrow_mut();
        let Some(image) = image else {
            cache.remove(id);
            return None;
        };
        let key = Arc::as_ptr(image) as usize;
        if let Some((cached, texture)) = cache.get(id)
            && *cached == key
        {
            return Some(texture.clone());
        }
        let texture = texture_of(image);
        cache.insert(id.to_owned(), (key, texture.clone()));
        Some(texture)
    }

    /// Open (or close) the ⋯ settings card for `id` beside its row.
    fn toggle_layer_menu(self: &Rc<Self>, id: &str, anchor: &gtk::Button) {
        let open = self.layer_menu.borrow_mut().take();
        if let Some(menu) = open {
            menu.popover.popdown();
            if menu.id == id {
                return;
            }
        }
        let element = {
            let presented = self.presented.borrow();
            let Some(element) = presented.as_ref().and_then(|p| {
                p.document
                    .elements
                    .iter()
                    .find(|element| element.base().id == id)
                    .cloned()
            }) else {
                return;
            };
            element
        };
        if self.text_active() {
            self.finish_text();
        }
        *self.selected.borrow_mut() = Some(id.to_owned());
        self.select_tool("v");
        let name = self
            .presented
            .borrow()
            .as_ref()
            .and_then(|p| p.layers.iter().find(|l| l.id == id).map(|l| l.name.clone()))
            .unwrap_or_default();
        let weak = Rc::downgrade(self);
        let menu = layer_menu::LayerMenu::new(&element, &name, move |action| {
            if let Some(state) = weak.upgrade() {
                state.layer_menu_action(action);
            }
        });
        // The rows are rebuilt after every edit, so the card hangs off the
        // Layers panel and points at the ⋯ button's place.
        let host = self
            .widgets
            .layers
            .ancestor(gtk::ScrolledWindow::static_type())
            .and_then(|scroller| scroller.parent())
            .expect("layers panel");
        menu.popover.set_parent(&host);
        if let Some(bounds) = anchor.compute_bounds(&host) {
            menu.popover.set_pointing_to(Some(&gdk::Rectangle::new(
                bounds.x() as i32,
                bounds.y() as i32,
                bounds.width() as i32,
                bounds.height() as i32,
            )));
        }
        let weak = Rc::downgrade(self);
        let id = id.to_owned();
        menu.popover.connect_closed(move |popover| {
            let popover = popover.clone();
            glib::idle_add_local_once(move || popover.unparent());
            let Some(state) = weak.upgrade() else { return };
            let current = state
                .layer_menu
                .borrow()
                .as_ref()
                .is_some_and(|menu| menu.id == id);
            if current {
                state.layer_menu.borrow_mut().take();
                state.rebuild_layers();
            }
        });
        *self.layer_menu.borrow_mut() = Some(menu.clone());
        self.sync_layer_menu();
        self.rebuild_layers();
        self.canvas.queue_draw();
        menu.popover.popup();
    }

    fn sync_layer_menu(&self) {
        let Some(menu) = self.layer_menu.borrow().clone() else {
            return;
        };
        let presented = self.presented.borrow();
        let Some(presented) = presented.as_ref() else {
            return;
        };
        let elements = &presented.document.elements;
        match elements.iter().position(|e| e.base().id == menu.id) {
            Some(index) => menu.sync(
                &elements[index],
                index,
                elements.len(),
                &layer_menu::Caps {
                    merge_down_ids: &presented.merge_down_ids,
                    can_merge_visible: presented.can_merge_visible,
                    can_flatten: presented.can_flatten,
                },
            ),
            None => menu.popover.popdown(),
        }
    }

    fn layer_menu_action(self: &Rc<Self>, action: layer_menu::Action) {
        let close = match action {
            layer_menu::Action::Layer { edit, live, close } => {
                self.property_edit(inspector::Edit::Layer { edit, live });
                close
            }
            layer_menu::Action::Document { request, select } => {
                *self.pending_select.borrow_mut() = select;
                self.submit(request);
                true
            }
        };
        // Clone first: popdown emits `closed`, which clears the menu.
        let menu = self.layer_menu.borrow().clone();
        if close && let Some(menu) = menu {
            menu.popover.popdown();
        }
    }

    /// Shipping `beginLayerRename`: image layers only, selected, Select tool.
    fn begin_rename(self: &Rc<Self>, id: &str) {
        *self.selected.borrow_mut() = Some(id.to_owned());
        self.select_tool("v");
        *self.renaming.borrow_mut() = Some(id.to_owned());
        self.rebuild_layers();
    }

    /// The row's name entry: Enter or leaving commits a nonempty trimmed
    /// name, Escape cancels.
    fn rename_field(self: &Rc<Self>, id: &str, name: &str) -> gtk::Entry {
        let entry = gtk::Entry::new();
        entry.add_css_class("editor-layer-rename");
        entry.set_text(name);
        entry.update_property(&[gtk::accessible::Property::Label("Rename layer")]);
        let finish = {
            let weak = Rc::downgrade(self);
            let id = id.to_owned();
            let old = name.to_owned();
            move |entry: &gtk::Entry, commit: bool| {
                let Some(state) = weak.upgrade() else { return };
                if state.renaming.borrow().as_deref() != Some(id.as_str()) {
                    return;
                }
                state.renaming.borrow_mut().take();
                let name = entry.text().trim().to_owned();
                if commit && !name.is_empty() && name != old {
                    state.submit(Request::Layer {
                        id: id.clone(),
                        edit: LayerEdit::Rename { name },
                    });
                }
                let state = state.clone();
                glib::idle_add_local_once(move || {
                    state.rebuild_layers();
                    GtkWindowExt::set_focus(
                        &state
                            .canvas
                            .root()
                            .and_downcast::<gtk::Window>()
                            .expect("window"),
                        Some(&state.canvas),
                    );
                });
            }
        };
        let finish = Rc::new(finish);
        {
            let finish = finish.clone();
            entry.connect_activate(move |entry| finish(entry, true));
        }
        let keys = gtk::EventControllerKey::new();
        {
            let finish = finish.clone();
            let entry = entry.clone();
            keys.connect_key_pressed(move |_, key, _, _| {
                if key == gdk::Key::Escape {
                    finish(&entry, false);
                    glib::Propagation::Stop
                } else {
                    glib::Propagation::Proceed
                }
            });
        }
        entry.add_controller(keys);
        let focus = gtk::EventControllerFocus::new();
        {
            let entry = entry.clone();
            focus.connect_leave(move |_| finish(&entry, true));
        }
        entry.add_controller(focus);
        glib::idle_add_local_once({
            let entry = entry.clone();
            move || {
                entry.grab_focus();
                entry.select_region(0, -1);
            }
        });
        entry
    }

    /// Shipping drags a layer row onto another: the top or bottom half of
    /// the target places it before (above) or after (below) that layer.
    /// Locked layers do not move; the session keeps locked neighbours fixed.
    fn connect_layer_drag(self: &Rc<Self>) {
        let list = self.widgets.layers.clone();
        let drag = gtk::GestureDrag::new();
        let weak = Rc::downgrade(self);
        drag.connect_drag_begin(move |_, _, y| {
            let Some(state) = weak.upgrade() else { return };
            let start = state.widgets.layers.row_at_y(y as i32).filter(|row| {
                !row.has_css_class("locked")
                    && state.renaming.borrow().as_deref() != Some(row.widget_name().as_str())
            });
            *state.layer_drag.borrow_mut() = start.map(|row| (row.widget_name().to_string(), None));
        });
        let weak = Rc::downgrade(self);
        drag.connect_drag_update(move |gesture, _, dy| {
            let Some(state) = weak.upgrade() else { return };
            let Some((_, y)) = gesture.start_point() else {
                return;
            };
            let mut current = state.layer_drag.borrow_mut();
            let Some((moved, target)) = current.as_mut() else {
                return;
            };
            if target.is_none() && dy.abs() < 6. {
                return;
            }
            gesture.set_state(gtk::EventSequenceState::Claimed);
            let list = &state.widgets.layers;
            let y = y + dy;
            let over = list
                .row_at_y(y as i32)
                .filter(|row| row.widget_name().as_str() != moved.as_str());
            *target = Some(match over {
                Some(row) => {
                    let top = row
                        .compute_point(list, &graphene::Point::new(0., 0.))
                        .map_or(0., |point| f64::from(point.y()));
                    let placement = if y < top + f64::from(row.height()) / 2. {
                        LayerPlacement::Before
                    } else {
                        LayerPlacement::After
                    };
                    (row.widget_name().to_string(), placement)
                }
                None => (String::new(), LayerPlacement::Before),
            });
            let mut child = list.first_child();
            while let Some(row) = child {
                let name = row.widget_name();
                set_class(&row, "dragging", name.as_str() == moved.as_str());
                let (id, placement) = target.as_ref().expect("target");
                let here = name.as_str() == id.as_str();
                set_class(
                    &row,
                    "drop-before",
                    here && matches!(placement, LayerPlacement::Before),
                );
                set_class(
                    &row,
                    "drop-after",
                    here && matches!(placement, LayerPlacement::After),
                );
                child = row.next_sibling();
            }
        });
        let weak = Rc::downgrade(self);
        drag.connect_drag_end(move |_, _, _| {
            let Some(state) = weak.upgrade() else { return };
            let finished = state.layer_drag.borrow_mut().take();
            let mut child = state.widgets.layers.first_child();
            while let Some(row) = child {
                for class in ["dragging", "drop-before", "drop-after"] {
                    row.remove_css_class(class);
                }
                child = row.next_sibling();
            }
            if let Some((moved, Some((target_id, placement)))) = finished
                && !target_id.is_empty()
            {
                *state.selected.borrow_mut() = Some(moved.clone());
                state.select_tool("v");
                state.submit(Request::Layer {
                    id: moved,
                    edit: LayerEdit::Reorder {
                        target_id,
                        placement,
                    },
                });
            }
        });
        list.add_controller(drag);
    }

    fn update_export(&self) {
        let w = &self.widgets;
        let presented = self.presented.borrow();
        let target = self.target.borrow();
        let (Some(presented), Some(target)) = (presented.as_ref(), target.as_ref()) else {
            w.save.set_sensitive(false);
            w.copy.set_sensitive(false);
            if let Some((text, _)) = self.notice.borrow().as_ref() {
                w.notice.set_label(text);
                w.notice.add_css_class("error");
            }
            return;
        };
        let view = editor_export::present(
            target,
            self.options.get(),
            (
                presented.document.width.round() as u32,
                presented.document.height.round() as u32,
            ),
            presented.document.background.is_none(),
            self.estimate.get(),
        );
        w.export.sync(
            self.options.get(),
            &view,
            (
                presented.document.width.round() as u32,
                presented.document.height.round() as u32,
            ),
            presented.document.background.is_none(),
            self.estimate.get().pending,
        );
        w.export.set_sensitive(!self.saving.get());
        w.change_folder.set_sensitive(!self.saving.get());
        self.schedule_estimate(&presented.pixels);
        let compresses = self.options.get().quality != ExportQuality::Preserve;
        w.export
            .set_show_compare(compresses && self.compare.dismissed.get());
        self.refresh_compare();
        w.folder
            .set_label(&format!("Saving to {}", home_relative(&target.directory)));
        w.folder
            .set_tooltip_text(Some(&target.directory.display().to_string()));
        // Routine save copy rides on Save's tooltip; the footer line only
        // carries a warning.
        w.save.set_tooltip_text(Some(&view.hint));
        w.hint.set_label(&view.status_hint);
        let notice = self.notice.borrow();
        let (text, error) = match (&view.error, notice.as_ref()) {
            (Some(error), _) => (error.clone(), true),
            (None, Some((text, error))) => (text.clone(), *error),
            (None, None) => (String::new(), false),
        };
        w.notice.set_label(&text);
        set_class(&w.notice, "error", error);
        set_class(&w.notice, "success", !error && !text.is_empty());
        show_status(&w.notice, &w.hint);
        w.save
            .set_sensitive(view.plan.is_some() && view.error.is_none() && !self.saving.get());
        w.copy.set_sensitive(self.texture.borrow().is_some());
        if w.save_as_new.is_on() != view.saving_copy {
            w.save_as_new.activate_toggle_quietly(view.saving_copy);
        }
        // Shipping drops the toggle (and its rule) when the format forces a
        // new file instead of showing it disabled.
        if let Some(row) = w.save_as_new.parent() {
            row.set_visible(!view.format_requires_copy);
        }
    }

    fn set_options(&self, options: ExportOptions) {
        let previous = self.options.replace(options);
        if previous.quality != options.quality {
            // Shipping brings a hidden comparison back on a new quality mode.
            self.compare.dismissed.set(false);
        }
        *self.notice.borrow_mut() = None;
        self.update_export();
    }

    /// Re-encode the export off the UI thread once edits or options settle,
    /// exactly as Save would; a superseded result is dropped.
    fn schedule_estimate(&self, pixels: &Arc<RgbaImage>) {
        let options = self.options.get();
        let comparing = self.compare_visible();
        let key = (Arc::as_ptr(pixels) as usize, options, comparing);
        if self.estimate_key.get() == Some(key) {
            return;
        }
        self.estimate_key.set(Some(key));
        let generation = self.estimate_generation.get() + 1;
        self.estimate_generation.set(generation);
        if let Some(timer) = self.estimate_timer.borrow_mut().take() {
            timer.remove();
        }
        let mut estimate = self.estimate.get();
        estimate.pending = true;
        self.estimate.set(estimate);
        let (pixels, original, me) = (pixels.clone(), self.original_bytes, self.me.clone());
        let timer =
            glib::timeout_add_local_once(std::time::Duration::from_millis(250), move || {
                if let Some(state) = me.upgrade() {
                    state.estimate_timer.borrow_mut().take();
                }
                glib::spawn_future_local(async move {
                    let result = gtk::gio::spawn_blocking(move || {
                        if !comparing {
                            return editor_export::estimate_export(&pixels, options, original)
                                .map(|estimate| (estimate, None));
                        }
                        // One encode serves the comparison's After side and the
                        // estimate; compression modes compare with lossless PNG.
                        let (after, bytes) =
                            captures_app::compression_compare::encode_after(&pixels, options)?;
                        let baseline = captures_history::encode_png(&pixels)
                            .map_err(|error| error.to_string())?
                            .len() as u64;
                        Ok((
                            editor_export::ExportEstimate {
                                bytes,
                                baseline_bytes: Some(baseline),
                            },
                            Some(after),
                        ))
                    })
                    .await;
                    let Some(state) = me.upgrade() else { return };
                    if state.estimate_generation.get() != generation {
                        return;
                    }
                    state.estimate.set(match &result {
                        Ok(Ok((estimate, _))) => EstimateState {
                            bytes: Some(estimate.bytes),
                            baseline_bytes: estimate.baseline_bytes,
                            pending: false,
                        },
                        _ => EstimateState::default(),
                    });
                    if let Ok(Ok((_, Some(after)))) = &result {
                        *state.compare.texture.borrow_mut() = Some(texture_of(after));
                        state.canvas.queue_draw();
                    }
                    state.update_export();
                });
            });
        *self.estimate_timer.borrow_mut() = Some(timer);
    }

    /// Shipping `showCompressCompare`.
    fn compare_visible(&self) -> bool {
        captures_app::compression_compare::screenshot_visible(
            self.options.get().quality != ExportQuality::Preserve,
            self.widgets.export.is_open(),
            self.compare.dismissed.get(),
        )
    }

    fn compare_suppressed_now(&self) -> bool {
        self.gesture.borrow().is_some() || self.text_active()
    }

    /// Show the comparison's widgets and badges for the current state.
    fn refresh_compare(&self) {
        let visible = self.compare_visible();
        if !visible {
            self.compare.texture.borrow_mut().take();
        }
        let estimate = self.estimate.get();
        let suppressed = self.compare_suppressed_now();
        self.compare_suppressed.set(suppressed);
        let drawing = !matches!(self.tool.borrow().as_str(), "v" | "h");
        self.compare.sync(
            visible && !suppressed,
            estimate.baseline_bytes,
            estimate.bytes,
            estimate.pending,
            drawing,
        );
        self.text_overlay.queue_allocate();
        self.canvas.queue_draw();
    }

    /// Shipping's Change…: pick the folder Save writes to.
    fn choose_folder(self: &Rc<Self>) {
        let window = self.canvas.root().and_downcast::<gtk::Window>();
        #[allow(deprecated)] // `GtkFileDialog` needs GTK 4.10.
        let chooser = gtk::FileChooserNative::new(
            Some("Choose save location"),
            window.as_ref(),
            gtk::FileChooserAction::SelectFolder,
            Some("Select"),
            Some("Cancel"),
        );
        if let Some(target) = self.target.borrow().as_ref() {
            #[allow(deprecated)]
            let _ = chooser.set_current_folder(Some(&gtk::gio::File::for_path(&target.directory)));
        }
        let weak = Rc::downgrade(self);
        #[allow(deprecated)]
        chooser.connect_response(move |chooser, response| {
            let folder = chooser.file().and_then(|file| file.path());
            chooser.destroy();
            let Some(state) = weak.upgrade() else { return };
            state.chooser.borrow_mut().take();
            if response != gtk::ResponseType::Accept {
                return;
            }
            if let Some(folder) = folder {
                if let Some(target) = state.target.borrow_mut().as_mut() {
                    target.set_directory(folder);
                }
                *state.notice.borrow_mut() = None;
                state.sync_filename();
                state.update_export();
            }
        });
        #[allow(deprecated)]
        chooser.show();
        *self.chooser.borrow_mut() = Some(chooser);
    }

    fn copy(&self) {
        if let Some(texture) = self.texture.borrow().as_ref() {
            self.canvas.clipboard().set_texture(texture);
            *self.notice.borrow_mut() = Some(("Copied image".into(), false));
            self.update_export();
        }
    }

    fn save(&self) {
        let plan = self
            .target
            .borrow()
            .as_ref()
            .and_then(|target| target.plan(self.options.get().format).ok());
        let Some(plan) = plan else { return };
        if self.saving.replace(true) {
            return;
        }
        self.busy.set(true);
        *self.notice.borrow_mut() = Some(("Saving…".into(), false));
        self.update_export();
        let _ = self.jobs.send(Job::Save {
            plan,
            options: self.options.get(),
            mode: self.mode,
        });
    }

    /// Whether the focused widget moves through options or values with the
    /// arrow keys itself: a dropdown or its list, a slider, a number field.
    fn focus_uses_arrows(&self) -> bool {
        let mut widget = self.canvas.root().and_then(|root| root.focus());
        while let Some(current) = widget {
            if current.is::<gtk::Popover>()
                || current.is::<gtk::Scale>()
                || current.is::<gtk::SpinButton>()
                || current.is::<gtk::ListBox>()
                || current.is::<gtk::ListView>()
                || current.accessible_role() == gtk::AccessibleRole::ComboBox
            {
                return true;
            }
            widget = current.parent();
        }
        false
    }

    /// Shipping `nudgeSelected`: move the selected, unlocked layer. A run of
    /// nudges on one layer is one Undo step, like shipping's coalescing.
    fn nudge_selected(&self, dx: f64, dy: f64) -> bool {
        let Some(id) = self.selected.borrow().clone() else {
            return false;
        };
        let locked = self
            .presented
            .borrow()
            .as_ref()
            .is_some_and(|p| p.layers.iter().any(|l| l.id == id && l.locked));
        if !locked {
            self.submit(Request::Live {
                key: format!("nudge:{id}"),
                request: Box::new(Request::Layer {
                    id,
                    edit: LayerEdit::Translate {
                        delta_x: dx,
                        delta_y: dy,
                    },
                }),
            });
        }
        true
    }

    /// Whether a focused button outside the tool rail would take Enter.
    fn focus_on_button(&self) -> bool {
        let focus = self.canvas.root().and_then(|root| root.focus());
        let button = focus.and_then(|focus| {
            if focus.is::<gtk::Button>() {
                Some(focus)
            } else {
                focus.ancestor(gtk::Button::static_type())
            }
        });
        button.is_some_and(|button| {
            !button
                .parent()
                .is_some_and(|p| p.has_css_class("editor-rail"))
        })
    }

    /// Shipping Ctrl+D: duplicate the selected layer above it and select it.
    fn duplicate_selected(&self) -> bool {
        let Some(id) = self.selected.borrow().clone() else {
            return false;
        };
        if self.text_active() {
            self.finish_text();
        }
        let new_id = uuid::Uuid::new_v4().to_string();
        *self.pending_select.borrow_mut() = Some(new_id.clone());
        self.submit(Request::Layer {
            id,
            edit: LayerEdit::Duplicate { new_id },
        });
        self.select_tool("v");
        true
    }

    /// Shipping copies the selected layer within this editor and claims the
    /// clipboard with a marker; another app's copy replaces the marker, so a
    /// stale layer never hides a copied image.
    fn copy_layer(&self) -> bool {
        let Some(id) = self.selected.borrow().clone() else {
            return false;
        };
        let marker = uuid::Uuid::new_v4().to_string();
        let provider = gdk::ContentProvider::for_bytes(
            LAYER_CLIPBOARD_TYPE,
            &glib::Bytes::from_owned(marker.clone().into_bytes()),
        );
        if self
            .canvas
            .clipboard()
            .set_content(Some(&provider))
            .is_err()
        {
            return false;
        }
        *self.layer_marker.borrow_mut() = Some(marker);
        self.submit(Request::CopyLayer { id });
        true
    }

    /// Shipping paste: this editor's copied layer while the clipboard still
    /// holds its marker, otherwise copied image files or an image.
    fn paste(self: &Rc<Self>) {
        let clipboard = self.canvas.clipboard();
        let formats = clipboard.formats();
        let weak = Rc::downgrade(self);
        if formats.contain_mime_type(LAYER_CLIPBOARD_TYPE) && self.layer_marker.borrow().is_some() {
            clipboard.read_async(
                &[LAYER_CLIPBOARD_TYPE],
                glib::Priority::DEFAULT,
                gtk::gio::Cancellable::NONE,
                move |result| {
                    let Some(state) = weak.upgrade() else { return };
                    let Ok((stream, _)) = result else { return };
                    let bytes = stream
                        .read_bytes(256, gtk::gio::Cancellable::NONE)
                        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
                    if bytes.ok() == *state.layer_marker.borrow() {
                        let new_id = uuid::Uuid::new_v4().to_string();
                        let after_id = state.selected.borrow().clone();
                        *state.pending_select.borrow_mut() = Some(new_id.clone());
                        state.submit(Request::PasteLayer { new_id, after_id });
                        state.select_tool("v");
                    }
                },
            );
            return;
        }
        if formats.contains_type(gdk::FileList::static_type()) {
            clipboard.read_value_async(
                gdk::FileList::static_type(),
                glib::Priority::DEFAULT,
                gtk::gio::Cancellable::NONE,
                move |result| {
                    let (Some(state), Ok(value)) = (weak.upgrade(), result) else {
                        return;
                    };
                    let Ok(files) = value.get::<gdk::FileList>() else {
                        return;
                    };
                    let paths: Vec<_> = files
                        .files()
                        .iter()
                        .filter_map(|file| file.path())
                        .filter(|path| captures_app::editor_session::is_supported_image_path(path))
                        .collect();
                    state.import_files(paths, None);
                },
            );
            return;
        }
        if formats.contains_type(gdk::Texture::static_type()) {
            clipboard.read_texture_async(gtk::gio::Cancellable::NONE, move |result| {
                let (Some(state), Ok(Some(texture))) = (weak.upgrade(), result) else {
                    return;
                };
                state.import_png(texture.save_to_png_bytes().to_vec());
            });
        }
    }

    fn delete_selected(&self) {
        let id = self.selected.borrow().clone();
        let locked = self.presented.borrow().as_ref().is_some_and(|p| {
            p.layers
                .iter()
                .any(|l| Some(&l.id) == id.as_ref() && l.locked)
        });
        if let Some(id) = id
            && !locked
        {
            self.submit(Request::Layer {
                id,
                edit: LayerEdit::Delete,
            });
        }
    }

    fn connect_canvas(self: &Rc<Self>, window: &gtk::ApplicationWindow) {
        let weak = Rc::downgrade(self);
        self.canvas.set_draw(move |snapshot, width, height| {
            let Some(state) = weak.upgrade() else { return };
            state.draw(snapshot, width, height);
            // Fit's percent follows the window; refresh the readouts after
            // this frame rather than restyling the header mid-snapshot.
            if let Some(shown) = state.zoom_percent()
                && (shown - state.zoom_synced.get()).abs() > 0.05
            {
                state.zoom_synced.set(shown);
                let weak = Rc::downgrade(&state);
                glib::idle_add_local_once(move || {
                    if let Some(state) = weak.upgrade() {
                        state.sync_zoom();
                    }
                });
            }
        });

        // Primary-button gestures: tools.
        let drag = gtk::GestureDrag::new();
        drag.set_button(gdk::BUTTON_PRIMARY);
        let weak = Rc::downgrade(self);
        let shift = |gesture: &gtk::GestureDrag| {
            gesture
                .current_event_state()
                .contains(gdk::ModifierType::SHIFT_MASK)
        };
        drag.connect_drag_begin(move |gesture, x, y| {
            if let Some(state) = weak.upgrade() {
                state.canvas.grab_focus();
                // Shipping: Ctrl-drag pans from anywhere, whatever the tool.
                let control = gesture
                    .current_event_state()
                    .contains(gdk::ModifierType::CONTROL_MASK);
                if control || state.pan_ready.get() {
                    state.begin_pan();
                } else {
                    state.begin(x, y, shift(gesture));
                }
            }
        });
        let weak = Rc::downgrade(self);
        drag.connect_drag_update(move |gesture, dx, dy| {
            let Some(state) = weak.upgrade() else { return };
            if let Some((x, y)) = gesture.start_point() {
                state.update(x + dx, y + dy, dx, dy, shift(gesture));
            }
        });
        let weak = Rc::downgrade(self);
        drag.connect_drag_end(move |_, _, _| {
            if let Some(state) = weak.upgrade() {
                state.finish();
                state.sync_cursor();
            }
        });
        self.canvas.add_controller(drag);

        // The inline text field: typing, Escape and leaving it commit.
        let weak = Rc::downgrade(self);
        self.text_field.view.buffer().connect_changed(move |_| {
            if let Some(state) = weak.upgrade() {
                state.text_changed();
            }
        });
        let keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape
                && let Some(state) = weak.upgrade()
            {
                state.finish_text();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        self.text_field.view.add_controller(keys);
        let focus = gtk::EventControllerFocus::new();
        let weak = Rc::downgrade(self);
        focus.connect_leave(move |_| {
            if let Some(state) = weak.upgrade() {
                state.finish_text();
            }
        });
        self.text_field.view.add_controller(focus);
        let weak = Rc::downgrade(self);
        self.text_overlay
            .connect_get_child_position(move |_, child| {
                let state = weak.upgrade()?;
                if child == state.expand_action.upcast_ref::<gtk::Widget>() {
                    return state.expand_rect();
                }
                if child == state.quick_bar.root.upcast_ref::<gtk::Widget>() {
                    return state.quick_bar_rect();
                }
                if !state.compare.owns(child) {
                    return state.text_field_rect();
                }
                let rect = state.image_rect()?;
                // Only the part of the image inside the canvas carries the controls.
                let (cw, ch) = (state.canvas.width() as f32, state.canvas.height() as f32);
                let x = (rect.x as f32).max(0.);
                let y = (rect.y as f32).max(0.);
                let visible = graphene::Rect::new(
                    x,
                    y,
                    ((rect.x + rect.width) as f32).min(cw) - x,
                    ((rect.y + rect.height) as f32).min(ch) - y,
                );
                state.compare.position(child, &visible)
            });
        // The comparison handle drags the divider with any tool. The drag
        // lives on the overlay, so its coordinates do not move with the handle.
        let split_drag = gtk::GestureDrag::new();
        split_drag.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(self);
        split_drag.connect_drag_begin(move |gesture, x, y| {
            let Some(state) = weak.upgrade() else { return };
            let handle = &state.compare.handle;
            let inside = handle.is_visible()
                && handle.is_sensitive()
                && handle
                    .compute_bounds(&state.text_overlay)
                    .is_some_and(|b| b.contains_point(&graphene::Point::new(x as f32, y as f32)));
            gesture.set_state(if inside {
                gtk::EventSequenceState::Claimed
            } else {
                gtk::EventSequenceState::Denied
            });
        });
        let weak = Rc::downgrade(self);
        split_drag.connect_drag_update(move |gesture, dx, _| {
            let Some(state) = weak.upgrade() else { return };
            let (Some((x, _)), Some(rect)) = (gesture.start_point(), state.image_rect()) else {
                return;
            };
            state
                .compare
                .set_split(captures_app::compression_compare::split_at(
                    x + dx - rect.x,
                    rect.width,
                ));
            state.canvas.queue_draw();
            state.text_overlay.queue_allocate();
        });
        self.text_overlay.add_controller(split_drag);
        let weak = Rc::downgrade(self);
        self.compare.hide.connect_clicked(move |_| {
            if let Some(state) = weak.upgrade() {
                state.compare.dismissed.set(true);
                state.update_export();
            }
        });
        let weak = Rc::downgrade(self);
        self.widgets.export.show_compare.connect_clicked(move |_| {
            if let Some(state) = weak.upgrade() {
                state.compare.dismissed.set(false);
                state.update_export();
            }
        });
        let weak = Rc::downgrade(self);
        self.widgets.export.disclosure.connect_clicked(move |_| {
            if let Some(state) = weak.upgrade() {
                state.update_export();
            }
        });

        // Middle-button drag pans with any tool.
        let pan = gtk::GestureDrag::new();
        pan.set_button(gdk::BUTTON_MIDDLE);
        let weak = Rc::downgrade(self);
        pan.connect_drag_begin(move |_, _, _| {
            if let Some(state) = weak.upgrade() {
                state.begin_pan();
            }
        });
        let weak = Rc::downgrade(self);
        pan.connect_drag_update(move |_, dx, dy| {
            if let Some(state) = weak.upgrade() {
                state.update(0., 0., dx, dy, false);
            }
        });
        let weak = Rc::downgrade(self);
        pan.connect_drag_end(move |_, _, _| {
            if let Some(state) = weak.upgrade() {
                state.gesture.borrow_mut().take();
                state.sync_cursor();
            }
        });
        self.canvas.add_controller(pan);

        // Pointer position anchors wheel zoom.
        let motion = gtk::EventControllerMotion::new();
        let weak = Rc::downgrade(self);
        motion.connect_motion(move |controller, x, y| {
            let Some(state) = weak.upgrade() else { return };
            state.pointer.set(Some((x, y)));
            // Ctrl was released while the window lacked keyboard focus.
            if state.pan_ready.get()
                && !controller
                    .current_event_state()
                    .contains(gdk::ModifierType::CONTROL_MASK)
            {
                state.set_pan_ready(false);
            }
            state.hover_grips(x, y);
            // Only the brush ring follows the pointer; nothing else redraws.
            if state.brush_mode().is_some() {
                state.sync_cursor();
                state.canvas.queue_draw();
            }
        });
        let weak = Rc::downgrade(self);
        motion.connect_leave(move |_| {
            let Some(state) = weak.upgrade() else { return };
            state.pointer.set(None);
            if state.brush_mode().is_some() {
                state.canvas.queue_draw();
            }
        });
        self.canvas.add_controller(motion);
        let weak = Rc::downgrade(self);
        self.widgets.add_images.connect_clicked(move |_| {
            if let Some(state) = weak.upgrade() {
                state.choose_images();
            }
        });
        let weak = Rc::downgrade(self);
        self.widgets.add_layer.connect_clicked(move |_| {
            if let Some(state) = weak.upgrade() {
                state.choose_images();
            }
        });
        let weak = Rc::downgrade(self);
        self.expand_action.connect_clicked(move |_| {
            let Some(state) = weak.upgrade() else { return };
            let id = state.expand_for.borrow().clone();
            if let Some(id) = id {
                state.expand_armed.set(false);
                state.submit(Request::Layer {
                    id,
                    edit: LayerEdit::ExpandCanvas,
                });
            }
        });
        let hover = gtk::EventControllerMotion::new();
        let weak = Rc::downgrade(self);
        hover.connect_enter(move |_, _, _| {
            if let Some(state) = weak.upgrade() {
                state.expand_armed.set(true);
                state.canvas.queue_draw();
            }
        });
        let weak = Rc::downgrade(self);
        hover.connect_leave(move |_| {
            if let Some(state) = weak.upgrade() {
                state.expand_armed.set(false);
                state.canvas.queue_draw();
            }
        });
        self.expand_action.add_controller(hover);
        // Shipping previews Trim edges while its button is hovered or focused.
        let trim = &self.widgets.canvas_bar.trim;
        let set_trim_hover = {
            let weak = Rc::downgrade(self);
            move |on: bool| {
                if let Some(state) = weak.upgrade() {
                    state.trim_hover.set(on);
                    state.canvas.queue_draw();
                }
            }
        };
        let hover = gtk::EventControllerMotion::new();
        let set = set_trim_hover.clone();
        hover.connect_enter(move |_, _, _| set(true));
        let set = set_trim_hover.clone();
        hover.connect_leave(move |_| set(false));
        trim.add_controller(hover);
        let focus = gtk::EventControllerFocus::new();
        let set = set_trim_hover.clone();
        focus.connect_enter(move |_| set(true));
        focus.connect_leave(move |_| set_trim_hover(false));
        trim.add_controller(focus);
        // Shipping takes image drops anywhere in the editor: the guide shows
        // where they will land, and they import at the drop point.
        let drop = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
        let weak = Rc::downgrade(self);
        drop.connect_enter(move |target, x, y| {
            if let Some(state) = weak.upgrade() {
                state.drag_over(target, x, y);
            }
            gdk::DragAction::COPY
        });
        let weak = Rc::downgrade(self);
        drop.connect_motion(move |target, x, y| {
            if let Some(state) = weak.upgrade() {
                state.drag_over(target, x, y);
            }
            gdk::DragAction::COPY
        });
        let weak = Rc::downgrade(self);
        drop.connect_leave(move |_| {
            if let Some(state) = weak.upgrade() {
                state.end_drag_over();
            }
        });
        let weak = Rc::downgrade(self);
        drop.connect_drop(move |target, value, x, y| {
            let (Some(state), Ok(files)) = (weak.upgrade(), value.get::<gdk::FileList>()) else {
                return false;
            };
            let point = state.drop_point(target, x, y);
            state.end_drag_over();
            let paths: Vec<_> = files
                .files()
                .iter()
                .filter_map(|f| f.path())
                .filter(|path| captures_app::editor_session::is_supported_image_path(path))
                .collect();
            if paths.is_empty() {
                *state.notice.borrow_mut() =
                    Some((captures_app::editor_session::DROP_UNSUPPORTED.into(), true));
                state.update_export();
                return false;
            }
            state.import_files(paths, point);
            true
        });
        window.add_controller(drop);
        let weak = Rc::downgrade(self);
        self.properties.inspector.defaults.connect_pick(move |key| {
            if let Some(state) = weak.upgrade() {
                state.select_tool(key);
            }
        });
        let weak = Rc::downgrade(self);
        self.properties.inspector.sink.connect(move |edit| {
            if let Some(state) = weak.upgrade() {
                state.property_edit(edit);
            }
        });
        // The quick bar edits through the same path as the inspector.
        let weak = Rc::downgrade(self);
        self.quick_bar.connect(move |edit| {
            if let Some(state) = weak.upgrade() {
                state.property_edit(edit);
            }
        });
        let weak = Rc::downgrade(self);
        self.properties.crop.apply.connect_clicked(move |_| {
            if let Some(state) = weak.upgrade() {
                state.apply_crop();
            }
        });
        let weak = Rc::downgrade(self);
        self.properties.crop.clear.connect_clicked(move |_| {
            if let Some(state) = weak.upgrade() {
                state.set_crop(None);
            }
        });
        let weak = Rc::downgrade(self);
        self.properties.eraser.connect_mode(move || {
            if let Some(state) = weak.upgrade() {
                state.sync_cursor();
                state.canvas.queue_draw();
            }
        });
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES);
        let weak = Rc::downgrade(self);
        scroll.connect_scroll(move |controller, dx, dy| {
            let Some(state) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            let control = controller
                .current_event_state()
                .contains(gdk::ModifierType::CONTROL_MASK);
            if control {
                if let (Some(factor), Some(percent)) = (
                    editor_viewport::wheel_zoom_factor(dy * 50.),
                    state.zoom_percent(),
                ) {
                    state.zoom_to(percent * factor, state.pointer.get());
                }
            } else {
                let mut viewport = state.viewport.get();
                viewport.pan_x -= dx * 40.;
                viewport.pan_y -= dy * 40.;
                state.set_viewport(viewport);
            }
            glib::Propagation::Stop
        });
        self.canvas.add_controller(scroll);

        let keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, modifiers| {
            let Some(state) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            if matches!(key, gdk::Key::Control_L | gdk::Key::Control_R) {
                // Shipping `isPanModifierKey`: hold Ctrl to pan, unless a
                // control has focus.
                state.set_pan_ready(!state.control_focused());
                return glib::Propagation::Proceed;
            }
            if state.key(key, modifiers) {
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        let weak = Rc::downgrade(self);
        keys.connect_key_released(move |_, key, _, _| {
            if matches!(key, gdk::Key::Control_L | gdk::Key::Control_R)
                && let Some(state) = weak.upgrade()
            {
                state.set_pan_ready(false);
            }
        });
        window.add_controller(keys);
        let weak = Rc::downgrade(self);
        window.connect_is_active_notify(move |window| {
            if !window.is_active()
                && let Some(state) = weak.upgrade()
            {
                state.set_pan_ready(false);
            }
        });
    }

    fn key(self: &Rc<Self>, key: gdk::Key, modifiers: gdk::ModifierType) -> bool {
        let control = modifiers.contains(gdk::ModifierType::CONTROL_MASK);
        let shift = modifiers.contains(gdk::ModifierType::SHIFT_MASK);
        let typing = self
            .canvas
            .root()
            .and_then(|root| root.focus())
            .is_some_and(|focus| {
                focus.is::<gtk::Text>() || focus.is::<gtk::Entry>() || focus.is::<gtk::TextView>()
            });
        let name = key
            .to_lower()
            .name()
            .map(|n| n.to_string())
            .unwrap_or_default();
        let alt = modifiers.contains(gdk::ModifierType::ALT_MASK);
        // Shipping's order: zoom and Save work from a field too; the rest
        // leaves a text field its own keys.
        match (control, name.as_str()) {
            (true, "plus" | "equal" | "KP_Add") => self.zoom_by(1.25),
            (true, "minus" | "underscore" | "KP_Subtract") => self.zoom_by(1. / 1.25),
            (true, "0" | "KP_0") => self.zoom_to(100., None),
            (true, "s") if !alt && !shift => self.save(),
            _ if typing => return false,
            (true, "c") if shift && !alt => self.copy(),
            (true, "c") => return self.copy_layer(),
            (true, "v") => self.paste(),
            (true, "d") => return self.duplicate_selected(),
            (true, "z") if shift => self.submit(Request::Redo),
            (true, "z") => self.submit(Request::Undo),
            (true, "y") if !shift => self.submit(Request::Redo),
            _ if control => return false,
            // Enter applies a staged crop, as Escape drops it. A focused
            // button keeps Enter, except the tool rail Crop was picked from.
            (_, "Return" | "KP_Enter") => {
                if self.crop.get().is_none() || self.focus_on_button() {
                    return false;
                }
                self.apply_crop();
            }
            (_, "Delete" | "BackSpace") => self.delete_selected(),
            // Shipping nudges the selected layer 1 px (10 with Shift), except
            // when a focused control already uses the arrow (Tauri a18afc62).
            (
                _,
                "Left" | "Right" | "Up" | "Down" | "KP_Left" | "KP_Right" | "KP_Up" | "KP_Down",
            ) if !self.focus_uses_arrows() => {
                let step = if shift { 10. } else { 1. };
                let (dx, dy) = match name.trim_start_matches("KP_") {
                    "Left" => (-step, 0.),
                    "Right" => (step, 0.),
                    "Up" => (0., -step),
                    _ => (0., step),
                };
                return self.nudge_selected(dx, dy);
            }
            (_, "Escape") => {
                let gesture = self.gesture.borrow_mut().take();
                if matches!(gesture, Some(Gesture::Layer { .. })) {
                    self.preview_dirty.set(false);
                    self.restore_frame();
                }
                if self.crop.get().is_some() {
                    self.set_crop(None);
                } else if gesture.is_none() {
                    self.selected.borrow_mut().take();
                    // Clear the Layers highlight and show the tool's defaults.
                    self.rebuild_selection();
                }
                self.canvas.queue_draw();
            }
            (_, "v" | "c" | "a" | "p" | "t" | "b") => self.select_tool(&name),
            (_, letter) => {
                let Some(tool) = SHAPE_TOOLS.iter().find(|tool| {
                    tool.shortcut
                        .is_some_and(|key| key.eq_ignore_ascii_case(letter))
                }) else {
                    return false;
                };
                self.select_tool(tool.key);
            }
        }
        true
    }

    fn begin(&self, x: f64, y: f64, shift: bool) {
        let Some(point) = self.to_document(x, y) else {
            return;
        };
        let tool = self.tool.borrow().clone();
        if tool == "t" {
            self.begin_text(point);
            return;
        }
        if self.text_active() {
            self.finish_text();
        }
        // A second Select press on a text label within the double-click time
        // edits it in place (one gesture, so focus stays in the field).
        let now = std::time::Instant::now();
        let double = self
            .last_press
            .replace(Some((now, x, y)))
            .is_some_and(|(at, px, py)| {
                let limit = gtk::Settings::default().map_or(400, |s| s.gtk_double_click_time());
                now.duration_since(at).as_millis() <= limit as u128 && (x - px).hypot(y - py) <= 5.
            });
        if tool == "v" && double && self.edit_text_at(point) {
            self.last_press.set(None);
            return;
        }
        if tool == "b" {
            let eraser = &self.properties.eraser;
            match eraser.mode() {
                properties::EraserMode::Wand => {
                    *self.notice.borrow_mut() = None;
                    self.submit(Request::RemoveImageBackground {
                        point,
                        tolerance: eraser.tolerance.value(),
                        contiguous: eraser.contiguous.is_active(),
                    });
                }
                properties::EraserMode::Brush(_) => {
                    *self.gesture.borrow_mut() = Some(Gesture::Brush(vec![point]));
                    self.preview_brush();
                    self.canvas.queue_draw();
                }
            }
            return;
        }
        if self.transform_tool() {
            if double && self.edit_curve_at(point) {
                self.last_press.set(None);
                return;
            }
            if let Some((element, outline, hit)) = self.grip_at(point) {
                let drag = match hit {
                    canvas_chrome::Hit::Rotate => LayerDrag::Rotate {
                        outline,
                        initial: element.base().rotation(),
                    },
                    canvas_chrome::Hit::Resize(handle) => LayerDrag::Resize(handle),
                    canvas_chrome::Hit::Curve(handle) => LayerDrag::Curve(handle),
                };
                if !matches!(drag, LayerDrag::Resize(_)) {
                    self.canvas.set_cursor_from_name(Some("grabbing"));
                }
                self.canvas.set_tooltip_text(None);
                *self.gesture.borrow_mut() = Some(Gesture::Layer {
                    id: element.base().id.clone(),
                    drag,
                    start: point,
                    current: point,
                    shift,
                });
                self.canvas.queue_draw();
                return;
            }
        }
        let gesture = match tool.as_str() {
            "c" => {
                let Some((width, height)) = self.document_size() else {
                    return;
                };
                let bounds = Rect {
                    x: 0.,
                    y: 0.,
                    width,
                    height,
                };
                self.selected.borrow_mut().take();
                self.rebuild_selection();
                let drag = CropDrag::new(point, bounds, self.properties.crop.aspect(), shift);
                self.set_crop(Some(drag.rect()));
                Gesture::Crop(drag)
            }
            "v" => {
                let hit = self.presented.borrow().as_ref().and_then(|presented| {
                    let scale = self.zoom_percent().unwrap_or(100.) / 100.;
                    presented
                        .document
                        .hit_test(point, 4. / scale)
                        .ok()
                        .flatten()
                        .filter(|element| !element.base().locked)
                        .map(|element| element.base().id.clone())
                });
                *self.selected.borrow_mut() = hit.clone();
                self.rebuild_selection();
                match hit {
                    Some(id) => Gesture::Move {
                        id,
                        start: point,
                        end: point,
                    },
                    None => Gesture::Pan {
                        from: self.viewport.get(),
                        dx: 0.,
                        dy: 0.,
                    },
                }
            }
            "p" => Gesture::Freehand(vec![point]),
            tool => Gesture::Shape {
                tool: tool.to_owned(),
                start: point,
                end: point,
            },
        };
        *self.gesture.borrow_mut() = Some(gesture);
        self.canvas.queue_draw();
    }

    fn rebuild_selection(&self) {
        let selected = self.selected.borrow().clone();
        let list = &self.widgets.layers;
        let mut child = list.first_child();
        while let Some(row) = child {
            if let Some(row) = row.downcast_ref::<gtk::ListBoxRow>()
                && selected.as_deref() == Some(row.widget_name().as_str())
            {
                list.select_row(Some(row));
            }
            child = row.next_sibling();
        }
        if selected.is_none() {
            list.unselect_all();
        }
        self.refresh_properties();
    }

    fn update(&self, x: f64, y: f64, dx: f64, dy: f64, shift: bool) {
        let point = self.to_document(x, y);
        let mut gesture = self.gesture.borrow_mut();
        match gesture.as_mut() {
            Some(Gesture::Crop(drag)) => {
                let Some(point) = point else { return };
                let rect = drag.update(point, self.properties.crop.aspect(), shift);
                drop(gesture);
                self.set_crop(Some(rect));
                return;
            }
            Some(Gesture::Pan { from, .. }) => {
                let mut viewport = *from;
                viewport.pan_x += dx;
                viewport.pan_y += dy;
                drop(gesture);
                self.viewport.set(viewport);
                self.canvas.queue_draw();
                if self.text_active() || self.expand_action.is_visible() {
                    self.text_overlay.queue_allocate();
                }
                return;
            }
            Some(Gesture::Layer {
                current,
                shift: held,
                ..
            }) => {
                if let Some(point) = point {
                    *current = point;
                }
                *held = shift;
                drop(gesture);
                self.preview_brush();
                return;
            }
            Some(Gesture::Shape { end, .. } | Gesture::Move { end, .. }) => {
                if let Some(point) = point {
                    *end = point;
                }
            }
            Some(Gesture::Freehand(points)) => {
                if let Some(point) = point {
                    points.push(point);
                }
            }
            Some(Gesture::Brush(points)) => {
                // Off-canvas points still paint, as shipping's stroke does.
                if let Some(point) = self.to_document(x, y) {
                    points.push(point);
                }
                drop(gesture);
                self.pointer.set(Some((x, y)));
                self.preview_brush();
                self.canvas.queue_draw();
                return;
            }
            None => return,
        }
        drop(gesture);
        self.canvas.queue_draw();
    }

    fn finish(&self) {
        let Some(gesture) = self.gesture.borrow_mut().take() else {
            return;
        };
        let scale = self.zoom_percent().unwrap_or(100.) / 100.;
        let moved = |a: Point, b: Point| (b.x - a.x).hypot(b.y - a.y) * scale >= 3.;
        let defaults = &self.properties.inspector.defaults.values;
        let style = defaults.style.borrow().clone();
        let opacity = defaults.opacity.get();
        let request = match gesture {
            Gesture::Pan { .. } => None,
            Gesture::Layer { start, current, .. } if !moved(start, current) => {
                self.restore_frame();
                None
            }
            gesture @ Gesture::Layer { .. } => {
                self.preview_dirty.set(false);
                let request = self.layer_request(&gesture);
                if request.is_none() {
                    self.restore_frame();
                }
                request
            }
            Gesture::Move { id, start, end } if moved(start, end) => Some(Request::Layer {
                id,
                edit: LayerEdit::Translate {
                    delta_x: end.x - start.x,
                    delta_y: end.y - start.y,
                },
            }),
            Gesture::Move { .. } => None,
            Gesture::Freehand(points) if points.len() >= 2 => Some(Request::CreateFreehandPath {
                create: FreehandPathCreate {
                    points,
                    style,
                    opacity,
                },
            }),
            Gesture::Freehand(_) => None,
            Gesture::Brush(points) => {
                self.preview_dirty.set(false);
                let eraser = &self.properties.eraser;
                self.brush_mode().map(|mode| Request::PaintImageBackground {
                    points,
                    size: eraser.size.value(),
                    softness: eraser.softness.value(),
                    mode,
                })
            }
            Gesture::Shape { start, end, .. } if !moved(start, end) => None,
            // The rectangle stays staged for Apply crop.
            Gesture::Crop(_) => None,
            Gesture::Shape { tool, start, end } => {
                if let Some(shape) = closed_kind(&tool) {
                    Some(Request::CreateClosedShape {
                        create: ClosedShapeCreate {
                            shape,
                            start,
                            end,
                            style,
                            opacity,
                        },
                    })
                } else {
                    open_kind(&tool).map(|shape| Request::CreateOpenShape {
                        create: OpenShapeCreate {
                            shape,
                            start,
                            end,
                            style,
                            opacity,
                        },
                    })
                }
            }
        };
        if let Some(request) = request {
            *self.notice.borrow_mut() = None;
            self.submit(request);
        }
        self.sync_cursor();
        self.canvas.queue_draw();
    }

    /// Shipping `toolShowsTransformChrome`: Select and the shape tools.
    fn transform_tool(&self) -> bool {
        let tool = self.tool.borrow();
        *tool == "v" || *tool == "a" || SHAPE_TOOLS.iter().any(|shape| shape.key == *tool)
    }

    fn display_scale(&self) -> f64 {
        self.zoom_percent().unwrap_or(100.) / 100.
    }

    /// The selected layer, its outline and the grip under `point`.
    fn grip_at(&self, point: Point) -> Option<(Element, [Point; 4], canvas_chrome::Hit)> {
        let id = self.selected.borrow().clone()?;
        let presented = self.presented.borrow();
        let presented = presented.as_ref()?;
        let element = presented
            .document
            .elements
            .iter()
            .find(|element| element.base().id == id)?;
        let outline = *presented.outlines.get(&id)?;
        let bounds = &presented.text_bounds;
        let hit = canvas_chrome::hit(
            element,
            outline,
            point,
            self.display_scale(),
            (presented.document.width, presented.document.height),
            &mut |text| {
                bounds
                    .bounds(text)
                    .or_else(|_| captures_app::editor_text::selection_bounds(text))
            },
        )?;
        Some((element.clone(), outline, hit))
    }

    /// The selected, unlocked line or arrow.
    fn selected_curve(&self) -> Option<captures_app::editor::ShapeElement> {
        let id = self.selected.borrow().clone()?;
        let presented = self.presented.borrow();
        presented
            .as_ref()?
            .document
            .elements
            .iter()
            .find_map(|element| match element {
                Element::Shape(shape)
                    if shape.base.id == id
                        && !shape.base.locked
                        && captures_app::editor_canvas::is_curveable(shape) =>
                {
                    Some(shape.clone())
                }
                _ => None,
            })
    }

    /// Shipping's double-click on a selected line or arrow: a control dot
    /// goes away, and the path gains a dot there.
    fn edit_curve_at(&self, point: Point) -> bool {
        let Some(shape) = self.selected_curve() else {
            return false;
        };
        let radius = canvas_chrome::HIT_PX / self.display_scale();
        let edit = match captures_app::editor_canvas::hit_test_curve_handle(&shape, point, radius) {
            Some(CurveHandle::Control { index }) => CurveEdit::Remove { index },
            Some(_) => return false,
            None => {
                let (_, _, distance) =
                    captures_app::editor_canvas::closest_point_on_curve(&shape, point);
                if distance > captures_app::editor_canvas::curve_path_hit_radius(&shape, radius) {
                    return false;
                }
                CurveEdit::Insert { point }
            }
        };
        self.submit(Request::Layer {
            id: shape.base.id.clone(),
            edit: LayerEdit::Curve { edit },
        });
        true
    }

    /// Drop a grip drag's preview and show the committed frame again.
    fn restore_frame(&self) {
        if self.gesture_frame.borrow_mut().take().is_some()
            && let Some(presented) = self.presented.borrow().as_ref()
        {
            self.show_frame(&presented.pixels);
        }
        self.canvas.queue_draw();
    }

    /// Upload a frame for the canvas and hand it to the mini preview.
    fn show_frame(&self, pixels: &RgbaImage) {
        let texture = texture_of(pixels);
        (self.canvas_changed.borrow())(&texture);
        *self.texture.borrow_mut() = Some(texture);
    }

    /// Grip cursors and shipping's hover tips over the selected layer.
    fn hover_grips(&self, x: f64, y: f64) {
        if self.gesture.borrow().is_some() || self.brush_mode().is_some() || self.pan_ready.get() {
            return;
        }
        let point = self.to_document(x, y).filter(|_| self.transform_tool());
        let found = point.and_then(|point| {
            let id = self.selected.borrow().clone()?;
            let element = self
                .presented
                .borrow()
                .as_ref()?
                .document
                .elements
                .iter()
                .find(|element| element.base().id == id && !element.base().locked)?
                .clone();
            let hit = self.grip_at(point).map(|(_, _, hit)| hit);
            Some((element, hit, point))
        });
        let hint = found.as_ref().and_then(|(element, hit, point)| {
            canvas_chrome::hint(element, *hit, *point, self.display_scale())
        });
        if self.canvas.tooltip_text().as_deref() != hint.as_deref() {
            self.canvas.set_tooltip_text(hint.as_deref());
        }
        match found {
            Some((element, Some(hit), _)) => self
                .canvas
                .set_cursor_from_name(Some(canvas_chrome::cursor(hit, element.base().rotation()))),
            _ => self.sync_cursor(),
        }
    }

    /// The selected layer's chrome: dashed bounds, grips, curve dots and the
    /// rotation grip, following a move offset or a grip drag's preview.
    fn draw_selection(&self, snapshot: &gtk::Snapshot, id: &str, gesture: Option<&Gesture>) {
        let presented = self.presented.borrow();
        let Some(presented) = presented.as_ref() else {
            return;
        };
        let Some(element) = presented
            .document
            .elements
            .iter()
            .find(|element| element.base().id == id)
        else {
            return;
        };
        let frame = self.gesture_frame.borrow();
        let Some(points) = frame
            .as_ref()
            .and_then(|frame| frame.outlines.get(id))
            .or_else(|| presented.outlines.get(id))
            .copied()
        else {
            return;
        };
        let (ox, oy) = match gesture {
            Some(Gesture::Move { start, end, .. }) => (end.x - start.x, end.y - start.y),
            _ => (0., 0.),
        };
        let screen = |p: Point| {
            self.to_screen(Point {
                x: p.x + ox,
                y: p.y + oy,
            })
        };
        let Some(outline) = points
            .iter()
            .map(|p| screen(*p))
            .collect::<Option<Vec<_>>>()
            .and_then(|points| <[(f64, f64); 4]>::try_from(points).ok())
        else {
            return;
        };
        let accent = self.tokens.get().rgba("theme-accent");
        let base = element.base();
        let curve = match element {
            Element::Shape(shape) if captures_app::editor_canvas::is_curveable(shape) => {
                Some(shape)
            }
            _ => None,
        };
        let live = !base.locked && base.visible && self.transform_tool();
        let idle = gesture.is_none() && frame.is_none();
        canvas_chrome::selection(snapshot, &outline, curve.is_some(), live && idle, accent);
        if let Some(shape) = curve.filter(|_| live) {
            let mut shape = shape.clone();
            let show = match gesture {
                None => frame.is_none(),
                Some(Gesture::Layer {
                    drag: LayerDrag::Curve(handle),
                    current,
                    ..
                }) => captures_app::editor_canvas::apply_curve_edit(
                    &mut shape,
                    CurveEdit::Move {
                        handle: *handle,
                        point: *current,
                    },
                )
                .is_ok(),
                _ => false,
            };
            if show && let Some(handles) = captures_app::editor_canvas::curve_handles(&shape) {
                let map = |points: &[Point]| -> Vec<(f64, f64)> {
                    points.iter().filter_map(|p| self.to_screen(*p)).collect()
                };
                if let (Some(start), Some(end)) =
                    (self.to_screen(handles.start), self.to_screen(handles.end))
                {
                    let angle = (outline[1].1 - outline[0].1).atan2(outline[1].0 - outline[0].0);
                    canvas_chrome::curve(
                        snapshot,
                        [start, end],
                        &map(&handles.controls),
                        &map(&handles.starters),
                        angle,
                        accent,
                    );
                }
            }
        }
        if live
            && idle
            && let Some(grip) = captures_app::editor::rotation_handle(
                points,
                base.rotation(),
                self.display_scale(),
                presented.document.width,
                presented.document.height,
            )
            && let (Some(anchor), Some(handle)) =
                (self.to_screen(grip.anchor), self.to_screen(grip.handle))
        {
            canvas_chrome::rotation_grip(snapshot, anchor, handle, accent);
        }
        // `.screenshot-align-snap-guide`: 2 px accent lines with a glow.
        if let Some(frame) = frame.as_ref()
            && let Some(image) = self.image_rect()
        {
            for guide in &frame.guides {
                let rect = match guide.orientation {
                    GuideOrientation::Vertical => self
                        .to_screen(Point {
                            x: guide.position,
                            y: 0.,
                        })
                        .map(|(x, _)| (x - 1., image.y, 2., image.height)),
                    GuideOrientation::Horizontal => self
                        .to_screen(Point {
                            x: 0.,
                            y: guide.position,
                        })
                        .map(|(_, y)| (image.x, y - 1., image.width, 2.)),
                };
                let Some((x, y, w, h)) = rect else { continue };
                let bounds = graphene::Rect::new(x as f32, y as f32, w as f32, h as f32);
                let mut glow = accent;
                glow.set_alpha(0.75);
                snapshot.append_outset_shadow(
                    &gsk::RoundedRect::from_rect(bounds, 0.),
                    &glow,
                    0.,
                    0.,
                    0.,
                    10.,
                );
                snapshot.append_color(&accent, &bounds);
            }
        }
    }

    /// The layer the Expand canvas action is for: the selected layer while
    /// it hangs past the canvas and nothing is being dragged.
    fn expand_target(&self) -> Option<CanvasExpandPreview> {
        if self.gesture.borrow().is_some()
            || self.gesture_frame.borrow().is_some()
            || !self.transform_tool()
        {
            return None;
        }
        let id = self.selected.borrow().clone()?;
        self.presented
            .borrow()
            .as_ref()?
            .canvas_expand
            .get(&id)
            .cloned()
    }

    /// Show, hide and place the Expand canvas action (outside a snapshot).
    fn sync_expand(&self) {
        let target = self
            .expand_target()
            .and_then(|_| self.selected.borrow().clone());
        let shown = target.is_some();
        *self.expand_for.borrow_mut() = target;
        if self.expand_action.is_visible() != shown {
            self.expand_action.set_visible(shown);
            if !shown {
                self.expand_armed.set(false);
            }
        }
        self.text_overlay.queue_allocate();
        self.canvas.queue_draw();
    }

    /// Where the action sits: centred outside the largest overflow gap,
    /// kept inside the canvas.
    fn expand_rect(&self) -> Option<gdk::Rectangle> {
        let preview = self.expand_target()?;
        let anchor = captures_app::editor_canvas::inset_anchor(
            preview.anchor,
            preview.anchor_edge,
            captures_app::editor_canvas::EXPAND_ACTION_INSET / self.display_scale(),
        );
        let (x, y) = self.to_screen(anchor)?;
        let (_, natural) = self.expand_action.preferred_size();
        let (w, h) = (natural.width(), natural.height());
        let (cw, ch) = (self.canvas.width(), self.canvas.height());
        let left = ((x as i32) - w / 2).clamp(4, (cw - w - 4).max(4));
        let top = ((y as i32) - h / 2).clamp(4, (ch - h - 4).max(4));
        Some(gdk::Rectangle::new(left, top, w, h))
    }

    /// The After encode right of the divider, the divider, and the veil while
    /// a new encode runs (shipping `is-cover`).
    fn draw_compare(&self, snapshot: &gtk::Snapshot, bounds: &graphene::Rect) {
        let suppressed = self.compare_suppressed_now();
        if suppressed != self.compare_suppressed.get() {
            // Widgets cannot change during a snapshot; update them after it.
            let me = self.me.clone();
            glib::idle_add_local_once(move || {
                if let Some(state) = me.upgrade() {
                    state.refresh_compare();
                }
            });
        }
        if suppressed || !self.compare_visible() {
            return;
        }
        let t = self.tokens.get();
        if let Some(after) = self.compare.texture.borrow().as_ref() {
            let x = bounds.x() + bounds.width() * self.compare.split.get() as f32;
            snapshot.push_clip(&graphene::Rect::new(
                x,
                bounds.y(),
                bounds.x() + bounds.width() - x,
                bounds.height(),
            ));
            snapshot.append_texture(after, bounds);
            snapshot.pop();
            snapshot.append_color(
                &gdk::RGBA::new(0., 0., 0., 0.35),
                &graphene::Rect::new(x - 2., bounds.y(), 4., bounds.height()),
            );
            snapshot.append_color(
                &t.rgba("glass-text"),
                &graphene::Rect::new(x - 1., bounds.y(), 2., bounds.height()),
            );
        }
        if self.estimate.get().pending {
            snapshot.append_color(&t.rgba("glass-veil"), bounds);
        }
    }

    fn draw(&self, snapshot: &gtk::Snapshot, width: f32, height: f32) {
        let t = self.tokens.get();
        let light = t.rgba("text").red() < 0.5;
        // `--editor-canvas`: deep obsidian cinema well in dark mode, cooler well in light.
        snapshot.append_color(
            &t.rgba(if light { "n-5" } else { "n-1" }),
            &graphene::Rect::new(0., 0., width, height),
        );
        let (Some(rect), Some(texture)) = (self.image_rect(), self.texture.borrow().clone()) else {
            return;
        };
        let bounds = graphene::Rect::new(
            rect.x as f32,
            rect.y as f32,
            rect.width as f32,
            rect.height as f32,
        );
        let edge = gsk::RoundedRect::from_rect(bounds, 0.);
        for (x, y, blur, spread, color) in t.shadow("shadow-lg") {
            snapshot.append_outset_shadow(&edge, &color, x, y, spread, blur);
        }
        // `.screenshot-canvas-surface`: the document background under the
        // image, so erased pixels show it (a checkerboard when transparent).
        let background = self
            .presented
            .borrow()
            .as_ref()
            .map(|presented| presented.document.background.clone())
            // A brush stroke previews the candidate, whose background the
            // session clears to transparent, as the commit will.
            .map(|background| {
                background.filter(|_| !matches!(*self.gesture.borrow(), Some(Gesture::Brush(_))))
            });
        match background {
            Some(None) => {
                let tile = graphene::Rect::new(bounds.x(), bounds.y(), 16., 16.);
                snapshot.push_repeat(&bounds, Some(&tile));
                snapshot.append_color(&t.rgba("canvas-checker-b"), &tile);
                let a = t.rgba("canvas-checker-a");
                snapshot.append_color(&a, &graphene::Rect::new(bounds.x(), bounds.y(), 8., 8.));
                snapshot.append_color(
                    &a,
                    &graphene::Rect::new(bounds.x() + 8., bounds.y() + 8., 8., 8.),
                );
                snapshot.pop();
            }
            Some(Some(color)) => {
                let color = gdk::RGBA::parse(color.as_str())
                    .unwrap_or_else(|_| gdk::RGBA::new(0.969, 0.969, 0.961, 1.));
                snapshot.append_color(&color, &bounds);
            }
            None => {}
        }
        snapshot.append_texture(&texture, &bounds);
        let border = t.rgba("border");
        let outline = gsk::RoundedRect::from_rect(
            graphene::Rect::new(
                bounds.x() - 1.,
                bounds.y() - 1.,
                bounds.width() + 2.,
                bounds.height() + 2.,
            ),
            0.,
        );
        snapshot.append_border(&outline, &[1.; 4], &[border; 4]);
        self.draw_compare(snapshot, &bounds);

        let accent = t.rgba("theme-accent");
        let gesture = self.gesture.borrow().clone();
        // Expand canvas: the overflow tint and, while the action is hovered,
        // the ghost of the grown canvas.
        let expand = self.expand_target();
        snapshot.push_clip(&graphene::Rect::new(0., 0., width, height));
        if let Some(preview) = &expand {
            canvas_chrome::expand(
                snapshot,
                preview,
                &|point| self.to_screen(point),
                self.expand_armed.get(),
                accent,
            );
        }
        // A file drag: where the image will land.
        if let Some(guide) = self.drop_guide.borrow().as_ref() {
            canvas_chrome::drop_guide(
                snapshot,
                guide,
                &|point| self.to_screen(point),
                accent,
                t.number("r-md"),
            );
        }
        // Trim edges: the margins it would cut, while its button is hovered.
        if self.trim_hover.get() {
            let preview = self
                .presented
                .borrow()
                .as_ref()
                .filter(|presented| presented.can_trim)
                .and_then(|presented| presented.trim_preview.clone());
            if let Some(preview) = preview {
                canvas_chrome::trim(snapshot, &preview, &|point| self.to_screen(point));
            }
        }
        snapshot.pop();
        // Gestures, tool and selection changes all redraw: follow them with
        // the quick bar once the snapshot is done.
        if self.quick_bar_id() != *self.quick_for.borrow() {
            let me = self.me.clone();
            glib::idle_add_local_once(move || {
                if let Some(state) = me.upgrade() {
                    state.sync_quick_bar();
                }
            });
        }
        let wanted = expand.and(self.selected.borrow().clone());
        if wanted != *self.expand_for.borrow() {
            // Widgets cannot change during a snapshot.
            let me = self.me.clone();
            glib::idle_add_local_once(move || {
                if let Some(state) = me.upgrade() {
                    state.sync_expand();
                }
            });
        }
        // The selected layer's chrome, moved with a live drag.
        let selected = self.selected.borrow().clone();
        if let Some(id) = selected {
            self.draw_selection(snapshot, &id, gesture.as_ref());
        }
        match &gesture {
            Some(Gesture::Shape { tool, start, end }) => {
                let (Some(a), Some(b)) = (self.to_screen(*start), self.to_screen(*end)) else {
                    return;
                };
                if open_kind(tool).is_some() {
                    stroke(snapshot, &[a, b], false, accent, 2.);
                } else {
                    stroke(
                        snapshot,
                        &[(a.0, a.1), (b.0, a.1), (b.0, b.1), (a.0, b.1)],
                        true,
                        accent,
                        1.5,
                    );
                }
            }
            Some(Gesture::Freehand(points)) => {
                let screen: Vec<(f64, f64)> =
                    points.iter().filter_map(|p| self.to_screen(*p)).collect();
                stroke(snapshot, &screen, false, accent, 2.);
            }
            _ => {}
        }
        self.draw_crop(snapshot, &bounds);
        self.draw_brush_ring(snapshot);
    }

    /// Stage, change or drop the crop rectangle.
    fn set_crop(&self, crop: Option<Rect>) {
        if self.crop.replace(crop) == crop {
            return;
        }
        self.properties.crop.sync(crop);
        self.canvas.queue_draw();
    }

    /// Shipping `applyCrop`: crop, then back to Select with nothing selected.
    fn apply_crop(&self) {
        let Some(rect) = self.crop.get() else { return };
        self.set_crop(None);
        self.selected.borrow_mut().take();
        self.rebuild_selection();
        self.submit(Request::Crop { rect });
        self.select_tool("v");
    }

    /// Shipping `drawEditorOverlays`' crop: the dimmed outside, a white
    /// 8/5 dashed outline and the `W × H` tag, constant in screen size.
    fn draw_crop(&self, snapshot: &gtk::Snapshot, image: &graphene::Rect) {
        let Some(crop) = self.crop.get() else { return };
        let (Some(a), Some(b)) = (
            self.to_screen(Point {
                x: crop.x,
                y: crop.y,
            }),
            self.to_screen(Point {
                x: crop.x + crop.width,
                y: crop.y + crop.height,
            }),
        ) else {
            return;
        };
        let (x0, y0, x1, y1) = (a.0 as f32, a.1 as f32, b.0 as f32, b.1 as f32);
        let shade = gdk::RGBA::new(5. / 255., 6. / 255., 8. / 255., 0.64);
        // Whole-pixel strips: translucent edges meeting at a fraction seam.
        let (il, it) = (image.x(), image.y());
        let (ir, ib) = (il + image.width(), it + image.height());
        let [il, it, ir, ib, x0, y0, x1, y1] = [il, it, ir, ib, x0, y0, x1, y1].map(f32::round);
        for part in [
            graphene::Rect::new(il, it, ir - il, y0 - it),
            graphene::Rect::new(il, y1, ir - il, ib - y1),
            graphene::Rect::new(il, y0, x0 - il, y1 - y0),
            graphene::Rect::new(x1, y0, ir - x1, y1 - y0),
        ] {
            if part.width() > 0. && part.height() > 0. {
                snapshot.append_color(&shade, &part);
            }
        }
        let outline = graphene::Rect::new(x0 - 2., y0 - 2., x1 - x0 + 4., y1 - y0 + 4.);
        let cr = snapshot.append_cairo(&outline);
        cr.set_source_rgb(1., 1., 1.);
        cr.set_line_width(2.);
        cr.set_dash(&[8., 5.], 0.);
        cr.rectangle(a.0, a.1, b.0 - a.0, b.1 - a.1);
        let _ = cr.stroke();
        drop(cr);
        let layout = self
            .canvas
            .create_pango_layout(Some(&format!("{} × {}", crop.width, crop.height)));
        let mut font = gtk::pango::FontDescription::new();
        font.set_weight(gtk::pango::Weight::Bold);
        font.set_absolute_size(12. * f64::from(gtk::pango::SCALE));
        layout.set_font_description(Some(&font));
        let (_, logical) = layout.pixel_extents();
        let (tw, th) = (logical.width() as f32, logical.height() as f32);
        let (lw, cx) = (tw + 14., (x0 + x1) / 2.);
        snapshot.append_color(
            &gdk::RGBA::new(17. / 255., 18. / 255., 22. / 255., 1.),
            &graphene::Rect::new(cx - lw / 2., y0 + 8., lw, 24.),
        );
        snapshot.save();
        snapshot.translate(&graphene::Point::new(cx - tw / 2., y0 + 20. - th / 2.));
        snapshot.append_layout(&layout, &gdk::RGBA::WHITE);
        snapshot.restore();
    }

    /// Shipping `.screenshot-brush-cursor`: a ring the brush's document size
    /// at the current zoom, in its own small Cairo node.
    fn draw_brush_ring(&self, snapshot: &gtk::Snapshot) {
        let (Some(mode), Some((x, y))) = (self.brush_mode(), self.pointer.get()) else {
            return;
        };
        if self.brush_hover() != brush_cursor::Hover::Ring {
            return;
        }
        let scale = self.zoom_percent().unwrap_or(100.) / 100.;
        let diameter = brush_cursor::screen_diameter(self.properties.eraser.size.value(), scale);
        let radius = diameter / 2.;
        let pad = 3.;
        let bounds = graphene::Rect::new(
            (x - radius - pad) as f32,
            (y - radius - pad) as f32,
            (diameter + 2. * pad) as f32,
            (diameter + 2. * pad) as f32,
        );
        let cr = snapshot.append_cairo(&bounds);
        let rgba =
            |cr: &gtk::cairo::Context, [r, g, b, a]: [f64; 4]| cr.set_source_rgba(r, g, b, a);
        let ring = brush_cursor::RING_WIDTH;
        cr.arc(x, y, radius - ring / 2., 0., std::f64::consts::TAU);
        match mode {
            BrushMode::Erase => rgba(&cr, brush_cursor::ERASE_FILL_RGBA),
            BrushMode::Restore => {
                let accent = self.tokens.get().rgba("theme-accent");
                cr.set_source_rgba(
                    accent.red().into(),
                    accent.green().into(),
                    accent.blue().into(),
                    brush_cursor::RESTORE_FILL_ACCENT_ALPHA,
                );
            }
        }
        let _ = cr.fill();
        // Halo outside, then the ring (dashed for Restore), then the inset.
        cr.set_line_width(brush_cursor::HALO_WIDTH);
        rgba(&cr, brush_cursor::HALO_RGBA);
        cr.arc(
            x,
            y,
            radius + brush_cursor::HALO_WIDTH / 2.,
            0.,
            std::f64::consts::TAU,
        );
        let _ = cr.stroke();
        cr.set_line_width(ring);
        rgba(&cr, brush_cursor::RING_RGBA);
        if mode == BrushMode::Restore {
            cr.set_dash(&brush_cursor::RESTORE_DASH, 0.);
        }
        cr.arc(x, y, radius - ring / 2., 0., std::f64::consts::TAU);
        let _ = cr.stroke();
        cr.set_dash(&[], 0.);
        cr.set_line_width(brush_cursor::INSET_WIDTH);
        rgba(&cr, brush_cursor::INSET_RGBA);
        cr.arc(
            x,
            y,
            (radius - ring - brush_cursor::INSET_WIDTH / 2.).max(0.),
            0.,
            std::f64::consts::TAU,
        );
        let _ = cr.stroke();
    }
}

/// A polyline in a Cairo node bounded to its own box, so only those pixels
/// rasterize, never the canvas or the image under it.
fn stroke(
    snapshot: &gtk::Snapshot,
    points: &[(f64, f64)],
    close: bool,
    color: gdk::RGBA,
    width: f64,
) {
    if points.len() < 2 {
        return;
    }
    let pad = width + 1.;
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for (x, y) in points {
        x0 = x0.min(*x);
        y0 = y0.min(*y);
        x1 = x1.max(*x);
        y1 = y1.max(*y);
    }
    let bounds = graphene::Rect::new(
        (x0 - pad) as f32,
        (y0 - pad) as f32,
        (x1 - x0 + 2. * pad) as f32,
        (y1 - y0 + 2. * pad) as f32,
    );
    let cr = snapshot.append_cairo(&bounds);
    cr.set_source_rgba(
        color.red().into(),
        color.green().into(),
        color.blue().into(),
        color.alpha().into(),
    );
    cr.set_line_width(width);
    cr.set_line_join(gtk::cairo::LineJoin::Round);
    cr.set_line_cap(gtk::cairo::LineCap::Round);
    cr.move_to(points[0].0, points[0].1);
    for (x, y) in &points[1..] {
        cr.line_to(*x, *y);
    }
    if close {
        cr.close_path();
    }
    let _ = cr.stroke();
}
