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
    ClosedShapeCreate, ClosedShapeKind, Document, ElementStyle, FreehandPathCreate, LayerEdit,
    OpenShapeCreate, OpenShapeKind, Point, Rect,
};
use captures_app::editor_chrome::{self, RAIL_TOOLS, SHAPE_TOOLS, brush_cursor, header};
use captures_app::editor_export::{self, EstimateState, ExportSource, ExportTarget};
use captures_app::editor_image_background::BrushMode;
use captures_app::editor_session::{
    EditorSession, ExportFormat, ExportOptions, ExportQuality, ExportSize, OpenRequest, PngOptions,
    Request,
};
use captures_app::editor_viewport::{self, Viewport};
use gtk::prelude::*;
use gtk::{gdk, glib, graphene, gsk};
use image::RgbaImage;

use crate::switch::TokenSwitch;
use crate::tokens::Tokens;
use crate::widgets::{Icon, SnapshotArea, icon_button};

mod canvas_bar;
mod controls;
mod export_bar;
mod inspector;
mod properties;
mod text;
pub use text::register_fonts;

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
    style: ElementStyle,
    has_draft: bool,
    source: Option<ExportSource>,
    initial_text_size: f64,
    /// Document family keys to installed face names (`"rounded"` → Nunito).
    font_families: BTreeMap<String, String>,
    active_input: Option<ActiveInput>,
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
}

enum Job {
    Apply(Request),
    /// An uncommitted brush stroke, rendered without touching history.
    Preview(Request),
    /// Decode image files and import them as one edit.
    Import {
        paths: Vec<PathBuf>,
        selected_id: Option<String>,
        point: Option<Point>,
    },
    Save {
        plan: editor_export::SavePlan,
        options: ExportOptions,
        mode: captures_capture::CaptureMode,
    },
}

enum Outcome {
    Presented(Box<Presented>),
    /// Images were imported; select the last new layer.
    Imported(Box<Presented>, String),
    Preview(Result<Arc<RgbaImage>, String>),
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

fn present(session: &EditorSession, artifact_id: &str) -> Presented {
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
    /// An Eraser Erase/Restore stroke.
    Brush(Vec<Point>),
    /// Moving the selected layer by the pointer's document delta.
    Move {
        id: String,
        start: Point,
        end: Point,
    },
    /// Panning from the viewport it started with.
    Pan {
        from: Viewport,
        dx: f64,
        dy: f64,
    },
}

pub struct Editor {
    pub window: gtk::ApplicationWindow,
    state: Rc<State>,
}

struct State {
    tokens: Rc<Tokens>,
    output_directory: PathBuf,
    default_stem: String,
    mode: captures_capture::CaptureMode,
    jobs: mpsc::Sender<Job>,
    session: Arc<std::sync::Mutex<Option<EditorSession>>>,
    presented: RefCell<Option<Presented>>,
    texture: RefCell<Option<gdk::Texture>>,
    viewport: Cell<Viewport>,
    tool: RefCell<String>,
    last_shape: RefCell<String>,
    selected: RefCell<Option<String>>,
    gesture: RefCell<Option<Gesture>>,
    target: RefCell<Option<ExportTarget>>,
    options: Cell<ExportOptions>,
    notice: RefCell<Option<(String, bool)>>,
    busy: Cell<bool>,
    canvas: SnapshotArea,
    /// The Text tool's inline field over the canvas.
    text_overlay: gtk::Overlay,
    text_field: text::TextField,
    inline: RefCell<Option<text::Inline>>,
    input_counter: Cell<u64>,
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
    estimate_key: Cell<Option<(usize, ExportOptions)>>,
    estimate_generation: Cell<u64>,
    estimate_timer: RefCell<Option<glib::SourceId>>,
    /// The History entry's file size, the Preserve baseline.
    original_bytes: Option<u64>,
    me: std::rc::Weak<State>,
    preview_dirty: Cell<bool>,
    /// The previous primary press, for double-click editing.
    last_press: Cell<Option<(std::time::Instant, f64, f64)>>,
    widgets: Widgets,
}

struct Widgets {
    undo: gtk::Button,
    redo: gtk::Button,
    canvas_bar: Rc<canvas_bar::CanvasBar>,
    zoom_label: gtk::Label,
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
}

fn label(text: &str, class: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.add_css_class(class);
    label.set_xalign(0.);
    label
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
    window.set_size_request(860, 600);

    // The worker thread owns the session; the UI thread reads results.
    let session: Arc<std::sync::Mutex<Option<EditorSession>>> = Arc::default();
    let (jobs, inbox) = mpsc::channel::<Job>();
    let (results, outcomes) = std::sync::mpsc::channel::<Outcome>();
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
                    Ok(opened) => {
                        let _ = results.send(Outcome::Presented(Box::new(present(&opened, &id))));
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
                        Job::Preview(request) => Outcome::Preview(session.preview_drawing(request)),
                        Job::Import {
                            paths,
                            selected_id,
                            point,
                        } => match captures_app::editor_image_decode::decode_imports(paths)
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
    window.set_child(Some(&root));

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
    // Requests from the canvas toolbar reach the worker once State exists.
    let canvas_jobs = jobs.clone();
    let canvas_bar = canvas_bar::CanvasBar::new(&tokens, move |request| {
        let _ = canvas_jobs.send(Job::Apply(request));
    });
    left.append(&canvas_bar.root);
    main.append(&left);
    let right = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let undo = icon_only("undo", 16, header::UNDO, "editor-icon-button");
    let redo = icon_only("redo", 16, header::REDO, "editor-icon-button");
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
    let zoom_label = label("Fit", "editor-zoom-label");
    zoom_label.set_width_chars(6);
    zoom_label.set_xalign(0.5);
    let zoom_in = icon_only("plus", 14, header::ZOOM_IN, "editor-zoom-button");
    for widget in [
        fit.upcast_ref::<gtk::Widget>(),
        zoom_out.upcast_ref(),
        zoom_label.upcast_ref(),
        zoom_in.upcast_ref(),
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
        let icon = Icon::new(tool.icon, 18).with_stroke(1.75);
        button.set_child(Some(&icon));
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
    body.append(&text_overlay);
    let panel = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let heading = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    heading.add_css_class("editor-layers-heading");
    heading.append(&label("Layers", "editor-layers-title"));
    let layer_count = gtk::Label::new(Some("0"));
    layer_count.add_css_class("editor-layer-count");
    layer_count.set_valign(gtk::Align::Center);
    heading.append(&layer_count);
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
    let properties = properties::Properties::new(&tokens);
    let sidebar = properties::sidebar(&panel, &properties.root);
    sidebar.add_css_class("editor-layers");
    sidebar.set_size_request(320, -1);
    body.append(&sidebar);
    root.append(&body);

    // `.screenshot-export-bar`: the Export settings panel over the save row.
    let footer = gtk::Box::new(gtk::Orientation::Vertical, 12);
    footer.add_css_class("editor-export-bar");
    let late: Rc<RefCell<std::rc::Weak<State>>> = Rc::default();
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
    // `.screenshot-save-row`: minmax(178px, 210px).
    export.disclosure.set_size_request(210, -1);
    export.disclosure.set_hexpand(false);
    bar.append(&export.disclosure);
    let file = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let file_heading = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    file_heading.append(&label("Filename", "editor-field-label"));
    let folder = label("", "editor-folder");
    folder.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    folder.set_max_width_chars(28);
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
    filename.set_width_chars(28);
    filename.update_property(&[gtk::accessible::Property::Label("Filename")]);
    file_row.append(&filename);
    file_row.append(&export.format.root);
    file.append(&file_row);
    bar.append(&file);
    let (copy, _, _) = icon_button("copy", "Copy image", &["ui-btn"]);
    copy.set_valign(gtk::Align::End);
    bar.append(&copy);
    let status = gtk::Box::new(gtk::Orientation::Vertical, 2);
    status.set_hexpand(true);
    status.set_valign(gtk::Align::Center);
    let notice = label("", "editor-notice");
    notice.set_xalign(1.);
    notice.set_ellipsize(gtk::pango::EllipsizeMode::Start);
    notice.set_accessible_role(gtk::AccessibleRole::Status);
    let hint = label("", "editor-hint");
    hint.set_xalign(1.);
    hint.set_wrap(true);
    hint.set_justify(gtk::Justification::Right);
    status.append(&notice);
    status.append(&hint);
    bar.append(&status);
    let as_new = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    as_new.set_valign(gtk::Align::Center);
    let save_as_new = TokenSwitch::new(tokens.clone(), false);
    save_as_new.update_property(&[gtk::accessible::Property::Label("Save as new file")]);
    as_new.append(&save_as_new);
    as_new.append(&label("Save as new file", "editor-switch-label"));
    bar.append(&as_new);
    let (save, _, _) = icon_button("save", "Save", &["ui-btn", "primary", "editor-save"]);
    save.set_valign(gtk::Align::Center);
    bar.append(&save);
    root.append(&footer);

    let output_directory = options.output_directory.clone();
    let original_bytes =
        editor_export::original_size_bytes(&options.history_root, &options.artifact_id);
    let state = Rc::new_cyclic(|me| State {
        tokens,
        output_directory,
        default_stem: captures_app::recording_editor_ui::timestamp_stem(&options.created_at),
        mode,
        jobs,
        session,
        presented: RefCell::default(),
        texture: RefCell::default(),
        viewport: Cell::default(),
        tool: RefCell::new("v".into()),
        last_shape: RefCell::new("rectangle".into()),
        selected: RefCell::default(),
        gesture: RefCell::default(),
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
        canvas: canvas.clone(),
        text_overlay,
        text_field,
        inline: RefCell::default(),
        input_counter: Cell::default(),
        last_press: Cell::default(),
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
        preview_dirty: Cell::default(),
        widgets: Widgets {
            undo,
            redo,
            canvas_bar,
            zoom_label,
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
        },
    });
    *late.borrow_mut() = Rc::downgrade(&state);
    state.connect(&window);

    // Deliver worker results on the UI thread.
    let weak = Rc::downgrade(&state);
    glib::timeout_add_local(std::time::Duration::from_millis(16), move || {
        let Some(state) = weak.upgrade() else {
            return glib::ControlFlow::Break;
        };
        while let Ok(outcome) = outcomes.try_recv() {
            state.receive(outcome);
        }
        glib::ControlFlow::Continue
    });

    // Flush the draft on close, as shipping's autosave does.
    let weak = Rc::downgrade(&state);
    window.connect_close_request(move |_| {
        if let Some(state) = weak.upgrade()
            && let Ok(mut session) = state.session.lock()
            && let Some(session) = session.as_mut()
        {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as u64);
            let _ = session.execute(Request::AutosaveDraft { updated_at_ms: now });
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
        let zoom = self.widgets.zoom_label.parent().expect("zoom group");
        let mut child = zoom.first_child();
        let mut index = 0;
        while let Some(button) = child {
            if let Some(button) = button.downcast_ref::<gtk::Button>() {
                let weak = Rc::downgrade(self);
                button.connect_clicked(move |_| {
                    let Some(state) = weak.upgrade() else { return };
                    match index {
                        0 => state.set_viewport(Viewport::default()),
                        1 => state.zoom_by(1. / 1.25),
                        _ => state.zoom_by(1.25),
                    }
                });
                index += 1;
            }
            child = button.next_sibling();
        }
        for (key, button) in &w.rail {
            let weak = Rc::downgrade(self);
            let key = key.clone();
            button.connect_clicked(move |button| {
                let Some(state) = weak.upgrade() else { return };
                if key == editor_chrome::SHAPES_KEY {
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

impl State {
    fn submit(&self, request: Request) {
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
        self.widgets
            .zoom_label
            .set_label(&if viewport.zoom_percent == 0. {
                "Fit".to_owned()
            } else {
                editor_chrome::zoom_label(viewport.zoom_percent)
            });
        // Only the canvas node changes; the texture is reused as is.
        self.canvas.queue_draw();
        self.style_text_field();
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
            self.finish_text();
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
    fn import_files(&self, paths: Vec<PathBuf>, point: Option<Point>) {
        if paths.is_empty() {
            return;
        }
        if self.text_active() {
            self.finish_text();
        }
        *self.notice.borrow_mut() = None;
        self.busy.set(true);
        let _ = self.jobs.send(Job::Import {
            paths,
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

    fn sync_cursor(&self) {
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
        let panning = matches!(*self.gesture.borrow(), Some(Gesture::Pan { .. }));
        brush_cursor::hover(over_image, stroking, panning)
    }

    /// Render the stroke so far on the worker, one request at a time.
    fn preview_brush(&self) {
        if self.preview_busy.get() {
            self.preview_dirty.set(true);
            return;
        }
        let (Some(Gesture::Brush(points)), Some(mode)) =
            (self.gesture.borrow().clone(), self.brush_mode())
        else {
            return;
        };
        self.preview_busy.set(true);
        self.preview_dirty.set(false);
        let eraser = &self.properties.eraser;
        let _ = self.jobs.send(Job::Preview(Request::PaintImageBackground {
            points,
            size: eraser.size.value(),
            softness: eraser.softness.value(),
            mode,
        }));
    }

    /// The Shapes flyout (`repeat(3, 44px)`).
    fn shapes_menu(self: &Rc<Self>, anchor: &gtk::Button) {
        let popover = gtk::Popover::new();
        popover.add_css_class("captures-menu");
        popover.set_has_arrow(false);
        popover.set_position(gtk::PositionType::Right);
        let grid = gtk::Grid::new();
        grid.set_row_spacing(2);
        grid.set_column_spacing(2);
        for (index, tool) in SHAPE_TOOLS.iter().enumerate() {
            let button = icon_only(tool.icon, 18, &tool.name(), "editor-shape-button");
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
                self.drain_queued();
                let first = self.presented.borrow().is_none();
                let changed_pixels = self
                    .presented
                    .borrow()
                    .as_ref()
                    .is_none_or(|old| !Arc::ptr_eq(&old.pixels, &presented.pixels));
                if changed_pixels {
                    *self.texture.borrow_mut() = Some(texture_of(&presented.pixels));
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
                }
                *self.presented.borrow_mut() = Some(*presented);
                self.rebuild_layers();
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
                let stroking = matches!(*self.gesture.borrow(), Some(Gesture::Brush(_)));
                match result {
                    // Only a stroke still in progress shows its preview; the
                    // committed result replaces it once the worker publishes.
                    Ok(pixels) if stroking => {
                        *self.texture.borrow_mut() = Some(texture_of(&pixels));
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

    fn rebuild_layers(self: &Rc<Self>) {
        let list = &self.widgets.layers;
        while let Some(row) = list.first_child() {
            list.remove(&row);
        }
        let presented = self.presented.borrow();
        let Some(presented) = presented.as_ref() else {
            return;
        };
        let selected = self.selected.borrow().clone();
        for layer in &presented.layers {
            let row = gtk::ListBoxRow::new();
            row.set_widget_name(&layer.id);
            row.add_css_class("editor-layer-row");
            let content = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            let preview = gtk::Box::new(gtk::Orientation::Vertical, 0);
            preview.add_css_class("editor-layer-preview");
            let icon = Icon::new(layer.icon, 16).with_stroke(1.7);
            icon.set_vexpand(true);
            preview.append(&icon);
            content.append(&preview);
            let copy = gtk::Box::new(gtk::Orientation::Vertical, 2);
            copy.set_hexpand(true);
            copy.set_valign(gtk::Align::Center);
            let name = label(&layer.name, "editor-layer-name");
            name.set_ellipsize(gtk::pango::EllipsizeMode::End);
            copy.append(&name);
            copy.append(&label(layer.kind, "editor-layer-kind"));
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
            content.append(&eye);
            if layer.locked {
                let lock = Icon::new("lock", 15).with_stroke(1.7);
                lock.add_css_class("editor-layer-lock");
                lock.set_valign(gtk::Align::Center);
                content.append(&lock);
            }
            row.set_child(Some(&content));
            row.update_property(&[gtk::accessible::Property::Label(&format!(
                "{} · {}",
                layer.name, layer.kind
            ))]);
            list.append(&row);
            if selected.as_deref() == Some(layer.id.as_str()) {
                list.select_row(Some(&row));
            }
        }
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
        w.export.set_sensitive(!self.busy.get());
        w.change_folder.set_sensitive(!self.busy.get());
        self.schedule_estimate(&presented.pixels);
        w.folder
            .set_label(&format!("Saving to {}", target.directory.display()));
        w.folder
            .set_tooltip_text(Some(&target.directory.display().to_string()));
        w.hint.set_label(&view.hint);
        set_class(&w.hint, "warning", view.hint_warning);
        let notice = self.notice.borrow();
        let (text, error) = match (&view.error, notice.as_ref()) {
            (Some(error), _) => (error.clone(), true),
            (None, Some((text, error))) => (text.clone(), *error),
            (None, None) => (String::new(), false),
        };
        w.notice.set_label(&text);
        set_class(&w.notice, "error", error);
        set_class(&w.notice, "success", !error && !text.is_empty());
        w.save
            .set_sensitive(view.plan.is_some() && view.error.is_none() && !self.busy.get());
        w.copy.set_sensitive(self.texture.borrow().is_some());
        if w.save_as_new.is_on() != view.saving_copy {
            w.save_as_new.activate_toggle_quietly(view.saving_copy);
        }
        w.save_as_new.set_sensitive(!view.format_requires_copy);
    }

    fn set_options(&self, options: ExportOptions) {
        self.options.set(options);
        *self.notice.borrow_mut() = None;
        self.update_export();
    }

    /// Re-encode the export off the UI thread once edits or options settle,
    /// exactly as Save would; a superseded result is dropped.
    fn schedule_estimate(&self, pixels: &Arc<RgbaImage>) {
        let options = self.options.get();
        let key = (Arc::as_ptr(pixels) as usize, options);
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
                        editor_export::estimate_export(&pixels, options, original)
                    })
                    .await;
                    let Some(state) = me.upgrade() else { return };
                    if state.estimate_generation.get() != generation {
                        return;
                    }
                    state.estimate.set(match result {
                        Ok(Ok(estimate)) => EstimateState {
                            bytes: Some(estimate.bytes),
                            baseline_bytes: estimate.baseline_bytes,
                            pending: false,
                        },
                        _ => EstimateState::default(),
                    });
                    state.update_export();
                });
            });
        *self.estimate_timer.borrow_mut() = Some(timer);
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
        self.busy.set(true);
        *self.notice.borrow_mut() = Some(("Saving…".into(), false));
        self.update_export();
        let _ = self.jobs.send(Job::Save {
            plan,
            options: self.options.get(),
            mode: self.mode,
        });
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
            if let Some(state) = weak.upgrade() {
                state.draw(snapshot, width, height);
            }
        });

        // Primary-button gestures: tools.
        let drag = gtk::GestureDrag::new();
        drag.set_button(gdk::BUTTON_PRIMARY);
        let weak = Rc::downgrade(self);
        drag.connect_drag_begin(move |_, x, y| {
            if let Some(state) = weak.upgrade() {
                state.canvas.grab_focus();
                state.begin(x, y);
            }
        });
        let weak = Rc::downgrade(self);
        drag.connect_drag_update(move |gesture, dx, dy| {
            let Some(state) = weak.upgrade() else { return };
            if let Some((x, y)) = gesture.start_point() {
                state.update(x + dx, y + dy, dx, dy);
            }
        });
        let weak = Rc::downgrade(self);
        drag.connect_drag_end(move |_, _, _| {
            if let Some(state) = weak.upgrade() {
                state.finish();
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
            .connect_get_child_position(move |_, _| weak.upgrade()?.text_field_rect());

        // Middle-button drag pans with any tool.
        let pan = gtk::GestureDrag::new();
        pan.set_button(gdk::BUTTON_MIDDLE);
        let weak = Rc::downgrade(self);
        pan.connect_drag_begin(move |_, _, _| {
            if let Some(state) = weak.upgrade() {
                *state.gesture.borrow_mut() = Some(Gesture::Pan {
                    from: state.viewport.get(),
                    dx: 0.,
                    dy: 0.,
                });
            }
        });
        let weak = Rc::downgrade(self);
        pan.connect_drag_update(move |_, dx, dy| {
            if let Some(state) = weak.upgrade() {
                state.update(0., 0., dx, dy);
            }
        });
        let weak = Rc::downgrade(self);
        pan.connect_drag_end(move |_, _, _| {
            if let Some(state) = weak.upgrade() {
                state.gesture.borrow_mut().take();
            }
        });
        self.canvas.add_controller(pan);

        // Pointer position anchors wheel zoom.
        let motion = gtk::EventControllerMotion::new();
        let weak = Rc::downgrade(self);
        motion.connect_motion(move |_, x, y| {
            let Some(state) = weak.upgrade() else { return };
            state.pointer.set(Some((x, y)));
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
        // Dropped files import at the drop point, as shipping's drop guide.
        let drop = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
        let weak = Rc::downgrade(self);
        drop.connect_drop(move |_, value, x, y| {
            let (Some(state), Ok(files)) = (weak.upgrade(), value.get::<gdk::FileList>()) else {
                return false;
            };
            let paths: Vec<_> = files.files().iter().filter_map(|f| f.path()).collect();
            let point = state.to_document(x, y);
            state.import_files(paths, point);
            true
        });
        self.canvas.add_controller(drop);
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
            if state.key(key, modifiers) {
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        window.add_controller(keys);
    }

    fn key(&self, key: gdk::Key, modifiers: gdk::ModifierType) -> bool {
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
        match (control, name.as_str()) {
            (true, "z") if shift => self.submit(Request::Redo),
            (true, "z") => self.submit(Request::Undo),
            (true, "y") => self.submit(Request::Redo),
            (true, "c") if !typing => self.copy(),
            (true, "s") => self.save(),
            (true, "0") => self.set_viewport(Viewport::default()),
            (true, "plus" | "equal" | "KP_Add") => self.zoom_by(1.25),
            (true, "minus" | "KP_Subtract") => self.zoom_by(1. / 1.25),
            _ if control || typing => return false,
            (_, "Delete" | "BackSpace") => self.delete_selected(),
            (_, "Escape") => {
                if self.gesture.borrow_mut().take().is_none() {
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

    fn begin(&self, x: f64, y: f64) {
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
        let gesture = match tool.as_str() {
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

    fn update(&self, x: f64, y: f64, dx: f64, dy: f64) {
        let point = self.to_document(x, y);
        let mut gesture = self.gesture.borrow_mut();
        match gesture.as_mut() {
            Some(Gesture::Pan { from, .. }) => {
                let mut viewport = *from;
                viewport.pan_x += dx;
                viewport.pan_y += dy;
                drop(gesture);
                self.viewport.set(viewport);
                self.canvas.queue_draw();
                if self.text_active() {
                    self.text_overlay.queue_allocate();
                }
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
            Gesture::Shape { tool, start, end } if tool == "c" => Some(Request::Crop {
                rect: Rect {
                    x: start.x.min(end.x).round(),
                    y: start.y.min(end.y).round(),
                    width: (end.x - start.x).abs().round(),
                    height: (end.y - start.y).abs().round(),
                },
            }),
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
        self.canvas.queue_draw();
    }

    fn draw(&self, snapshot: &gtk::Snapshot, width: f32, height: f32) {
        let t = &self.tokens;
        let light = t.rgba("text").red() < 0.5;
        // `--editor-canvas`: a deeper well in light mode.
        snapshot.append_color(
            &t.rgba(if light { "n-5" } else { "surface-sunken" }),
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

        let accent = t.rgba("theme-accent");
        let gesture = self.gesture.borrow().clone();
        // The selected layer's outline, moved with a live drag.
        let selected = self.selected.borrow().clone();
        if let Some(id) = selected
            && let Some(points) = self
                .presented
                .borrow()
                .as_ref()
                .and_then(|p| p.outlines.get(&id).copied())
        {
            let (ox, oy) = match &gesture {
                Some(Gesture::Move { start, end, .. }) => (end.x - start.x, end.y - start.y),
                _ => (0., 0.),
            };
            let screen: Vec<(f64, f64)> = points
                .iter()
                .filter_map(|p| {
                    self.to_screen(Point {
                        x: p.x + ox,
                        y: p.y + oy,
                    })
                })
                .collect();
            stroke(snapshot, &screen, true, accent, 1.5);
        }
        match &gesture {
            Some(Gesture::Shape { tool, start, end }) => {
                let (Some(a), Some(b)) = (self.to_screen(*start), self.to_screen(*end)) else {
                    return;
                };
                if tool == "c" {
                    // Dim everything outside the crop.
                    let shade = t.rgba("capture-shade-window");
                    let (x0, y0) = (a.0.min(b.0) as f32, a.1.min(b.1) as f32);
                    let (x1, y1) = (a.0.max(b.0) as f32, a.1.max(b.1) as f32);
                    let r = bounds;
                    for part in [
                        graphene::Rect::new(r.x(), r.y(), r.width(), (y0 - r.y()).max(0.)),
                        graphene::Rect::new(
                            r.x(),
                            y1,
                            r.width(),
                            (r.y() + r.height() - y1).max(0.),
                        ),
                        graphene::Rect::new(r.x(), y0, (x0 - r.x()).max(0.), y1 - y0),
                        graphene::Rect::new(x1, y0, (r.x() + r.width() - x1).max(0.), y1 - y0),
                    ] {
                        snapshot.append_color(&shade, &part);
                    }
                    stroke(
                        snapshot,
                        &[(a.0, a.1), (b.0, a.1), (b.0, b.1), (a.0, b.1)],
                        true,
                        accent,
                        1.5,
                    );
                } else if open_kind(tool).is_some() {
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
        self.draw_brush_ring(snapshot);
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
                let accent = self.tokens.rgba("theme-accent");
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
