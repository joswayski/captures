//! First connected screenshot editor: one serialized worker per open artifact.
//! Only snapshots and retained pixels cross to the UI; disk/render work does not.
use std::{
    fs::File,
    io::{Cursor, Read},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, Sender},
    },
    thread,
};

use captures_app::{
    editor::{
        ARROW_MIN_DRAW_LENGTH, AnnotationStylePatch, ClosedShapeCreate, ClosedShapeKind, CropDrag,
        Document, DropShadowStyle, DropShadowStylePatch, Element, ElementBase, ElementStyle,
        FreehandPathCreate, ImageTransform, LayerEdit, LayerPlacement, OpenShapeCreate,
        OpenShapeKind, OptionalNullable, Point, Rect, ShapeElement, arrow_fill_polygon,
        smooth_path_centerline,
    },
    editor_output::{SavedExport, save_new_export},
    editor_render::{MAX_RENDER_DIMENSION, MAX_RENDER_PIXELS},
    editor_session::{
        EditorSession, ExportFormat, ExportOptions, ExportQuality, ImportImage, OpenRequest,
        PngOptions, Request,
    },
};
use captures_capture::CaptureMode;
use eframe::egui::{self, RichText};
use image::{ImageDecoder, ImageFormat, ImageReader, RgbaImage};

use crate::tokens::Tokens;

enum Job {
    Apply(Request),
    Import {
        path: PathBuf,
        selected_id: Option<String>,
    },
    Preview(ExportOptions),
    Copy,
    SaveNew {
        destination: PathBuf,
        options: ExportOptions,
    },
    Flush(Sender<Result<(), String>>),
    Shutdown,
}

struct Presented {
    document: Arc<Document>,
    pixels: Arc<RgbaImage>,
    output: Option<(RgbaImage, usize)>,
    saved: Option<SavedExport>,
    copied: bool,
    created_layer: Option<String>,
    can_undo: bool,
    can_redo: bool,
    unsaved: bool,
    has_draft: bool,
}

impl Presented {
    fn from_session(session: &EditorSession) -> Self {
        let snapshot = session.snapshot();
        Self {
            document: Arc::new(snapshot.document.clone()),
            pixels: session.pixels(),
            output: None,
            saved: None,
            copied: false,
            created_layer: None,
            can_undo: snapshot.can_undo,
            can_redo: snapshot.can_redo,
            unsaved: snapshot.unsaved_changes,
            has_draft: snapshot.has_draft,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Section {
    Geometry,
    Layers,
    Output,
    Draw,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DrawShape {
    Rectangle,
    Ellipse,
    Line,
    Arrow,
    Freehand,
}

impl DrawShape {
    fn request(self, start: Point, end: Point, display_scale: f64) -> Option<Request> {
        match self {
            Self::Rectangle | Self::Ellipse if start.x != end.x && start.y != end.y => {
                Some(Request::CreateClosedShape {
                    create: ClosedShapeCreate {
                        shape: if self == Self::Rectangle {
                            ClosedShapeKind::Rectangle
                        } else {
                            ClosedShapeKind::Ellipse
                        },
                        start,
                        end,
                        style: ElementStyle::default(),
                        opacity: 100.,
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
                        style: ElementStyle::default(),
                        opacity: 100.,
                    },
                })
            }
            _ => None,
        }
    }
}

const CROP_ASPECTS: [(&str, Option<f64>); 5] = [
    ("Free", None),
    ("1:1", Some(1.)),
    ("4:3", Some(4. / 3.)),
    ("3:2", Some(3. / 2.)),
    ("16:9", Some(16. / 9.)),
];

struct AnnotationFields {
    style: ElementStyle,
    shadow: DropShadowStyle,
}

impl AnnotationFields {
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
    crop: [f64; 4],
    crop_previous: Option<[f64; 4]>,
    crop_drag: Option<CropDrag>,
    crop_aspect: usize,
    draw_shape: DrawShape,
    shape_drag: Option<(Point, Point)>,
    freehand_points: Vec<Point>,
    canvas: [f64; 2],
    section: Section,
    export_options: ExportOptions,
    output: Option<(egui::TextureHandle, usize)>,
    show_output: bool,
    destination: String,
    folder_picker: Option<Receiver<Option<PathBuf>>>,
    output_notice: Option<String>,
    import_picker: Option<Receiver<Option<PathBuf>>>,
    history_changed: bool,
    selected_layer: Option<String>,
    layer_gesture: Option<LayerGesture>,
    pending_layer_selection: Option<String>,
    layer_name: String,
    layer_opacity: f64,
    layer_position: [f64; 2],
    annotation: Option<AnnotationFields>,
    pending: bool,
    closed: bool,
    close_requested: bool,
    close_after_save: bool,
    confirm_discard: bool,
    error: Option<String>,
}

impl Default for View {
    fn default() -> Self {
        Self {
            presented: None,
            texture: None,
            crop: [0., 0., 1., 1.],
            crop_previous: None,
            crop_drag: None,
            crop_aspect: 0,
            draw_shape: DrawShape::Rectangle,
            shape_drag: None,
            freehand_points: Vec::new(),
            canvas: [1., 1.],
            section: Section::Geometry,
            export_options: ExportOptions {
                format: ExportFormat::Png,
                quality: ExportQuality::Preserve,
                quality_value: 80,
                max_size_bytes: None,
                png: PngOptions::default(),
            },
            output: None,
            show_output: false,
            destination: String::new(),
            folder_picker: None,
            output_notice: None,
            import_picker: None,
            history_changed: false,
            selected_layer: None,
            layer_gesture: None,
            pending_layer_selection: None,
            layer_name: String::new(),
            layer_opacity: 100.,
            layer_position: [0., 0.],
            annotation: None,
            pending: true,
            closed: false,
            close_requested: false,
            close_after_save: false,
            confirm_discard: false,
            error: None,
        }
    }
}

impl View {
    fn cancel_crop(&mut self) {
        if let Some(previous) = self.crop_previous.take() {
            self.crop = previous;
        }
        self.crop_drag = None;
    }

    fn cancel_drawing(&mut self) {
        self.shape_drag = None;
        self.freehand_points.clear();
    }

    fn cancel_layer_gesture(&mut self) {
        self.layer_gesture = None;
    }

    fn title(&self) -> &'static str {
        if self.pending {
            "Screenshot editor — Captures — Working…"
        } else {
            "Screenshot editor — Captures"
        }
    }

    fn unsaved(&self) -> bool {
        self.presented.as_ref().is_some_and(|value| value.unsaved)
    }

    fn request_close(&mut self) {
        self.cancel_drawing();
        self.cancel_layer_gesture();
        self.pending_layer_selection = None;
        if self.pending || self.unsaved() {
            self.close_requested = true;
        } else {
            self.closed = true;
        }
    }

    fn receive(&mut self, ctx: &egui::Context, result: Result<Presented, String>) {
        if self.closed {
            return;
        }
        self.pending = false;
        match result {
            Ok(mut presented) => {
                let changed = self
                    .presented
                    .as_ref()
                    .is_none_or(|old| !Arc::ptr_eq(&old.pixels, &presented.pixels));
                if changed {
                    self.invalidate_output();
                    self.output_notice = None;
                    self.cancel_crop();
                    self.cancel_drawing();
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
                if let Some((image, length)) = presented.output.take() {
                    self.output = Some((
                        ctx.load_texture(
                            "encoded-screenshot",
                            egui::ColorImage::from_rgba_unmultiplied(
                                [image.width() as usize, image.height() as usize],
                                image.as_raw(),
                            ),
                            egui::TextureOptions::LINEAR,
                        ),
                        length,
                    ));
                    self.show_output = true;
                }
                if let Some(saved) = presented.saved.take() {
                    self.output_notice = Some(match saved {
                        SavedExport::Saved { path, .. } => {
                            self.history_changed = true;
                            format!("Saved copy to {}", path.display())
                        }
                        SavedExport::SavedWithoutHistory { path, warning } => {
                            format!(
                                "Saved copy to {}. History was not updated: {warning}",
                                path.display()
                            )
                        }
                    });
                }
                if presented.copied {
                    self.output_notice = Some("Copied edited pixels to the clipboard.".into());
                }
                let selected = self.pending_layer_selection.take().or_else(|| {
                    presented
                        .created_layer
                        .take()
                        .or(self.selected_layer.clone())
                });
                self.presented = Some(presented);
                self.select_layer(selected);
                self.error = None;
                if self.close_after_save || (self.close_requested && !self.unsaved()) {
                    self.closed = true;
                }
            }
            Err(error) => {
                self.pending_layer_selection = None;
                self.error = Some(error);
                self.select_layer_exact(self.selected_layer.clone());
            }
        }
        if self.close_requested && !self.unsaved() {
            self.closed = true;
        }
        self.close_after_save = false;
    }

    fn submit(&mut self, tx: &Sender<Job>, request: Request) {
        self.submit_job(tx, Job::Apply(request));
    }

    fn invalidate_output(&mut self) {
        self.output = None;
        self.show_output = false;
    }

    fn preview(&mut self, tx: &Sender<Job>) {
        self.invalidate_output();
        self.submit_job(tx, Job::Preview(self.export_options));
    }

    fn save_new(&mut self, tx: &Sender<Job>) {
        self.output_notice = None;
        self.submit_job(
            tx,
            Job::SaveNew {
                destination: PathBuf::from(&self.destination),
                options: self.export_options,
            },
        );
    }

    fn copy(&mut self, tx: &Sender<Job>) {
        self.output_notice = None;
        self.submit_job(tx, Job::Copy);
    }

    fn choose_folder(&mut self, ctx: &egui::Context) {
        if self.folder_picker.is_some() {
            return;
        }
        let directory = PathBuf::from(&self.destination)
            .parent()
            .map(|path| path.to_owned())
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
        let (tx, rx) = mpsc::channel();
        self.import_picker = Some(rx);
        let ctx = ctx.clone();
        let viewport = ctx.viewport_id();
        // Picking is independent of the session worker: quit and draft saves
        // never wait for a dialog. Decode and import happen on the worker later.
        thread::spawn(move || {
            let selected = rfd::FileDialog::new()
                .set_title("Import image")
                .add_filter(
                    "Images (PNG, JPEG, WebP, TIFF)",
                    &["png", "jpg", "jpeg", "webp", "tif", "tiff"],
                )
                .pick_file();
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
                let destination = PathBuf::from(&self.destination);
                self.destination = directory
                    .join(destination.file_name().unwrap_or_default())
                    .to_string_lossy()
                    .into_owned();
                self.output_notice = None;
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
        if self.pending || self.confirm_discard {
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
            Ok(Some(path)) => self.submit_job(
                tx,
                Job::Import {
                    path,
                    selected_id: self.selected_layer.clone(),
                },
            ),
            Ok(None) => {}
            Err(_) => self.error = Some("Image selection failed. Try again.".into()),
        }
        true
    }

    fn submit_job(&mut self, tx: &Sender<Job>, job: Job) {
        self.cancel_layer_gesture();
        match tx.send(job) {
            Ok(()) => {
                self.pending = true;
                self.error = None;
            }
            Err(_) => {
                self.pending_layer_selection = None;
                self.error =
                    Some("The editor worker stopped. Your last saved draft is preserved.".into())
            }
        }
    }

    fn select_layer(&mut self, id: Option<String>) {
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
        self.annotation = layer.and_then(|element| match element {
            Element::Shape(shape) => Some(AnnotationFields::new(&shape.style)),
            Element::Path(path) => Some(AnnotationFields::new(&path.style)),
            _ => None,
        });
        if let Some(layer) = layer {
            self.layer_name = layer_label(layer).into();
            self.layer_opacity = layer.base().opacity;
            self.layer_position = [layer.base().x, layer.base().y];
        }
    }

    fn select_layer_exact(&mut self, id: Option<String>) {
        if id.is_none() {
            self.selected_layer = None;
            self.annotation = None;
            self.layer_name.clear();
            self.layer_opacity = 100.;
            self.layer_position = [0., 0.];
        } else {
            self.select_layer(id);
        }
    }

    fn submit_layer(&mut self, tx: &Sender<Job>, edit: LayerEdit) {
        if let Some(id) = self.selected_layer.clone() {
            self.submit(tx, Request::Layer { id, edit });
        }
    }
}

#[derive(Clone)]
struct LayerGesture {
    id: Option<String>,
    outline: Option<[Point; 4]>,
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
    if session.snapshot().unsaved_changes {
        session.execute(Request::SaveDraft {
            updated_at_ms: chrono::Utc::now().timestamp_millis().max(0) as u64,
        })?;
    }
    Ok(())
}

fn decode_import(path: &Path) -> Result<RgbaImage, String> {
    let maximum = captures_history::editor_draft::MAX_IMAGE_BYTES as u64;
    let file = File::open(path).map_err(|error| error.to_string())?;
    if file.metadata().map_err(|error| error.to_string())?.len() > maximum {
        return Err("This image is too large to open in Captures.".into());
    }
    let mut bytes = Vec::new();
    file.take(maximum + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > maximum {
        return Err("This image is too large to open in Captures.".into());
    }
    let format = image::guess_format(&bytes).map_err(|error| error.to_string())?;
    if !matches!(
        format,
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP | ImageFormat::Tiff
    ) {
        return Err("Choose a PNG, JPEG, WebP or TIFF image.".into());
    }
    let mut decoder = ImageReader::with_format(Cursor::new(&bytes), format)
        .into_decoder()
        .map_err(|error| error.to_string())?;
    let (width, height) = decoder.dimensions();
    if width == 0
        || height == 0
        || width > MAX_RENDER_DIMENSION
        || height > MAX_RENDER_DIMENSION
        || u64::from(width) * u64::from(height) > MAX_RENDER_PIXELS
    {
        return Err(format!(
            "Images are limited to {MAX_RENDER_DIMENSION} pixels per side and {MAX_RENDER_PIXELS} total pixels."
        ));
    }
    // Browser-decoded imports in Tauri respect EXIF orientation. Normalize it
    // before choosing natural dimensions or owning the pixels in a draft asset.
    let orientation = decoder.orientation().map_err(|error| error.to_string())?;
    let color_error = |error| format!("Cannot convert this image's color profile to sRGB: {error}");
    let profile = if format == ImageFormat::Tiff {
        // image's TIFF adapter suppresses tag errors. Distinguish an absent ICC
        // profile from a malformed one rather than silently importing wrong colors.
        tiff::decoder::Decoder::new(Cursor::new(&bytes))
            .and_then(|mut decoder| decoder.image_ifd().find_tag(tiff::tags::Tag::IccProfile))
            .and_then(|tag| tag.map(|value| value.into_u8_vec()).transpose())
            .map_err(|error| color_error(error.to_string()))?
    } else {
        decoder
            .icc_profile()
            .map_err(|error| color_error(error.to_string()))?
    };
    if format == ImageFormat::Png {
        let metadata = png::Decoder::new(Cursor::new(&bytes))
            .read_info()
            .map_err(|error| color_error(error.to_string()))?;
        let info = metadata.info();
        // CICP takes precedence over ICC. Gamma/chromaticity-only PNGs require
        // a separate source-profile construction path, not an sRGB relabel.
        if info.coding_independent_code_points.is_some()
            || (profile.is_none()
                && info.srgb.is_none()
                && (info.gama_chunk.is_some() || info.chrm_chunk.is_some()))
        {
            return Err("This PNG's color metadata is not supported yet. Convert it to sRGB before importing.".into());
        }
    }
    let mut image =
        image::DynamicImage::from_decoder(decoder).map_err(|error| error.to_string())?;
    image.apply_orientation(orientation);
    let Some(profile) = profile else {
        return Ok(image.into_rgba8()); // Untagged files use the sRGB assumption.
    };
    let profile = moxcms::ColorProfile::new_from_slice(&profile)
        .map_err(|error| color_error(error.to_string()))?;
    // image decodes CMYK JPEG to RGB, losing the original CMYK samples. Such
    // profiles cannot safely be applied to the resulting pixels.
    let (width, height) = (image.width(), image.height());
    let (layout, samples) = match profile.color_space {
        moxcms::DataColorSpace::Rgb => (moxcms::Layout::Rgba, image.into_rgba8().into_raw()),
        moxcms::DataColorSpace::Gray if !image.color().has_color() => (
            moxcms::Layout::GrayAlpha,
            image.into_luma_alpha8().into_raw(),
        ),
        _ => return Err(
            "This image's color space is not supported yet. Convert it to sRGB before importing."
                .into(),
        ),
    };
    let transform = profile
        .create_transform_8bit(
            layout,
            &moxcms::ColorProfile::new_srgb(),
            moxcms::Layout::Rgba,
            moxcms::TransformOptions::default(),
        )
        .map_err(|error| color_error(error.to_string()))?;
    let mut pixels = RgbaImage::new(width, height);
    transform
        .transform(&samples, pixels.as_mut())
        .map_err(|error| color_error(error.to_string()))?;
    Ok(pixels)
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
        let destination = output_directory
            .join(format!(
                "Captures_{}_edited.png",
                chrono::Local::now().format("%Y-%m-%d_%H-%M-%S")
            ))
            .to_string_lossy()
            .into_owned();
        let (tx, jobs) = mpsc::channel();
        let (out, rx) = mpsc::channel();
        let wake_ctx = ctx.clone();
        let worker = thread::spawn(move || {
            let opened = EditorSession::open(OpenRequest {
                drafts_root: root.with_file_name("editor-drafts"),
                history_root: root.clone(),
                artifact_id,
            });
            let mut session = match opened {
                Ok(session) => {
                    let _ = out.send(Ok(Presented::from_session(&session)));
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
                    Job::Apply(request) => session
                        .as_mut()
                        .ok_or_else(|| "Editor is unavailable.".to_owned())
                        .and_then(|session| {
                            let creates_layer = matches!(
                                request,
                                Request::CreateClosedShape { .. }
                                    | Request::CreateOpenShape { .. }
                                    | Request::CreateFreehandPath { .. }
                            );
                            session.execute(request)?;
                            let mut presented = Presented::from_session(session);
                            if creates_layer {
                                presented.created_layer = presented
                                    .document
                                    .elements
                                    .last()
                                    .map(|element| element.base().id.clone());
                            }
                            Ok(presented)
                        }),
                    Job::Import { path, selected_id } => session
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
                                point: None,
                            })?;
                            let mut presented = Presented::from_session(session);
                            presented.created_layer = Some(id);
                            Ok(presented)
                        }),
                    Job::Preview(options) => session
                        .as_ref()
                        .ok_or_else(|| "Editor is unavailable.".to_owned())
                        .and_then(|session| {
                            let bytes = session.encode_export(options)?;
                            let image = image::load_from_memory(&bytes)
                                .map_err(|error| error.to_string())?
                                .into_rgba8();
                            let mut presented = Presented::from_session(session);
                            presented.output = Some((image, bytes.len()));
                            Ok(presented)
                        }),
                    Job::Copy => session
                        .as_ref()
                        .ok_or_else(|| "Editor is unavailable.".to_owned())
                        .and_then(|session| {
                            copy(session.pixels())?;
                            let mut presented = Presented::from_session(session);
                            presented.copied = true;
                            Ok(presented)
                        }),
                    Job::SaveNew {
                        destination,
                        options,
                    } => session
                        .as_ref()
                        .ok_or_else(|| "Editor is unavailable.".to_owned())
                        .and_then(|session| {
                            let saved = save_new_export(
                                &root,
                                &session.pixels(),
                                &destination,
                                options,
                                mode,
                            )
                            .map_err(|error| error.to_string())?;
                            let mut presented = Presented::from_session(session);
                            presented.saved = Some(saved);
                            Ok(presented)
                        }),
                    Job::Flush(reply) => {
                        let result = session.as_mut().map_or(Ok(()), save_dirty);
                        let _ = reply.send(result.clone());
                        match (result, session.as_ref()) {
                            (Ok(()), Some(session)) => Ok(Presented::from_session(session)),
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
                destination,
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

    pub fn receive(&self, ctx: &egui::Context) {
        if self.view.lock().unwrap().receive_folder() {
            ctx.request_repaint_of(self.viewport);
        }
        while let Ok(result) = self.rx.try_recv() {
            self.view.lock().unwrap().receive(ctx, result);
            ctx.request_repaint_of(self.viewport);
        }
        if self.view.lock().unwrap().receive_import(&self.tx) {
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
        self.tx
            .send(Job::Flush(tx))
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
                let mut view = state.lock().unwrap();
                if ui.input(|input| input.viewport().close_requested()) {
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::CancelClose);
                    view.request_close();
                }
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
        self.view.lock().unwrap().closed = true;
        let _ = self.tx.send(Job::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn show(ui: &mut egui::Ui, tokens: &Tokens, view: &mut View, tx: &Sender<Job>) {
    if !ui.ctx().egui_wants_keyboard_input()
        && !egui::Popup::is_any_open(ui.ctx())
        && ui.input(|input| input.key_pressed(egui::Key::Escape))
    {
        view.cancel_crop();
        view.cancel_drawing();
        view.cancel_layer_gesture();
    }
    egui::Panel::top("editor-actions").show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.heading("Screenshot editor");
            ui.label(RichText::new(if view.pending { "Working…" } else if view.unsaved() { "Unsaved edits" }
                else if view.presented.as_ref().is_some_and(|p| p.has_draft) { "Draft saved" } else { "Original screenshot" })
                .color(tokens.color("text-muted")));
        });
        ui.add_enabled_ui(!view.pending, |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui.add_enabled(view.presented.as_ref().is_some_and(|p| p.can_undo), egui::Button::new("Undo")).clicked() { view.submit(tx, Request::Undo); }
                if ui.add_enabled(view.presented.as_ref().is_some_and(|p| p.can_redo), egui::Button::new("Redo")).clicked() { view.submit(tx, Request::Redo); }
                if ui.add_enabled(view.presented.is_some(), egui::Button::new("Save draft")).clicked() {
                    view.submit(tx, Request::SaveDraft { updated_at_ms: chrono::Utc::now().timestamp_millis().max(0) as u64 });
                }
                if ui.add_enabled(view.presented.is_some(), egui::Button::new("Discard edits…")).clicked() { view.confirm_discard = true; }
                ui.separator();
                ui.selectable_value(&mut view.section, Section::Geometry, "Geometry");
                ui.selectable_value(&mut view.section, Section::Layers, "Layers");
                ui.selectable_value(&mut view.section, Section::Output, "Output");
                if ui.add_enabled(view.import_picker.is_none() && !view.close_requested && !view.confirm_discard, egui::Button::new("Import image…")).clicked() {
                    view.choose_image(ui.ctx());
                }
                ui.selectable_value(&mut view.section, Section::Draw, "Draw");
            });
        });
        if let Some(error) = &view.error { ui.colored_label(tokens.color("theme-signal"), error); }
        if view.close_requested && !view.pending {
            ui.group(|ui| {
                ui.label("Save unsaved edits before closing?");
                ui.horizontal(|ui| {
                    if ui.button("Save and close").clicked() {
                        view.close_after_save = true;
                        view.submit(tx, Request::SaveDraft { updated_at_ms: chrono::Utc::now().timestamp_millis().max(0) as u64 });
                    }
                    if ui.button("Close without saving").clicked() { view.closed = true; }
                    if ui.button("Cancel close").clicked() { view.close_requested = false; }
                });
                ui.small("Closing without saving keeps the last saved draft and original capture.");
            });
        }
        if view.confirm_discard {
            ui.group(|ui| {
                ui.label("Discard all edits and the saved draft? The original capture and exports stay unchanged.");
                ui.add_enabled_ui(!view.pending, |ui| ui.horizontal(|ui| {
                    if ui.button("Discard edits").clicked() { view.confirm_discard = false; view.submit(tx, Request::DiscardDraft); }
                    if ui.button("Cancel discard").clicked() { view.confirm_discard = false; }
                }));
            });
        }
    });
    if view.section != Section::Geometry {
        view.cancel_crop();
    }
    if view.section != Section::Draw
        || view.close_requested
        || view.confirm_discard
        || !ui.input(|input| input.focused)
    {
        view.cancel_drawing();
    }
    if view.section != Section::Layers
        || view.pending
        || view.close_requested
        || view.confirm_discard
        || !ui.input(|input| input.focused)
    {
        view.cancel_layer_gesture();
    }
    egui::Panel::left("editor-geometry").resizable(false).exact_size(230.).show(ui, |ui| {
        egui::ScrollArea::vertical().id_salt(view.section).auto_shrink([false, false]).show(ui, |ui| {
        ui.add_enabled_ui(!view.pending && view.presented.is_some(), |ui| {
            if view.section == Section::Output {
                show_output(ui, tokens, view, tx);
                return;
            }
            if view.section == Section::Layers {
                show_layers(ui, view, tx);
                return;
            }
            if view.section == Section::Draw {
                ui.heading("Draw shapes");
                let previous_tool = view.draw_shape;
                ui.horizontal_wrapped(|ui| {
                    ui.selectable_value(&mut view.draw_shape, DrawShape::Rectangle, "Rectangle");
                    ui.selectable_value(&mut view.draw_shape, DrawShape::Ellipse, "Ellipse");
                    ui.selectable_value(&mut view.draw_shape, DrawShape::Line, "Line");
                    ui.selectable_value(&mut view.draw_shape, DrawShape::Arrow, "Arrow");
                    ui.selectable_value(&mut view.draw_shape, DrawShape::Freehand, "Pen");
                });
                if view.draw_shape != previous_tool { view.cancel_drawing(); }
                ui.label("Drag to draw. Release to add one layer. Escape cancels the current drag.");
                ui.small("New shapes use the default annotation color. Change fill, stroke, shadow, opacity, position and ordering in Layers.");
                return;
            }
            ui.heading("Crop");
            ui.label("Coordinates in image pixels");
            egui::Grid::new("crop-fields").show(ui, |ui| {
                for (label, value) in ["X", "Y", "Width", "Height"].into_iter().zip(&mut view.crop) {
                    ui.label(label);
                    ui.add(egui::DragValue::new(value).range(0. ..=32768.).speed(1.)); ui.end_row();
                }
            });
            ui.horizontal(|ui| {
                if ui.button("Apply crop").clicked() {
                    let [x, y, width, height] = view.crop;
                    view.crop_previous = None;
                    view.crop_drag = None;
                    view.submit(tx, Request::Crop { rect: Rect { x, y, width, height } });
                }
                if ui.button(if view.crop_previous.is_some() { "Cancel" } else { "Draw crop" }).clicked() {
                    if view.crop_previous.is_some() {
                        view.cancel_crop();
                    } else {
                        view.crop_previous = Some(view.crop);
                    }
                }
            });
            if view.crop_previous.is_some() {
                ui.horizontal(|ui| {
                    ui.label("Aspect");
                    egui::ComboBox::from_id_salt("crop-aspect")
                        .selected_text(CROP_ASPECTS[view.crop_aspect].0)
                        .show_ui(ui, |ui| {
                            for (index, (label, _)) in CROP_ASPECTS.iter().enumerate() {
                                ui.selectable_value(&mut view.crop_aspect, index, *label);
                            }
                        });
                });
                ui.small("Drag on the canvas. Hold Shift to lock the ratio. Escape cancels; Apply crop commits.");
            }
            ui.add_space(tokens.number("s-6"));
            ui.heading("Canvas");
            egui::Grid::new("canvas-fields").show(ui, |ui| {
                for (label, value) in ["Width", "Height"].into_iter().zip(&mut view.canvas) {
                    ui.label(label);
                    ui.add(egui::DragValue::new(value).range(1. ..=16384.).speed(1.)); ui.end_row();
                }
            });
            if ui.button("Resize canvas").clicked() {
                view.submit(tx, Request::ResizeCanvas { width: view.canvas[0], height: view.canvas[1] });
            }
        });
        ui.add_space(tokens.number("s-6"));
        ui.label(RichText::new("Native editor preview").color(tokens.color("text-muted")));
        ui.small("Geometry, layers, filled shapes, drafts, new-copy export and clipboard output are connected. Other drawing tools and replacing files are still in development.");
        });
    });
    egui::CentralPanel::default().show(ui, |ui| {
        let texture = if view.show_output && view.section == Section::Output {
            view.output.as_ref().map(|(texture, _)| texture)
        } else {
            view.texture.as_ref()
        };
        if let Some(texture) = texture {
            let available = ui.available_rect_before_wrap();
            let image = ui.add(
                egui::Image::new(texture)
                    .fit_to_exact_size(ui.available_size())
                    .maintain_aspect_ratio(true),
            );
            if view.crop_previous.is_some() && !view.pending {
                show_crop(ui, tokens, view, available, image.rect);
            }
            if view.section == Section::Draw
                && !view.pending
                && !view.close_requested
                && !view.confirm_discard
            {
                show_shape(ui, view, tx, available, image.rect);
            }
            if view.section == Section::Layers
                && !view.pending
                && !view.close_requested
                && !view.confirm_discard
            {
                show_layer_canvas(ui, tokens, view, tx, available, image.rect);
            }
        } else if view.pending {
            ui.centered_and_justified(|ui| {
                ui.spinner();
            });
        } else {
            ui.centered_and_justified(|ui| {
                ui.label("Could not open this screenshot. See the error above.");
            });
        }
    });
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
) {
    let Some(presented) = &view.presented else {
        return;
    };
    let document = presented.document.clone();
    let bounds = Rect {
        x: 0.,
        y: 0.,
        width: f64::from(presented.pixels.width()),
        height: f64::from(presented.pixels.height()),
    };
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
        .on_hover_text("Click to select. Drag the outline and release to move. Escape cancels.");
    let first_pass = ui.ctx().current_pass_index() == 0;
    let input_enabled = ui.input(|input| input.focused) && !egui::Popup::is_any_open(ui.ctx());
    if !input_enabled {
        view.cancel_layer_gesture();
    }
    if first_pass && input_enabled {
        // Process in order: hover after release must not change the committed
        // delta, and a press/release in one frame must still be a plain click.
        for event in ui.input(|input| input.events.clone()) {
            match event {
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    ..
                } if !view.pending => {
                    view.cancel_layer_gesture();
                    if !preview.contains(pos) {
                        continue;
                    }
                    let point = image_point(pos, preview, bounds);
                    match document.hit_test(point, 8. * bounds.width / f64::from(preview.width())) {
                        Ok(hit) => {
                            view.layer_gesture = Some(LayerGesture {
                                id: hit.map(|element| element.base().id.clone()),
                                outline: hit.and_then(|element| element.selection_outline().ok()),
                                start: point,
                                current: point,
                                preview,
                            });
                            view.error = None;
                        }
                        Err(error) => view.error = Some(error),
                    }
                }
                egui::Event::PointerMoved(pos) => {
                    if let Some(gesture) = &mut view.layer_gesture {
                        gesture.current = image_point(pos, preview, bounds);
                    }
                }
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    ..
                } => {
                    let Some(gesture) = view.layer_gesture.take() else {
                        continue;
                    };
                    let end = image_point(pos, preview, bounds);
                    let delta_x = end.x - gesture.start.x;
                    let delta_y = end.y - gesture.start.y;
                    let distance = (delta_x * f64::from(preview.width()) / bounds.width)
                        .hypot(delta_y * f64::from(preview.height()) / bounds.height);
                    if distance >= 3.
                        && let Some(id) = gesture.id
                    {
                        view.pending_layer_selection = Some(id.clone());
                        view.invalidate_output();
                        view.submit(
                            tx,
                            Request::Layer {
                                id,
                                edit: LayerEdit::Translate { delta_x, delta_y },
                            },
                        );
                    } else {
                        view.select_layer_exact(gesture.id);
                    }
                }
                _ => {}
            }
        }
    }
    if response.hovered() || view.layer_gesture.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    }
    let (outline, delta) = if let Some(gesture) = &view.layer_gesture {
        let dx = gesture.current.x - gesture.start.x;
        let dy = gesture.current.y - gesture.start.y;
        let moving = (dx * f64::from(preview.width()) / bounds.width)
            .hypot(dy * f64::from(preview.height()) / bounds.height)
            >= 3.;
        (
            gesture.outline,
            Point {
                x: if moving { dx } else { 0. },
                y: if moving { dy } else { 0. },
            },
        )
    } else {
        let outline = view
            .selected_layer
            .as_ref()
            .and_then(|id| {
                document
                    .elements
                    .iter()
                    .find(|element| &element.base().id == id)
            })
            .and_then(|element| element.selection_outline().ok());
        (outline, Point { x: 0., y: 0. })
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
        ui.painter()
            .with_clip_rect(available.intersect(ui.clip_rect()))
            .add(egui::Shape::line(
                points,
                egui::Stroke::new(2., tokens.color("theme-accent")),
            ));
    }
}

fn show_shape(
    ui: &mut egui::Ui,
    view: &mut View,
    tx: &Sender<Job>,
    available: egui::Rect,
    preview: egui::Rect,
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
        egui::Sense::drag(),
    );
    let first_pass = ui.ctx().current_pass_index() == 0;
    let started = response.drag_started_by(egui::PointerButton::Primary);
    if first_pass
        && started
        && let Some(origin) = ui.input(|input| input.pointer.press_origin())
    {
        let start = image_point(origin, preview, bounds);
        view.shape_drag = Some((start, start));
        if view.draw_shape == DrawShape::Freehand {
            view.freehand_points = vec![start];
        }
    }
    if first_pass && view.draw_shape == DrawShape::Freehand && view.shape_drag.is_some() {
        let minimum = 1.5 * bounds.width / f64::from(preview.width());
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
                        let point = image_point(*position, preview, bounds);
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
    if first_pass
        && (response.dragged_by(egui::PointerButton::Primary)
            || response.drag_stopped_by(egui::PointerButton::Primary))
        && let Some(position) = response.interact_pointer_pos()
        && let Some((_, end)) = &mut view.shape_drag
    {
        *end = image_point(position, preview, bounds);
    }
    if response.hovered() || response.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
    }
    let style = ElementStyle::default();
    if let Some((start, end)) = view.shape_drag {
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
        let fill =
            egui::Color32::from_hex(style.fill.as_deref().expect("default closed-shape fill"))
                .expect("default annotation color is hex");
        match view.draw_shape {
            DrawShape::Rectangle => {
                let radius = (12. * preview.width() / bounds.width as f32)
                    .min(rect.width() / 6.)
                    .min(rect.height() / 6.);
                painter.rect_filled(rect, radius, fill);
            }
            DrawShape::Ellipse => {
                painter.add(egui::Shape::ellipse_filled(
                    rect.center(),
                    rect.size() / 2.,
                    fill,
                ));
            }
            DrawShape::Line => {
                painter.line_segment(
                    [position(start), position(end)],
                    egui::Stroke::new(
                        (style.stroke_width / bounds.width) as f32 * preview.width(),
                        fill,
                    ),
                );
            }
            DrawShape::Freehand => {
                let points: Vec<_> = smooth_path_centerline(&view.freehand_points)
                    .into_iter()
                    .map(position)
                    .collect();
                let width = (style.stroke_width / bounds.width) as f32 * preview.width();
                if points.len() == 1 {
                    painter.circle_filled(points[0], width / 2., fill);
                } else {
                    // egui's open path has flat caps; shipping Pen strokes are round.
                    painter.circle_filled(points[0], width / 2., fill);
                    painter.circle_filled(points[points.len() - 1], width / 2., fill);
                    painter.add(egui::Shape::line(points, egui::Stroke::new(width, fill)));
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
                    fill,
                )));
            }
        }
    }
    if first_pass
        && response.drag_stopped_by(egui::PointerButton::Primary)
        && let Some((start, end)) = view.shape_drag.take()
    {
        let request = if view.draw_shape == DrawShape::Freehand {
            Some(Request::CreateFreehandPath {
                create: FreehandPathCreate {
                    points: std::mem::take(&mut view.freehand_points),
                    style,
                    opacity: 100.,
                },
            })
        } else {
            view.draw_shape
                .request(start, end, f64::from(preview.width()) / bounds.width)
        };
        if let Some(request) = request {
            view.submit(tx, request);
        }
    }
}

fn polygon_mesh(points: Vec<egui::Pos2>, color: egui::Color32) -> egui::Mesh {
    // epaint's filled paths require convex polygons. Tapered arrow necks are concave.
    let coordinates: Vec<_> = points
        .iter()
        .flat_map(|point| [f64::from(point.x), f64::from(point.y)])
        .collect();
    let indices = earcutr::earcut(&coordinates, &[], 2)
        .expect("shared arrow polygon has finite two-dimensional coordinates");
    let mut mesh = egui::Mesh::default();
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
    if response.drag_started_by(egui::PointerButton::Primary)
        && let Some(origin) = ui.input(|input| input.pointer.press_origin())
    {
        view.crop_drag = Some(CropDrag::new(
            image_point(origin, preview, bounds),
            bounds,
            aspect,
            shift,
        ));
    }
    if (response.dragged_by(egui::PointerButton::Primary)
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
        .with_clip_rect(preview.intersect(ui.clip_rect()));
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

fn show_output(ui: &mut egui::Ui, tokens: &Tokens, view: &mut View, tx: &Sender<Job>) {
    ui.heading("Output preview");
    let previous = view.export_options;
    let options = &mut view.export_options;
    ui.label("Format");
    ui.horizontal(|ui| {
        for (value, label) in [
            (ExportFormat::Png, "PNG"),
            (ExportFormat::Jpeg, "JPEG"),
            (ExportFormat::Webp, "WebP"),
        ] {
            ui.selectable_value(&mut options.format, value, label);
        }
    });
    ui.label("Quality");
    for (value, label) in [
        (ExportQuality::Preserve, "Preserve"),
        (ExportQuality::Compress, "Compress"),
        (ExportQuality::Maximum, "Maximum file size"),
    ] {
        if ui.radio_value(&mut options.quality, value, label).changed() {
            options.max_size_bytes = (value == ExportQuality::Maximum).then_some(1_000_000);
        }
    }
    if options.quality == ExportQuality::Compress {
        ui.horizontal(|ui| {
            ui.label("Quality value");
            let minimum = if options.format == ExportFormat::Jpeg {
                40
            } else {
                1
            };
            options.quality_value = options.quality_value.clamp(minimum, 100);
            ui.add(egui::DragValue::new(&mut options.quality_value).range(minimum..=100));
        });
        if options.format == ExportFormat::Png {
            let mut palette = options.png.max_colors.is_some();
            if ui.checkbox(&mut palette, "Custom PNG colors").changed() {
                options.png.max_colors = palette.then_some(128);
            }
            if let Some(colors) = &mut options.png.max_colors {
                ui.add(
                    egui::DragValue::new(colors)
                        .range(2..=256)
                        .suffix(" colors"),
                );
            }
        }
    }
    if let Some(bytes) = &mut options.max_size_bytes {
        ui.add(
            egui::DragValue::new(bytes)
                .range(0..=u64::MAX)
                .suffix(" bytes"),
        );
    }
    if *options != previous {
        if options.format != previous.format {
            view.destination = PathBuf::from(&view.destination)
                .with_extension(match options.format {
                    ExportFormat::Png => "png",
                    ExportFormat::Jpeg => "jpg",
                    ExportFormat::Webp => "webp",
                })
                .to_string_lossy()
                .into_owned();
        }
        view.invalidate_output();
    }
    ui.add_space(tokens.number("s-2"));
    if ui.button("Preview output").clicked() {
        view.preview(tx);
    }
    if let Some((_, length)) = &view.output {
        ui.label(format!("Encoded size: {length} bytes"));
        ui.selectable_value(&mut view.show_output, false, "Edited canvas");
        ui.selectable_value(&mut view.show_output, true, "Encoded output");
    } else {
        ui.label("Preview to calculate encoded size.");
    }
    ui.small("Preview does not save a file or a draft. JPEG flattens transparency onto white.");
    ui.separator();
    ui.horizontal(|ui| {
        ui.label("Save location");
        if ui
            .add_enabled(view.folder_picker.is_none(), egui::Button::new("Change…"))
            .on_hover_text("Choose a folder; keep the current filename")
            .clicked()
        {
            view.choose_folder(ui.ctx());
        }
    });
    if ui
        .add(egui::TextEdit::singleline(&mut view.destination).desired_width(ui.available_width()))
        .on_hover_text(&view.destination)
        .changed()
    {
        view.output_notice = None;
        view.error = None;
    }
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                view.folder_picker.is_none(),
                egui::Button::new("Save new copy"),
            )
            .clicked()
        {
            view.save_new(tx);
        }
        if ui.button("Copy pixels").clicked() {
            view.copy(tx);
        }
    });
    ui.small(
        "Existing files are never replaced. Saving a copy does not save or discard your draft.",
    );
    ui.small("Copy uses the lossless edited canvas, regardless of export quality.");
    if let Some(notice) = &view.output_notice {
        ui.label(notice);
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

fn show_layers(ui: &mut egui::Ui, view: &mut View, tx: &Sender<Job>) {
    let Some(presented) = &view.presented else {
        return;
    };
    let document = presented.document.clone();
    let elements = &document.elements;
    ui.heading("Layers");
    ui.small("Front to back");
    egui::ScrollArea::vertical()
        .id_salt("layer-list")
        .max_height(112.)
        .min_scrolled_height(112.)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for element in elements.iter().rev() {
                let base = element.base();
                let label = format!(
                    "{}{}{}",
                    layer_label(element),
                    if base.locked { " · locked" } else { "" },
                    if base.visible { "" } else { " · hidden" }
                );
                let selected = view.selected_layer.as_deref() == Some(&base.id);
                if ui
                    .add_sized(
                        [ui.available_width(), 30.],
                        egui::Button::selectable(selected, &label).truncate(),
                    )
                    .on_hover_text(&label)
                    .clicked()
                {
                    view.select_layer(Some(base.id.clone()));
                }
            }
        });
    let Some(index) = elements
        .iter()
        .position(|element| Some(&element.base().id) == view.selected_layer.as_ref())
    else {
        ui.label("No layers. Undo to restore a deleted layer.");
        return;
    };
    let element = &elements[index];
    let base = element.base();
    ui.separator();
    ui.horizontal(|ui| {
        let mut visible = base.visible;
        let mut locked = base.locked;
        if ui.checkbox(&mut visible, "Visible").changed() {
            view.submit_layer(tx, LayerEdit::Visibility { visible });
        }
        if ui.checkbox(&mut locked, "Locked").changed() {
            view.submit_layer(tx, LayerEdit::Lock { locked });
        }
    });
    if matches!(element, Element::Image(_)) {
        ui.label("Name");
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut view.layer_name).desired_width(132.));
            if ui.button("Rename").clicked() {
                view.submit_layer(
                    tx,
                    LayerEdit::Rename {
                        name: view.layer_name.clone(),
                    },
                );
            }
        });
    }
    ui.horizontal(|ui| {
        ui.label("Opacity");
        ui.add(
            egui::DragValue::new(&mut view.layer_opacity)
                .range(0. ..=100.)
                .suffix("%"),
        );
        if ui.button("Apply").clicked() {
            view.submit_layer(
                tx,
                LayerEdit::Opacity {
                    opacity: view.layer_opacity,
                },
            );
        }
    });
    ui.add_enabled_ui(!base.locked, |ui| {
        egui::Grid::new("layer-position").show(ui, |ui| {
            for (label, value) in ["X", "Y"].into_iter().zip(&mut view.layer_position) {
                ui.label(label);
                ui.add(
                    egui::DragValue::new(value)
                        .range(-32768. ..=32768.)
                        .speed(1.),
                );
                ui.end_row();
            }
        });
        ui.horizontal(|ui| {
            if ui.button("Move").clicked() {
                view.submit_layer(
                    tx,
                    LayerEdit::Translate {
                        delta_x: view.layer_position[0] - base.x,
                        delta_y: view.layer_position[1] - base.y,
                    },
                );
            }
            for (label, target, placement) in [
                ("Up", elements.get(index + 1), LayerPlacement::Before),
                (
                    "Down",
                    index.checked_sub(1).and_then(|index| elements.get(index)),
                    LayerPlacement::After,
                ),
            ] {
                if ui
                    .add_enabled(
                        target.is_some_and(|element| !element.base().locked),
                        egui::Button::new(label),
                    )
                    .clicked()
                {
                    view.submit_layer(
                        tx,
                        LayerEdit::Reorder {
                            target_id: target.unwrap().base().id.clone(),
                            placement,
                        },
                    );
                }
            }
        });
    });
    ui.horizontal(|ui| {
        if ui.button("Duplicate").clicked() {
            let new_id = uuid::Uuid::new_v4().to_string();
            view.submit_layer(
                tx,
                LayerEdit::Duplicate {
                    new_id: new_id.clone(),
                },
            );
            view.selected_layer = Some(new_id);
        }
        if ui
            .add_enabled(!base.locked, egui::Button::new("Delete"))
            .clicked()
        {
            view.submit_layer(tx, LayerEdit::Delete);
        }
    });
    if matches!(element, Element::Image(_)) {
        ui.menu_button("Transform image", |ui| {
            for (label, transform) in [
                ("Rotate left", ImageTransform::RotateCounterclockwise),
                ("Rotate right", ImageTransform::RotateClockwise),
                ("Flip horizontal", ImageTransform::FlipHorizontal),
                ("Flip vertical", ImageTransform::FlipVertical),
            ] {
                if ui.button(label).clicked() {
                    view.submit_layer(tx, LayerEdit::ImageTransform { transform });
                    ui.close();
                }
            }
        });
    }
    match element {
        Element::Shape(shape) => show_annotation(
            ui,
            view,
            tx,
            &shape.style,
            matches!(
                shape.shape.as_str(),
                "rectangle" | "ellipse" | "triangle" | "diamond" | "star"
            ),
        ),
        Element::Path(path) => show_annotation(ui, view, tx, &path.style, false),
        _ => {
            ui.small("Hidden and locked images can transform.");
        }
    }
}

fn annotation_color(ui: &mut egui::Ui, label: &str, value: &mut String) {
    ui.push_id(label, |ui| {
        let label = ui.label(label);
        ui.horizontal(|ui| {
            let [red, green, blue, _] = egui::Color32::from_hex(value)
                .unwrap_or(egui::Color32::BLACK)
                .to_srgba_unmultiplied();
            let mut rgb = [red, green, blue];
            if ui
                .color_edit_button_srgb(&mut rgb)
                .labelled_by(label.id)
                .changed()
            {
                *value = format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2]);
            }
            ui.add(egui::TextEdit::singleline(value).desired_width(132.))
                .labelled_by(label.id);
        });
    });
}

fn show_annotation(
    ui: &mut egui::Ui,
    view: &mut View,
    tx: &Sender<Job>,
    original: &ElementStyle,
    closed: bool,
) {
    let Some(fields) = &mut view.annotation else {
        return;
    };
    ui.separator();
    ui.heading("Annotation style");
    let style = &mut fields.style;
    if closed {
        let mut stroke = style.has_stroke();
        if ui.checkbox(&mut stroke, "Stroke").changed() {
            style.stroke_enabled = Some(stroke);
        }
    }
    if !closed || style.has_stroke() {
        annotation_color(ui, "Stroke color", &mut style.color);
        ui.horizontal(|ui| {
            ui.label("Stroke width");
            ui.add(
                egui::DragValue::new(&mut style.stroke_width)
                    .range(2. ..=40.)
                    .clamp_existing_to_range(false)
                    .speed(1.),
            );
        });
    }
    if closed {
        let mut filled = style.fill.is_some();
        if ui.checkbox(&mut filled, "Filled shape").changed() {
            style.fill = filled.then(|| style.color.clone());
        }
        if let Some(fill) = &mut style.fill {
            annotation_color(ui, "Fill color", fill);
        }
    }
    let mut shadow = style.has_drop_shadow();
    if ui.checkbox(&mut shadow, "Drop shadow").changed() {
        style.drop_shadow = Some(shadow);
    }
    if shadow {
        annotation_color(ui, "Shadow color", &mut fields.shadow.color);
        for (label, value, range) in [
            ("Shadow opacity", &mut fields.shadow.opacity, 0. ..=100.),
            ("Blur", &mut fields.shadow.blur, 0. ..=100.),
            ("X offset", &mut fields.shadow.offset_x, -500. ..=500.),
            ("Y offset", &mut fields.shadow.offset_y, -500. ..=500.),
        ] {
            ui.horizontal(|ui| {
                ui.label(label);
                ui.add(
                    egui::DragValue::new(value)
                        .range(range)
                        .clamp_existing_to_range(false)
                        .speed(1.),
                );
            });
        }
    }
    let patch = fields.patch(original);
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                patch != AnnotationStylePatch::default(),
                egui::Button::new("Apply style"),
            )
            .clicked()
        {
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
            if invalid {
                view.error =
                    Some("Use a hex color such as #ff3b5c. Style changes were not applied.".into());
            } else {
                view.submit_layer(tx, LayerEdit::AnnotationStyle { patch });
            }
        }
        if ui.button("Reset fields").clicked() {
            view.annotation = Some(AnnotationFields::new(original));
            view.error = None;
        }
    });
    ui.small("Apply changes one undo step. Hidden and locked annotations remain editable.");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, time::Duration};

    fn presented(unsaved: bool) -> Presented {
        Presented {
            document: Arc::new(Document::new_capture("fixture", 7., 3., None)),
            pixels: Arc::new(RgbaImage::new(7, 3)),
            output: None,
            saved: None,
            copied: false,
            created_layer: None,
            can_undo: unsaved,
            can_redo: false,
            unsaved,
            has_draft: !unsaved,
        }
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
        show_annotation(&mut ui, &mut view, &tx, &original, true);
        let mut output = ctx.end_pass();
        output.textures_delta.clear();
        let fields = view.annotation.as_ref().unwrap();
        assert_eq!(fields.style.stroke_width, 200.);
        assert_eq!(fields.shadow.blur, 170.);
        assert_eq!(fields.patch(&original), AnnotationStylePatch::default());
        assert!(rx.try_recv().is_err());
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
                    show_layer_canvas(&mut ui, &tokens, view, &tx, screen, preview);
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

        frame(&mut view, vec![button(inside, true)]);
        frame(
            &mut view,
            vec![egui::Event::PointerMoved(egui::pos2(122., 116.))],
        );
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
            matches!(rx.recv().unwrap(), Job::Apply(Request::Layer { id: target, edit: LayerEdit::Translate { delta_x, delta_y } }) if target == id && delta_x == -70. && delta_y == -50.)
        );
        assert!(rx.try_recv().is_err(), "multipass must not submit twice");
        view.receive(&ctx, Err("move failed".into()));
        assert!(view.selected_layer.is_none() && view.pending_layer_selection.is_none());
    }

    #[test]
    fn freehand_samples_all_frame_moves_once_and_ignores_hover_and_release_positions() {
        let ctx = egui::Context::default();
        let mut view = View::default();
        let mut value = presented(false);
        value.pixels = Arc::new(RgbaImage::new(1280, 640));
        view.receive(&ctx, Ok(value));
        view.draw_shape = DrawShape::Freehand;
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
                    show_shape(&mut ui, view, &tx, screen, preview);
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
        assert!(
            rx.try_recv().is_err() && view.freehand_points.is_empty() && view.shape_drag.is_none()
        );

        view.pending = false;
        frame(&mut view, vec![moved(120., 130.), button(120., 130., true)]);
        frame(&mut view, vec![button(120., 130., false)]);
        let Job::Apply(Request::CreateFreehandPath { create }) = rx.try_recv().unwrap() else {
            panic!()
        };
        assert_eq!(create.points, vec![Point { x: 40., y: 60. }]);
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
                DrawShape::Line.request(start, end, 0.5)
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
            assert!(DrawShape::Arrow.request(start, below, scale).is_none());
            let Some(Request::CreateOpenShape { create }) =
                DrawShape::Arrow.request(start, at, scale)
            else {
                panic!("an arrow at the inclusive boundary must commit")
            };
            assert_eq!(create.shape, OpenShapeKind::Arrow);
            assert_eq!((create.start, create.end), (start, at));
        }
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
    fn shape_drag_maps_reverse_and_outside_points_without_committing_until_release() {
        let ctx = egui::Context::default();
        let mut view = View::default();
        let mut value = presented(false);
        value.pixels = Arc::new(RgbaImage::new(1280, 640));
        view.receive(&ctx, Ok(value));
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
            show_shape(&mut ui, view, &tx, screen, preview);
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
                    assert_eq!(
                        kind,
                        if create.shape == ClosedShapeKind::Rectangle {
                            DrawShape::Rectangle
                        } else {
                            DrawShape::Ellipse
                        }
                    );
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
            assert_eq!(style, ElementStyle::default());
            assert_eq!(opacity, 100.);
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
                .request(Point { x: 6., y: 2. }, Point { x: 0., y: 2. }, 1.)
                .unwrap(),
            DrawShape::Arrow
                .request(Point { x: 6., y: 2. }, Point { x: 0., y: 2. }, 1.)
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
            editor.view.lock().unwrap().preview(&editor.tx);
            receive(&editor, &ctx);
            assert!(editor.view.lock().unwrap().show_output);
            editor.view.lock().unwrap().submit(&editor.tx, request);
            receive(&editor, &ctx);
            {
                let view = editor.view.lock().unwrap();
                assert!(view.error.is_none() && view.unsaved());
                assert!(!view.show_output && view.output.is_none());
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
        editor.view.lock().unwrap().preview(&editor.tx);
        receive(&editor, &ctx);
        assert!(editor.view.lock().unwrap().show_output);
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
            assert!(!view.show_output && view.output.is_none());
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
        selection.send(Some(PathBuf::from("photo.png"))).unwrap();
        view.pending = true;
        assert!(!view.receive_import(&jobs));
        assert!(queued.try_recv().is_err() && view.import_picker.is_some());
        view.pending = false;
        assert!(view.receive_import(&jobs));
        assert!(
            matches!(queued.recv().unwrap(), Job::Import { path, selected_id }
            if path == Path::new("photo.png") && selected_id.as_deref() == Some("capture-background"))
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
            selection.send(Some(PathBuf::from("stale.png"))).unwrap();
            view.closed = closed;
            view.close_requested = !closed;
            assert!(!view.receive_import(&jobs));
            assert!(view.import_picker.is_none() && queued.try_recv().is_err());
        }
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
    fn import_rejects_unusable_color_metadata_instead_of_relabeling_pixels() {
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
        assert!(
            decode_import(&path)
                .unwrap_err()
                .contains("color metadata is not supported")
        );
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
            "Choose a PNG, JPEG, WebP or TIFF image."
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
        selection.send(Some(path.clone())).unwrap();
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
        assert_eq!(view.selected_layer.as_deref(), Some("copy"));
        assert_eq!(view.layer_position, [24., 24.]);
        let (tx, rx) = mpsc::channel();
        view.submit_layer(&tx, LayerEdit::Visibility { visible: false });
        assert!(
            matches!(rx.recv().unwrap(), Job::Apply(Request::Layer { id, edit: LayerEdit::Visibility { visible: false } }) if id == "copy")
        );
        assert!(view.pending);
        view.selected_layer = Some("rejected-duplicate".into());
        view.layer_position = [999., 999.];
        view.receive(&ctx, Err("unsupported layer".into()));
        assert_eq!(view.selected_layer.as_deref(), Some("copy"));
        assert_eq!(view.layer_position, [24., 24.]);
        assert!(!view.pending);
        view.receive(&ctx, Ok(presented(false))); // Undo removed the selected copy.
        assert_eq!(view.selected_layer.as_deref(), Some("capture-background"));
        assert_eq!(view.layer_position, [0., 0.]);
        let mut empty = presented(true);
        Arc::make_mut(&mut empty.document).elements.clear();
        view.receive(&ctx, Ok(empty));
        assert!(view.selected_layer.is_none());
    }

    #[test]
    fn folder_selection_preserves_filename_and_cancelled_or_stale_results_preserve_state() {
        let ctx = egui::Context::default();
        let mut view = View::default();
        view.receive(&ctx, Ok(presented(true)));
        let frame = view.presented.as_ref().unwrap().pixels.clone();
        let document = view.presented.as_ref().unwrap().document.clone();
        let old_path = PathBuf::from("old folder").join("capture.é.webp");
        view.destination = old_path.to_string_lossy().into_owned();
        view.output_notice = Some("Previous result".into());
        let (tx, rx) = mpsc::channel();
        view.folder_picker = Some(rx);
        view.receive_folder();
        assert!(view.folder_picker.is_some() && !view.pending);
        tx.send(None).unwrap();
        view.receive_folder();
        assert!(view.folder_picker.is_none());
        assert_eq!(PathBuf::from(&view.destination), old_path);
        assert_eq!(view.output_notice.as_deref(), Some("Previous result"));

        let (tx, rx) = mpsc::channel();
        view.folder_picker = Some(rx);
        drop(tx);
        view.receive_folder();
        assert!(view.error.is_some() && view.folder_picker.is_none());
        assert_eq!(PathBuf::from(&view.destination), old_path);
        let (tx, rx) = mpsc::channel();
        view.folder_picker = Some(rx);
        tx.send(Some(PathBuf::from("chosen folder"))).unwrap();
        view.receive_folder();
        let chosen = PathBuf::from("chosen folder").join("capture.é.webp");
        assert_eq!(PathBuf::from(&view.destination), chosen);
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
        assert_eq!(PathBuf::from(&view.destination), chosen);
        assert!(view.closed && view.folder_picker.is_none());
    }

    #[test]
    fn close_waits_for_pending_edits_and_failed_save_keeps_the_window_recoverable() {
        let ctx = egui::Context::default();
        let mut view = View::default();
        view.request_close();
        assert!(!view.closed);
        view.receive(&ctx, Ok(presented(true)));
        assert!(view.close_requested && !view.closed && view.unsaved());
        view.close_after_save = true;
        view.receive(&ctx, Err("disk full".into()));
        assert!(!view.closed && view.unsaved());
        assert_eq!(view.error.as_deref(), Some("disk full"));
        assert!(!view.close_after_save);
        view.close_after_save = true;
        view.receive(&ctx, Ok(presented(false)));
        assert!(view.closed);
        // A stale completion cannot reopen a closed controller.
        view.receive(&ctx, Ok(presented(true)));
        assert!(view.closed && !view.unsaved());
        let mut failed_open = View::default();
        failed_open.request_close();
        failed_open.receive(&ctx, Err("missing screenshot".into()));
        assert!(failed_open.closed);
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
    fn output_worker_decodes_formats_and_preserves_dirty_state_on_failure_and_retry() {
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
        for format in [ExportFormat::Png, ExportFormat::Jpeg, ExportFormat::Webp] {
            {
                let mut view = editor.view.lock().unwrap();
                view.export_options.format = format;
                view.export_options.quality = if format == ExportFormat::Jpeg {
                    ExportQuality::Compress
                } else {
                    ExportQuality::Preserve
                };
                view.preview(&editor.tx);
                assert!(view.pending && view.output.is_none() && !view.show_output);
            }
            let result = editor
                .rx
                .recv_timeout(Duration::from_secs(10))
                .unwrap()
                .unwrap();
            let (image, length) = result.output.as_ref().unwrap();
            assert_eq!(image.dimensions(), (4, 2));
            assert!(*length > 0 && result.unsaved && result.can_undo && !result.has_draft);
            if format != ExportFormat::Jpeg {
                assert_eq!(image.get_pixel(0, 0).0, [62, 71, 9, 255]);
                assert_eq!(image.get_pixel(3, 1).0, [155, 142, 9, 255]);
            }
            assert!(Arc::ptr_eq(&pixels, &result.pixels));
            editor.view.lock().unwrap().receive(&ctx, Ok(result));
            assert!(editor.view.lock().unwrap().show_output);
        }
        {
            let mut view = editor.view.lock().unwrap();
            view.export_options.max_size_bytes = Some(0);
            view.preview(&editor.tx);
        }
        receive(&editor, &ctx);
        {
            let mut view = editor.view.lock().unwrap();
            assert!(view.error.is_some() && view.output.is_none() && !view.show_output);
            assert!(view.unsaved() && !view.pending && !view.closed);
            view.export_options.max_size_bytes = None;
            view.preview(&editor.tx);
        }
        receive(&editor, &ctx);
        assert!(editor.view.lock().unwrap().output.is_some());
        editor
            .view
            .lock()
            .unwrap()
            .submit(&editor.tx, Request::Undo);
        receive(&editor, &ctx);
        let view = editor.view.lock().unwrap();
        assert!(view.output.is_none() && !view.show_output);
        assert!(view.presented.as_ref().unwrap().can_redo);
        assert!(!data.path().join("editor-drafts").exists());
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
        let destination = data.path().join("exports/edited.png");
        {
            let mut view = editor.view.lock().unwrap();
            view.destination = destination.to_string_lossy().into_owned();
            view.save_new(&editor.tx);
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
            assert!(
                view.output_notice
                    .as_ref()
                    .unwrap()
                    .contains("Saved copy to")
            );
            view.save_new(&editor.tx); // Same name must fail, not silently overwrite.
        }
        receive(&editor, &ctx);
        {
            let view = editor.view.lock().unwrap();
            assert!(view.error.is_some() && view.unsaved() && !view.pending && !view.closed);
            assert!(view.output_notice.is_none());
        }
        assert!(!editor.take_history_changed());
        assert_eq!(fs::read(&destination).unwrap(), bytes);
        assert_eq!(fs::read(&original_path).unwrap(), original);
        assert_eq!(fs::read_dir(&root).unwrap().count(), 2);

        // The retained session can publish even if History becomes unavailable.
        fs::rename(&root, data.path().join("previous-history")).unwrap();
        fs::write(&root, b"blocked").unwrap();
        let recovered = data.path().join("exports/recovered.png");
        {
            let mut view = editor.view.lock().unwrap();
            view.destination = recovered.to_string_lossy().into_owned();
            view.save_new(&editor.tx);
            view.request_close(); // A pending export must complete before close handling.
        }
        receive(&editor, &ctx);
        {
            let view = editor.view.lock().unwrap();
            assert!(view.error.is_none() && view.unsaved() && !view.closed && view.close_requested);
            let notice = view.output_notice.as_ref().unwrap();
            assert!(notice.contains("recovered.png") && notice.contains("History was not updated"));
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
            assert_eq!(view.error.as_deref(), Some("Clipboard unavailable"));
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
        assert_eq!(
            view.output_notice.as_deref(),
            Some("Copied edited pixels to the clipboard.")
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
        assert!(
            editor.view.lock().unwrap().output_notice.is_none(),
            "an edit invalidates the copied confirmation"
        );
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
        let destination = data.path().join("exports/queued.png");
        editor
            .tx
            .send(Job::SaveNew {
                destination: destination.clone(),
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
