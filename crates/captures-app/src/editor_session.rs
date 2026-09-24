//! Worker-owned screenshot editing. Hosts serialize calls on one worker and
//! deliver immutable snapshots/pixel buffers to their UI. No window, clipboard,
//! network, or installed-app data is accessed here.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{Cursor, Read},
    path::{Path, PathBuf},
    sync::Arc,
};

use captures_history::{ArtifactKind, HistoryEntry, editor_draft};
use captures_image::text::TextRenderer;
use image::{ImageFormat, ImageReader, RgbaImage};
use serde::{Deserialize, Serialize};

use crate::{
    editor::{
        AnnotationStylePatch, ClosedShapeCreate, Document, DocumentHistory, DropShadowStyle,
        DropShadowStylePatch, Element, ElementBase, FreehandPathCreate, ImageElement, LayerEdit,
        OpenShapeCreate, OptionalNullable, Point, Rect, TextElement, image_bounds,
    },
    editor_image_background::{BrushMode, paint_stroke},
    editor_render::{
        MAX_RENDER_DIMENSION, MAX_RENDER_PIXELS, prepare_text_edit, render, render_with_text,
    },
};

const ASSET_PREFIX: &str = "draft-asset:";
const MAX_METADATA_BYTES: u64 = 8 * 1024 * 1024;

pub use captures_image::{ExportFormat, ExportOptions, ExportQuality, ExportSize, PngOptions};

#[derive(Debug, Deserialize)]
pub struct OpenRequest {
    pub history_root: PathBuf,
    pub drafts_root: PathBuf,
    pub artifact_id: String,
}

/// One decoded image supplied by a native host. Hosts own file picking and
/// decoding; pixels never cross the JSON command boundary.
pub struct ImportImage {
    pub pixels: RgbaImage,
    pub name: String,
    /// Shipping falls back to the front-most visible image when this layer is
    /// missing, hidden, or not an image.
    pub selected_id: Option<String>,
    /// A document-space drag sample. Without one, shipping places the image
    /// below the selected/front-most visible image (or the canvas).
    pub point: Option<Point>,
}

/// Text placed with either the caller's explicit family or a shared named style.
/// Native hosts own composition/cancellation and submit accepted content, never
/// font bytes or an entire replacement document.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TextCreate {
    pub point: Point,
    pub text: String,
    pub font_size: f64,
    pub font_family: String,
    pub color: String,
    #[serde(default)]
    pub style_preset: Option<String>,
}

/// The text layer edited by a transient inline-input transaction.
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TextInputTarget {
    Existing { id: String },
    New { create: TextCreate },
}

/// Text property edits. Omitted fields preserve authored/unknown data;
/// a null background removes the plate. Paint-only changes do not refit text.
#[derive(Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TextPatch {
    pub text: Option<String>,
    pub font_size: Option<f64>,
    pub font_family: Option<String>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub align: Option<String>,
    pub color: Option<String>,
    #[serde(default)]
    pub background: OptionalNullable<String>,
    pub rounded_background: Option<bool>,
    pub drop_shadow: Option<bool>,
    pub drop_shadow_style: Option<DropShadowStylePatch>,
    pub outlined: Option<bool>,
}

impl TextPatch {
    fn apply(self, element: &mut TextElement) -> Result<bool, String> {
        if self
            .font_size
            .is_some_and(|size| !(8. ..=512.).contains(&size))
        {
            return Err("Text property size must be between 8 and 512.".into());
        }
        let refit = self.text.is_some()
            || self.font_size.is_some()
            || self.font_family.is_some()
            || self.bold.is_some()
            || self.italic.is_some();
        if let Some(text) = self.text {
            element.text = text;
        }
        if let Some(size) = self.font_size {
            element.font_size = size;
        }
        if let Some(family) = self.font_family {
            if family != "rounded" {
                element.rounded_background = false;
            }
            element.font_family = family;
        }
        if let Some(bold) = self.bold {
            element.bold = bold;
        }
        if let Some(italic) = self.italic {
            element.italic = italic;
        }
        if let Some(align) = self.align {
            element.align = align;
        }
        if let Some(color) = self.color {
            element.color = color;
        }
        match self.background {
            OptionalNullable::Missing => {}
            OptionalNullable::Null => element.background = None,
            OptionalNullable::Value(color) => element.background = Some(color),
        }
        if let Some(rounded) = self.rounded_background {
            element.rounded_background = rounded;
        }
        if self.drop_shadow.is_some() || self.drop_shadow_style.is_some() {
            let mut style = crate::editor_text::shadow_style(element, element.font_size);
            AnnotationStylePatch {
                drop_shadow: self.drop_shadow,
                drop_shadow_style: self.drop_shadow_style,
                ..Default::default()
            }
            .apply(&mut style, false)?;
            element.drop_shadow = style.drop_shadow;
            element.drop_shadow_style = style.drop_shadow_style;
        }
        if let Some(outlined) = self.outlined {
            element.outlined = outlined;
        }
        Ok(refit)
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum Request {
    Snapshot,
    Crop {
        rect: Rect,
    },
    ResizeCanvas {
        width: f64,
        height: f64,
    },
    TrimCanvas,
    SetBackground {
        color: Option<String>,
    },
    RemoveImageBackground {
        point: Point,
        tolerance: f64,
        contiguous: bool,
    },
    PaintImageBackground {
        points: Vec<Point>,
        size: f64,
        softness: f64,
        mode: BrushMode,
    },
    CreateClosedShape {
        #[serde(flatten)]
        create: ClosedShapeCreate,
    },
    CreateOpenShape {
        #[serde(flatten)]
        create: OpenShapeCreate,
    },
    CreateFreehandPath {
        #[serde(flatten)]
        create: FreehandPathCreate,
    },
    CreateText {
        #[serde(flatten)]
        create: TextCreate,
    },
    EditText {
        id: String,
        patch: TextPatch,
    },
    BeginTextInput {
        input_id: String,
        target: TextInputTarget,
    },
    UpdateTextInput {
        input_id: String,
        text: String,
    },
    FinishTextInput {
        input_id: String,
        commit: bool,
    },
    Layer {
        id: String,
        edit: LayerEdit,
    },
    CopyLayer {
        id: String,
    },
    PasteLayer {
        new_id: String,
        after_id: Option<String>,
    },
    MergeDown {
        id: String,
        new_id: String,
    },
    MergeVisible {
        new_id: String,
    },
    Flatten {
        new_id: String,
    },
    /// Asset references must already belong to this session. New image import
    /// will have a separate byte/file boundary, never pixels in command JSON.
    Commit {
        document: Document,
    },
    Undo,
    Redo,
    SaveDraft {
        updated_at_ms: u64,
    },
    DiscardDraft,
}

/// Resolved UI values, separate from the authored document. Reading legacy
/// defaults must not materialize fields in drafts or change undo/redo state.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnnotationControls<'a> {
    pub closed: bool,
    pub color: &'a str,
    pub fill: Option<&'a str>,
    pub stroke_width: f64,
    pub stroke_enabled: bool,
    pub drop_shadow: bool,
    pub drop_shadow_style: DropShadowStyle,
}

#[derive(Debug, Serialize)]
pub struct Snapshot<'a> {
    pub artifact_id: &'a str,
    pub original_export_path: Option<&'a Path>,
    /// Shipping's initial placement size, pinned to the capture rather than the
    /// editable canvas. Hosts adopt it once and retain subsequent user choices.
    pub initial_text_size: f64,
    /// Present while a host-owned inline input is active. Hosts must use this
    /// lifecycle marker rather than committed history or unsaved flags.
    pub active_text_input: Option<ActiveTextInput<'a>>,
    pub document: &'a Document,
    /// Only these pinned session fonts are available; host defaults never replace
    /// a reopened draft's exact files or expand its font set implicitly.
    pub font_families: Option<&'a BTreeMap<String, String>>,
    pub text_style_presets: Vec<crate::editor_text::TextStylePreset>,
    pub annotation_controls: BTreeMap<&'a str, AnnotationControls<'a>>,
    /// Resolved display defaults; reading them never authors custom shadow data.
    pub text_shadow_styles: BTreeMap<&'a str, DropShadowStyle>,
    pub selection_outlines: BTreeMap<&'a str, [Point; 4]>,
    /// Capabilities for committed document history. Transient text previews do
    /// not add or clear undo/redo entries.
    pub can_undo: bool,
    pub can_redo: bool,
    pub can_paste_layer: bool,
    pub merge_down_ids: Vec<&'a str>,
    pub can_merge_visible: bool,
    pub can_flatten: bool,
    /// Committed changes since the last successful draft save (or open), not
    /// transient text previews and not changes since capture.
    pub unsaved_changes: bool,
    pub has_draft: bool,
}

#[derive(Debug, Serialize)]
pub struct ActiveTextInput<'a> {
    pub input_id: &'a str,
    pub layer_id: &'a str,
    pub is_new: bool,
}

pub struct EditorSession {
    artifact_id: String,
    history_root: PathBuf,
    drafts_root: PathBuf,
    original_export_path: Option<PathBuf>,
    initial_text_size: f64,
    history: DocumentHistory,
    original_path: PathBuf,
    persisted: Document,
    assets: BTreeMap<String, Arc<RgbaImage>>,
    fonts: Option<SessionFonts>,
    pixels: Arc<RgbaImage>,
    has_draft: bool,
    layer_clipboard: Option<Element>,
    layer_paste_count: u32,
    text_input: Option<TransientTextInput>,
}

struct TransientTextInput {
    input_id: String,
    layer_id: String,
    is_new: bool,
    document: Document,
    original_pixels: Arc<RgbaImage>,
}

struct SessionFonts {
    assets: editor_draft::FontAssets,
    renderer: TextRenderer,
}

fn render_frame(
    document: &Document,
    assets: &BTreeMap<String, Arc<RgbaImage>>,
    fonts: Option<&mut SessionFonts>,
) -> Result<RgbaImage, String> {
    match fonts {
        Some(fonts) => render_with_text(
            document,
            assets,
            &mut fonts.renderer,
            &fonts.assets.families,
        ),
        None => render(document, assets),
    }
}

impl EditorSession {
    pub fn open(request: OpenRequest) -> Result<Self, String> {
        Self::open_with_fonts(request, None)
    }

    /// Hosts supply trusted, appropriately licensed font bytes, not paths or
    /// system generic families. A draft's persisted fonts take precedence over
    /// new host defaults, including after an OS/font update. No discovery occurs.
    pub fn open_with_fonts(
        request: OpenRequest,
        mut fonts: Option<editor_draft::FontAssets>,
    ) -> Result<Self, String> {
        let directory =
            captures_history::entry_directory(&request.history_root, &request.artifact_id)
                .map_err(|error| error.to_string())?;
        let metadata = read_bounded(
            &directory.join(captures_history::HISTORY_METADATA_FILE),
            MAX_METADATA_BYTES,
        )?;
        let entry: HistoryEntry =
            serde_json::from_slice(&metadata).map_err(|error| error.to_string())?;
        if entry.id != request.artifact_id || entry.kind != ArtifactKind::Screenshot {
            return Err("Select a screenshot from History to edit.".into());
        }
        let original_export_path = entry.saved_path.as_deref().map(PathBuf::from);
        let mut remaining_pixels = MAX_RENDER_PIXELS;
        let original_path = directory.join(captures_history::HISTORY_IMAGE_FILE);
        let mut assets = BTreeMap::new();
        let loaded = editor_draft::load(&request.drafts_root, &request.artifact_id, |_, id| {
            format!("{ASSET_PREFIX}{id}")
        })
        .map_err(|error| error.to_string())?;
        let has_draft = loaded.is_some();
        let document = match loaded {
            Some(draft) => {
                fonts = draft.fonts.or(fonts);
                let document: Document =
                    serde_json::from_value(draft.document).map_err(|error| error.to_string())?;
                for source in sources(&document) {
                    let id = asset_id(source)?;
                    let image = decode_png(
                        &request
                            .drafts_root
                            .join(&request.artifact_id)
                            .join("assets")
                            .join(format!("{id}.png")),
                        &mut remaining_pixels,
                    )?;
                    assets.insert(source.to_owned(), Arc::new(image));
                }
                document
            }
            None => original_document(&original_path, &request.artifact_id, &mut assets)?,
        };
        let mut fonts = fonts
            .map(|assets| {
                assets.validate().map_err(|error| error.to_string())?;
                let renderer = TextRenderer::new(assets.files.values().cloned())?;
                Ok::<_, String>(SessionFonts { assets, renderer })
            })
            .transpose()?;
        let pixels = Arc::new(render_frame(&document, &assets, fonts.as_mut())?);
        Ok(Self {
            artifact_id: request.artifact_id,
            history_root: request.history_root,
            drafts_root: request.drafts_root,
            original_export_path,
            initial_text_size: (f64::from(entry.width.min(entry.height)) * 0.055)
                .round()
                .clamp(24., 72.),
            history: DocumentHistory::new(document.clone()),
            original_path,
            persisted: document,
            assets,
            fonts,
            pixels,
            has_draft,
            layer_clipboard: None,
            layer_paste_count: 0,
            text_input: None,
        })
    }

    fn visible_document(&self) -> &Document {
        self.text_input
            .as_ref()
            .map_or_else(|| self.history.current(), |input| &input.document)
    }

    #[must_use]
    pub fn snapshot(&self) -> Snapshot<'_> {
        let document = self.visible_document();
        let merge_down_ids = document
            .elements
            .windows(2)
            .filter(|pair| !pair[0].base().locked && !pair[1].base().locked)
            .map(|pair| pair[1].base().id.as_str())
            .collect();
        let visible_count = document
            .elements
            .iter()
            .filter(|element| element.base().visible)
            .count();
        Snapshot {
            artifact_id: &self.artifact_id,
            original_export_path: self.original_export_path.as_deref(),
            initial_text_size: self.initial_text_size,
            active_text_input: self.text_input.as_ref().map(|input| ActiveTextInput {
                input_id: &input.input_id,
                layer_id: &input.layer_id,
                is_new: input.is_new,
            }),
            document,
            font_families: self.fonts.as_ref().map(|fonts| &fonts.assets.families),
            text_style_presets: crate::editor_text::TEXT_STYLE_PRESETS
                .iter()
                .filter(|preset| {
                    self.fonts
                        .as_ref()
                        .is_some_and(|fonts| fonts.assets.families.contains_key(preset.font_family))
                })
                .copied()
                .collect(),
            text_shadow_styles: self
                .visible_document()
                .elements
                .iter()
                .filter_map(|element| match element {
                    Element::Text(text) => Some((
                        text.base.id.as_str(),
                        crate::editor_text::shadow_style(text, text.font_size)
                            .resolved_drop_shadow_style(),
                    )),
                    _ => None,
                })
                .collect(),
            selection_outlines: self
                .visible_document()
                .elements
                .iter()
                .filter_map(|element| {
                    element
                        .selection_outline()
                        .ok()
                        .map(|outline| (element.base().id.as_str(), outline))
                })
                .collect(),
            annotation_controls: self
                .visible_document()
                .elements
                .iter()
                .filter_map(|element| {
                    let (style, closed) = match element {
                        Element::Shape(shape) => (
                            &shape.style,
                            matches!(
                                shape.shape.as_str(),
                                "rectangle" | "ellipse" | "triangle" | "diamond" | "star"
                            ),
                        ),
                        Element::Path(path) => (&path.style, false),
                        _ => return None,
                    };
                    Some((
                        element.base().id.as_str(),
                        AnnotationControls {
                            closed,
                            color: &style.color,
                            fill: style.fill.as_deref(),
                            stroke_width: style.stroke_width,
                            stroke_enabled: style.has_stroke(),
                            drop_shadow: style.has_drop_shadow(),
                            drop_shadow_style: style.resolved_drop_shadow_style(),
                        },
                    ))
                })
                .collect(),
            can_undo: self.history.undo_len() > 0,
            can_redo: self.history.redo_len() > 0,
            can_paste_layer: self.layer_clipboard.is_some(),
            merge_down_ids,
            can_merge_visible: visible_count >= 2,
            can_flatten: document.elements.len() >= 2
                || (document.elements.len() == 1 && document.background.is_some()),
            unsaved_changes: self.history.current() != &self.persisted,
            has_draft: self.has_draft,
        }
    }

    /// Replace the file that was explicitly associated with this screenshot
    /// when the session opened, and update that same History artifact.
    pub fn save_original_export(
        &self,
        destination: &Path,
        options: ExportOptions,
    ) -> Result<crate::editor_output::SavedExport, String> {
        self.require_finished_text_input()?;
        let expected = self.original_export_path.as_deref().ok_or_else(|| {
            "This screenshot did not have an original saved file to replace.".to_owned()
        })?;
        if destination != expected {
            return Err(
                "The overwrite destination is not this screenshot's original saved file."
                    .to_owned(),
            );
        }
        crate::editor_output::save_original_export(
            &self.history_root,
            &self.artifact_id,
            expected,
            &self.pixels,
            options,
        )
        .map_err(|error| error.to_string())
    }

    /// Cloning this Arc does not copy pixels; an old UI frame may safely outlive
    /// subsequent edits or even the session itself.
    #[must_use]
    pub fn pixels(&self) -> Arc<RgbaImage> {
        self.pixels.clone()
    }

    /// Encode the current rendered frame on the session worker. The owned bytes
    /// outlive edits/close. This does not save a draft, change undo/redo, or write
    /// any files; hosts own file publication and clipboard operations.
    pub fn encode_export(&self, options: ExportOptions) -> Result<Vec<u8>, String> {
        self.require_finished_text_input()?;
        captures_image::encode_export(&self.pixels, options)
    }

    /// Import one decoded image as a single undoable edit. Asset ownership,
    /// document history, and rendered pixels are published atomically.
    pub fn import_image(&mut self, request: ImportImage) -> Result<String, String> {
        self.require_finished_text_input()?;
        let (width, height) = request.pixels.dimensions();
        validate_import_dimensions(width, height, retained_asset_pixels(&self.assets)?)?;
        if request
            .point
            .is_some_and(|point| !point.x.is_finite() || !point.y.is_finite())
        {
            return Err("Image drop coordinates must be finite.".into());
        }

        let layer_id = fresh_id(|id| {
            self.history
                .current()
                .elements
                .iter()
                .any(|element| element.base().id == id)
        });
        let asset_id = fresh_id(|id| self.assets.contains_key(&format!("{ASSET_PREFIX}{id}")));
        let source = format!("{ASSET_PREFIX}{asset_id}");
        let mut document = self.history.current().clone();
        let (target, placement, point) =
            import_placement(&document, request.selected_id.as_deref(), request.point);
        let bounds = position_imported_image(
            width,
            height,
            document.width,
            document.height,
            target,
            placement,
            point,
        );
        let element = Element::Image(ImageElement {
            base: ElementBase {
                id: layer_id.clone(),
                x: bounds.x,
                y: bounds.y,
                rotation: None,
                locked: false,
                visible: true,
                opacity: 100.,
                blend_mode: "source-over".into(),
            },
            source: "imported".into(),
            src: source.clone(),
            original_src: OptionalNullable::Null,
            name: request.name,
            source_artifact_id: None,
            width: bounds.width,
            height: bounds.height,
            natural_width: f64::from(width),
            natural_height: f64::from(height),
            orientation: None,
            extra: Default::default(),
        });
        if fully_outside_canvas(bounds, document.width, document.height) {
            expand_document_for_element(
                &mut document,
                element,
                if placement == ImportPlacement::Stack {
                    24.
                } else {
                    0.
                },
            );
        } else {
            document.elements.push(element);
        }

        let mut history = self.history.clone();
        history.commit(document);
        let mut assets = self.assets.clone();
        assets.insert(source, Arc::new(request.pixels));
        let pixels = render_frame(history.current(), &assets, self.fonts.as_mut())?;

        self.history = history;
        self.assets = assets;
        self.pixels = Arc::new(pixels);
        Ok(layer_id)
    }

    fn remove_image_background(
        &mut self,
        point: Point,
        tolerance: f64,
        contiguous: bool,
    ) -> Result<(), String> {
        if !point.x.is_finite() || !point.y.is_finite() || !tolerance.is_finite() {
            return Err("Background removal requires finite coordinates and tolerance.".into());
        }
        let (index, image, pixel) = self
            .history
            .current()
            .elements
            .iter()
            .enumerate()
            .rev()
            .find_map(|(index, element)| match element {
                Element::Image(image) if image.base.visible => image
                    .natural_pixel_at(point)
                    .map(|pixel| (index, image, pixel)),
                _ => None,
            })
            .ok_or("Click inside a visible image layer to sample a color.")?;
        // Locked image layers remain editable, as in the shipping wand. Other
        // layer kinds and hidden images do not block image-background picking.
        let source = self
            .assets
            .get(&image.src)
            .ok_or("The editor image asset is unavailable.")?;
        let mut edited = (**source).clone();
        let changed = crate::editor_image_background::remove_color(
            &mut edited,
            pixel,
            tolerance.round().clamp(0., 255.) as u8,
            contiguous,
        );
        if changed == 0 {
            return Err("No matching pixels were found. Try a higher tolerance.".into());
        }
        self.publish_image_background_edit(index, edited)
    }

    fn paint_image_background(
        &mut self,
        points: Vec<Point>,
        size: f64,
        softness: f64,
        mode: BrushMode,
    ) -> Result<(), String> {
        if points.is_empty()
            || points
                .iter()
                .any(|point| !point.x.is_finite() || !point.y.is_finite())
            || !size.is_finite()
            || size <= 0.
            || !softness.is_finite()
        {
            return Err(
                "Background brush requires finite sample coordinates, positive size, and finite softness."
                    .into(),
            );
        }
        let first = points[0];
        let (index, image) = self
            .history
            .current()
            .elements
            .iter()
            .enumerate()
            .rev()
            .find_map(|(index, element)| match element {
                Element::Image(image)
                    if image.base.visible && image.natural_pixel_at(first).is_some() =>
                {
                    Some((index, image))
                }
                _ => None,
            })
            .ok_or("Start the background brush inside a visible image layer.")?;
        let samples: Vec<_> = points
            .into_iter()
            .filter_map(|point| image.natural_pixel_at(point))
            .collect();
        let current = self
            .assets
            .get(&image.src)
            .ok_or("The editor image asset is unavailable.")?;
        let original = match mode {
            BrushMode::Erase => None,
            BrushMode::Restore => {
                let OptionalNullable::Value(source) = &image.original_src else {
                    return Err("Restore requires a retained original image asset.".into());
                };
                Some(
                    self.assets
                        .get(source)
                        .ok_or("The retained original image asset is unavailable.")?
                        .as_ref(),
                )
            }
        };
        let displayed_natural_width = if image.resolved_orientation().matrix().a == 0 {
            image.natural_height
        } else {
            image.natural_width
        };
        let radius = (size * displayed_natural_width / image.width.max(1.) * 0.5).max(1.);
        let hardness = 1. - softness.clamp(0., 100.) / 100.;
        let mut edited = (**current).clone();
        let changed = paint_stroke(&mut edited, original, &samples, radius, hardness, mode)?;
        if changed == 0 {
            return Ok(());
        }
        self.publish_image_background_edit(index, edited)
    }

    fn publish_image_background_edit(
        &mut self,
        index: usize,
        edited: RgbaImage,
    ) -> Result<(), String> {
        validate_import_dimensions(
            edited.width(),
            edited.height(),
            retained_asset_pixels(&self.assets)?,
        )?;
        let asset_id = fresh_id(|id| self.assets.contains_key(&format!("{ASSET_PREFIX}{id}")));
        let source = format!("{ASSET_PREFIX}{asset_id}");
        let mut document = self.history.current().clone();
        let Element::Image(image) = &mut document.elements[index] else {
            unreachable!()
        };
        if !matches!(image.original_src, OptionalNullable::Value(_)) {
            image.original_src = OptionalNullable::Value(image.src.clone());
        }
        image.src = source.clone();
        document.background = None;
        let mut assets = self.assets.clone();
        assets.insert(source, Arc::new(edited));
        let pixels = render_frame(&document, &assets, self.fonts.as_mut())?;
        // Assets, rendered frame and history change together. Failed/no-op
        // requests keep redo and the pre-edit source for a later restore brush.
        let mut history = self.history.clone();
        history.commit(document);
        self.assets = assets;
        self.history = history;
        self.pixels = Arc::new(pixels);
        Ok(())
    }

    fn require_finished_text_input(&self) -> Result<(), String> {
        if self.text_input.is_some() {
            Err("Finish or cancel the active text input before continuing.".into())
        } else {
            Ok(())
        }
    }

    fn combine_layers(&mut self, request: Request) -> Result<(), String> {
        let current = self.history.current();
        let (new_id, raster_document, insertion, name, flatten, merge_down) = match request {
            Request::MergeDown { id, new_id } => {
                let index = current
                    .elements
                    .iter()
                    .position(|element| element.base().id == id)
                    .ok_or("The selected layer no longer exists.")?;
                if index == 0
                    || current.elements[index].base().locked
                    || current.elements[index - 1].base().locked
                {
                    return Err("Merge Down requires two adjacent unlocked layers.".into());
                }
                let mut layers = current.elements[index - 1..=index].to_vec();
                for layer in &mut layers {
                    match layer {
                        Element::Image(element) => &mut element.base,
                        Element::Text(element) => &mut element.base,
                        Element::Shape(element) => &mut element.base,
                        Element::Path(element) => &mut element.base,
                    }
                    .visible = true;
                }
                let name = layers
                    .iter()
                    .find_map(|layer| match layer {
                        Element::Image(image) => Some(image.name.clone()),
                        _ => None,
                    })
                    .unwrap_or_else(|| "Merged".into());
                let mut raster = current.clone();
                raster.background = None;
                raster.elements = layers;
                (new_id, raster, index - 1, name, false, true)
            }
            Request::MergeVisible { new_id } => {
                let visible: Vec<_> = current
                    .elements
                    .iter()
                    .filter(|element| element.base().visible)
                    .cloned()
                    .collect();
                if visible.len() < 2 {
                    return Err("Merge Visible requires at least two visible layers.".into());
                }
                let insertion = current
                    .elements
                    .iter()
                    .position(|element| element.base().visible)
                    .expect("two visible layers have a first index");
                let mut raster = current.clone();
                raster.background = None;
                raster.elements = visible;
                (new_id, raster, insertion, "Merged".into(), false, false)
            }
            Request::Flatten { new_id } => {
                if current.elements.len() < 2
                    && !(current.elements.len() == 1 && current.background.is_some())
                {
                    return Err("Flatten requires layers or a background to combine.".into());
                }
                (new_id, current.clone(), 0, "Flattened".into(), true, false)
            }
            _ => unreachable!(),
        };
        if new_id.is_empty()
            || current
                .elements
                .iter()
                .any(|element| element.base().id == new_id)
        {
            return Err("A combined layer needs a new nonempty identifier.".into());
        }

        // Combining keeps the canvas dimensions. Bound the newly retained
        // asset before allocating a second full-canvas render.
        validate_import_dimensions(
            self.pixels.width(),
            self.pixels.height(),
            retained_asset_pixels(&self.assets)?,
        )?;
        let combined = render_frame(&raster_document, &self.assets, self.fonts.as_mut())?;
        let asset_id = fresh_id(|id| self.assets.contains_key(&format!("{ASSET_PREFIX}{id}")));
        let source = format!("{ASSET_PREFIX}{asset_id}");
        let image = Element::Image(ImageElement {
            base: ElementBase {
                id: new_id,
                x: 0.,
                y: 0.,
                rotation: None,
                locked: flatten,
                visible: true,
                opacity: 100.,
                blend_mode: "source-over".into(),
            },
            source: if flatten { "background" } else { "imported" }.into(),
            src: source.clone(),
            original_src: OptionalNullable::Null,
            name,
            source_artifact_id: None,
            width: f64::from(combined.width()),
            height: f64::from(combined.height()),
            natural_width: f64::from(combined.width()),
            natural_height: f64::from(combined.height()),
            orientation: None,
            extra: Default::default(),
        });
        let mut document = current.clone();
        if flatten {
            document.background = None;
            document.elements = vec![image];
        } else if merge_down {
            document.elements.splice(insertion..insertion + 2, [image]);
        } else {
            document.elements.retain(|element| !element.base().visible);
            document.elements.insert(insertion, image);
        }
        let mut assets = self.assets.clone();
        assets.insert(source, Arc::new(combined));
        let pixels = render_frame(&document, &assets, self.fonts.as_mut())?;
        let mut history = self.history.clone();
        history.commit(document);
        self.assets = assets;
        self.history = history;
        self.pixels = Arc::new(pixels);
        Ok(())
    }

    fn begin_text_input(
        &mut self,
        input_id: String,
        target: TextInputTarget,
    ) -> Result<(), String> {
        if input_id.is_empty() {
            return Err("Text input requires a nonempty token.".into());
        }
        if self.text_input.is_some() {
            return Err("Finish or cancel the active text input before beginning another.".into());
        }

        let original_pixels = self.pixels.clone();
        let mut document = self.history.current().clone();
        let (layer_id, is_new, pixels) = match target {
            TextInputTarget::Existing { id } => {
                let element = document
                    .elements
                    .iter()
                    .find(|element| element.base().id == id)
                    .ok_or("The selected layer no longer exists.")?;
                let Element::Text(text) = element else {
                    return Err("The selected layer is not text.".into());
                };
                if !text.base.visible || text.base.locked {
                    return Err("Inline text input requires a visible, unlocked text layer.".into());
                }
                (id, false, original_pixels.clone())
            }
            TextInputTarget::New { create } => {
                let id = prepare_text_create(&mut document, create, self.fonts.as_mut())?;
                let pixels = Arc::new(render_frame(&document, &self.assets, self.fonts.as_mut())?);
                (id, true, pixels)
            }
        };
        self.text_input = Some(TransientTextInput {
            input_id,
            layer_id,
            is_new,
            document,
            original_pixels,
        });
        self.pixels = pixels;
        Ok(())
    }

    fn update_text_input(&mut self, input_id: &str, text: String) -> Result<(), String> {
        let input = self
            .text_input
            .as_ref()
            .filter(|input| input.input_id == input_id)
            .ok_or("The text input token is stale.")?;
        let layer_id = input.layer_id.clone();
        let mut document = input.document.clone();
        let element = document
            .elements
            .iter_mut()
            .find(|element| element.base().id == layer_id)
            .expect("active text input layer remains in its private document");
        let Element::Text(element) = element else {
            unreachable!("active text input layer remains text")
        };
        element.text = text;
        let fonts = self
            .fonts
            .as_mut()
            .ok_or("Text requires explicit font bytes.")?;
        *element = prepare_text_edit(element, true, &mut fonts.renderer, &fonts.assets.families)?;
        let pixels = render_frame(&document, &self.assets, self.fonts.as_mut())?;

        self.text_input
            .as_mut()
            .expect("validated active text input")
            .document = document;
        self.pixels = Arc::new(pixels);
        Ok(())
    }

    fn finish_text_input(&mut self, input_id: &str, commit: bool) -> Result<(), String> {
        let input = self
            .text_input
            .as_ref()
            .filter(|input| input.input_id == input_id)
            .ok_or("The text input token is stale.")?;
        if !commit {
            self.pixels = input.original_pixels.clone();
            self.text_input = None;
            return Ok(());
        }

        let mut document = input.document.clone();
        let index = document
            .elements
            .iter()
            .position(|element| element.base().id == input.layer_id)
            .expect("active text input layer remains in its private document");
        let Element::Text(text) = &document.elements[index] else {
            unreachable!("active text input layer remains text")
        };
        if crate::editor_text::is_blank(&text.text) {
            document.elements.remove(index);
        }

        let pixels = if &document == self.history.current() {
            input.original_pixels.clone()
        } else {
            Arc::new(render_frame(&document, &self.assets, self.fonts.as_mut())?)
        };
        let mut history = self.history.clone();
        history.commit(document);
        self.history = history;
        self.pixels = pixels;
        self.text_input = None;
        Ok(())
    }

    pub fn execute(&mut self, request: Request) -> Result<(), String> {
        if self.text_input.is_some() {
            match request {
                Request::Snapshot => return Ok(()),
                Request::UpdateTextInput { input_id, text } => {
                    return self.update_text_input(&input_id, text);
                }
                Request::FinishTextInput { input_id, commit } => {
                    return self.finish_text_input(&input_id, commit);
                }
                Request::BeginTextInput { .. } => {
                    return Err(
                        "Finish or cancel the active text input before beginning another.".into(),
                    );
                }
                _ => return self.require_finished_text_input(),
            }
        }
        let request = match request {
            Request::Snapshot => return Ok(()),
            Request::BeginTextInput { input_id, target } => {
                return self.begin_text_input(input_id, target);
            }
            Request::UpdateTextInput { .. } | Request::FinishTextInput { .. } => {
                return Err("The text input token is stale.".into());
            }
            Request::SaveDraft { updated_at_ms } => return self.save_draft(updated_at_ms),
            Request::DiscardDraft => return self.discard_draft(),
            Request::CopyLayer { id } => {
                let element = self
                    .history
                    .current()
                    .elements
                    .iter()
                    .find(|element| element.base().id == id)
                    .ok_or("The selected layer no longer exists.")?;
                self.layer_clipboard = Some(element.clone());
                self.layer_paste_count = 0;
                return Ok(());
            }
            Request::RemoveImageBackground {
                point,
                tolerance,
                contiguous,
            } => {
                return self.remove_image_background(point, tolerance, contiguous);
            }
            Request::PaintImageBackground {
                points,
                size,
                softness,
                mode,
            } => return self.paint_image_background(points, size, softness, mode),
            request @ (Request::MergeDown { .. }
            | Request::MergeVisible { .. }
            | Request::Flatten { .. }) => return self.combine_layers(request),
            edit => edit,
        };
        let is_paste = matches!(&request, Request::PasteLayer { .. });
        let mut next = self.history.clone();
        match request {
            Request::Snapshot
            | Request::BeginTextInput { .. }
            | Request::UpdateTextInput { .. }
            | Request::FinishTextInput { .. }
            | Request::SaveDraft { .. }
            | Request::DiscardDraft
            | Request::RemoveImageBackground { .. }
            | Request::PaintImageBackground { .. } => unreachable!(),
            Request::Undo => {
                next.undo();
            }
            Request::Redo => {
                next.redo();
            }
            Request::Commit { document } => {
                next.commit(document);
            }
            Request::SetBackground { color } => {
                let mut document = next.current().clone();
                document.background = color;
                next.commit(document);
            }
            Request::CreateClosedShape { create } => {
                let mut document = next.current().clone();
                document.create_closed_shape(create)?;
                next.commit(document);
            }
            Request::CreateOpenShape { create } => {
                let mut document = next.current().clone();
                document.create_open_shape(create)?;
                next.commit(document);
            }
            Request::CreateFreehandPath { create } => {
                let mut document = next.current().clone();
                document.create_freehand_path(create)?;
                next.commit(document);
            }
            Request::CreateText { create } => {
                let mut document = next.current().clone();
                prepare_text_create(&mut document, create, self.fonts.as_mut())?;
                next.commit(document);
            }
            Request::EditText { id, patch } => {
                let mut document = next.current().clone();
                let element = document
                    .elements
                    .iter_mut()
                    .find(|e| e.base().id == id)
                    .ok_or("The selected layer no longer exists.")?;
                let Element::Text(element) = element else {
                    return Err("The selected layer is not text.".into());
                };
                // Shipping property edits apply even to hidden/locked layers.
                let refit = patch.apply(element)?;
                let fonts = self
                    .fonts
                    .as_mut()
                    .ok_or("Text requires explicit font bytes.")?;
                *element =
                    prepare_text_edit(element, refit, &mut fonts.renderer, &fonts.assets.families)?;
                next.commit(document);
            }
            Request::Layer { id, edit } => {
                let mut document = next.current().clone();
                document.edit_layer(&id, edit)?;
                next.commit(document);
            }
            Request::PasteLayer { new_id, after_id } => {
                let clipboard = self
                    .layer_clipboard
                    .as_ref()
                    .ok_or("No copied layer is available.")?;
                let mut document = next.current().clone();
                if new_id.is_empty()
                    || document
                        .elements
                        .iter()
                        .any(|element| element.base().id == new_id)
                {
                    return Err("A pasted layer needs a new nonempty identifier.".into());
                }
                let offset = 24. * f64::from(self.layer_paste_count + 1);
                let pasted = clipboard.copied_layer(new_id, offset)?;
                let insertion = after_id
                    .as_deref()
                    .and_then(|id| {
                        document
                            .elements
                            .iter()
                            .position(|element| element.base().id == id)
                    })
                    .map_or(document.elements.len(), |index| index + 1);
                document.elements.insert(insertion, pasted);
                next.commit(document);
            }
            Request::CopyLayer { .. } => unreachable!(),
            Request::MergeDown { .. } | Request::MergeVisible { .. } | Request::Flatten { .. } => {
                unreachable!()
            }
            Request::Crop { rect } => {
                if ![rect.x, rect.y, rect.width, rect.height]
                    .into_iter()
                    .all(f64::is_finite)
                    || rect.width <= 0.
                    || rect.height <= 0.
                {
                    return Err("Crop geometry must be finite with positive dimensions.".into());
                }
                let mut document = next.current().clone();
                document.crop(rect);
                next.commit(document);
            }
            Request::ResizeCanvas { width, height } => {
                if !width.is_finite() || !height.is_finite() || width <= 0. || height <= 0. {
                    return Err("Canvas dimensions must be finite and positive.".into());
                }
                let mut document = next.current().clone();
                document.resize_canvas(width, height);
                next.commit(document);
            }
            Request::TrimCanvas => {
                let mut document = next.current().clone();
                document.trim_to_content()?;
                next.commit(document);
            }
        }
        if next.current() == self.history.current() {
            return Ok(());
        }
        // Check even hidden/original image references: a later save or unhide
        // must not turn a successful edit into a missing-asset draft.
        for source in sources(next.current()) {
            if !self.assets.contains_key(source) {
                return Err(format!("The editor does not own image asset {source}."));
            }
        }
        let pixels = render_frame(next.current(), &self.assets, self.fonts.as_mut())?;
        // Rendering/validation failure leaves both the undo stacks and frame
        // unchanged. Hosts never receive a half-applied edit.
        self.history = next;
        self.pixels = Arc::new(pixels);
        if is_paste {
            self.layer_paste_count += 1;
        }
        Ok(())
    }

    fn save_draft(&mut self, updated_at_ms: u64) -> Result<(), String> {
        let document = self.history.current();
        let mut total = 0;
        let mut assets = Vec::new();
        for source in sources(document) {
            let png = captures_history::encode_png(&self.assets[source])
                .map_err(|error| error.to_string())?;
            total += png.len();
            if total > 80 * 1024 * 1024 {
                return Err("Unsaved edits are too large to keep as a draft.".into());
            }
            assets.push(editor_draft::AssetInput {
                id: asset_id(source)?.into(),
                png: Some(png),
            });
        }
        editor_draft::save_with_fonts(
            &self.drafts_root,
            editor_draft::SaveRequest {
                artifact_id: self.artifact_id.clone(),
                document: serde_json::to_value(document).map_err(|error| error.to_string())?,
                assets,
                updated_at_ms,
            },
            self.fonts
                .as_ref()
                .filter(|_| {
                    document
                        .elements
                        .iter()
                        .any(|element| matches!(element, Element::Text(_)))
                })
                .map(|fonts| &fonts.assets),
        )
        .map_err(|error| error.to_string())?;
        self.persisted = document.clone();
        self.has_draft = true;
        Ok(())
    }

    fn discard_draft(&mut self) -> Result<(), String> {
        // Do not retain a second full-size original alongside a restored draft.
        // Prepare the replacement before deleting anything; a missing original
        // must not destroy the only surviving draft.
        let mut assets = BTreeMap::new();
        let original = original_document(&self.original_path, &self.artifact_id, &mut assets)?;
        let pixels = render_frame(&original, &assets, self.fonts.as_mut())?;
        editor_draft::discard(&self.drafts_root, &self.artifact_id)
            .map_err(|error| error.to_string())?;
        self.history = DocumentHistory::new(original.clone());
        self.persisted = original;
        self.assets = assets;
        self.pixels = Arc::new(pixels);
        self.has_draft = false;
        self.layer_clipboard = None;
        self.layer_paste_count = 0;
        Ok(())
    }
}

fn prepare_text_create(
    document: &mut Document,
    create: TextCreate,
    fonts: Option<&mut SessionFonts>,
) -> Result<String, String> {
    if !(8. ..=512.).contains(&create.font_size) {
        return Err("Text property size must be between 8 and 512.".into());
    }
    let preset = create
        .style_preset
        .as_deref()
        .map(|id| {
            crate::editor_text::TEXT_STYLE_PRESETS
                .iter()
                .find(|preset| preset.id == id)
                .copied()
                .ok_or_else(|| format!("Unknown text style preset: {id}"))
        })
        .transpose()?;
    let font_family = preset
        .map(|preset| preset.font_family)
        .unwrap_or(&create.font_family);
    let fonts = fonts.ok_or("Text requires explicit font bytes.")?;
    if preset.is_some() && !fonts.assets.families.contains_key(font_family) {
        return Err(format!(
            "Text style requires unavailable font family: {font_family}"
        ));
    }
    let id = fresh_id(|id| {
        document
            .elements
            .iter()
            .any(|element| element.base().id == id)
    });
    let width = (create.font_size * 8.).round();
    let centered =
        preset.is_some_and(|preset| matches!(preset.id, "box" | "mono-box" | "rounded-box"));
    let element = TextElement {
        base: ElementBase {
            id: id.clone(),
            x: create.point.x - if centered { width / 2. } else { 0. },
            y: create.point.y,
            rotation: None,
            locked: false,
            visible: true,
            opacity: 100.,
            blend_mode: "source-over".into(),
        },
        text: create.text,
        font_size: create.font_size,
        width,
        auto_width: Some(true),
        font_family: font_family.into(),
        bold: false,
        italic: false,
        align: if centered { "center" } else { "left" }.into(),
        color: create.color,
        background: preset.and_then(|preset| preset.background.map(str::to_owned)),
        outlined: preset.is_some_and(|preset| preset.outlined),
        rounded_background: preset.is_some_and(|preset| preset.rounded_background),
        drop_shadow: None,
        drop_shadow_style: None,
        extra: Default::default(),
    };
    let element = prepare_text_edit(&element, true, &mut fonts.renderer, &fonts.assets.families)?;
    document.elements.push(Element::Text(element));
    Ok(id)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ImportPlacement {
    Top,
    Right,
    Bottom,
    Left,
    Stack,
}

fn fresh_id(mut exists: impl FnMut(&str) -> bool) -> String {
    loop {
        let id = uuid::Uuid::new_v4().to_string();
        if !exists(&id) {
            return id;
        }
    }
}

fn retained_asset_pixels(assets: &BTreeMap<String, Arc<RgbaImage>>) -> Result<u64, String> {
    assets.values().try_fold(0_u64, |total, image| {
        total
            .checked_add(u64::from(image.width()) * u64::from(image.height()))
            .ok_or_else(|| "Editor images exceed the total decoded-pixel limit.".into())
    })
}

fn validate_import_dimensions(width: u32, height: u32, retained: u64) -> Result<(), String> {
    let pixels = u64::from(width) * u64::from(height);
    if width == 0
        || height == 0
        || width > MAX_RENDER_DIMENSION
        || height > MAX_RENDER_DIMENSION
        || pixels > MAX_RENDER_PIXELS.saturating_sub(retained)
    {
        return Err("Editor images exceed the dimension or total decoded-pixel limit.".into());
    }
    Ok(())
}

fn import_placement(
    document: &Document,
    selected_id: Option<&str>,
    point: Option<Point>,
) -> (Rect, ImportPlacement, Option<Point>) {
    let target = resolve_import_target(document, selected_id, point);
    match point {
        Some(point) => (target, placement_at_point(point, target), Some(point)),
        None => (
            target,
            ImportPlacement::Bottom,
            Some(Point {
                x: target.x + target.width / 2.,
                y: target.y + target.height,
            }),
        ),
    }
}

fn resolve_import_target(
    document: &Document,
    selected_id: Option<&str>,
    point: Option<Point>,
) -> Rect {
    let canvas = Rect {
        x: 0.,
        y: 0.,
        width: document.width,
        height: document.height,
    };
    let visible_images = || {
        document.elements.iter().filter_map(|element| {
            let Element::Image(image) = element else {
                return None;
            };
            image.base.visible.then_some(image)
        })
    };
    if let Some(point) = point {
        if let Some(bounds) = visible_images().rev().find_map(|image| {
            let bounds = image_bounds(image);
            (point.x >= bounds.x
                && point.x <= bounds.x + bounds.width
                && point.y >= bounds.y
                && point.y <= bounds.y + bounds.height)
                .then_some(bounds)
        }) {
            return bounds;
        }
        return visible_images()
            .rev()
            .map(image_bounds)
            .min_by(|left, right| {
                distance_to_rect(point, *left).total_cmp(&distance_to_rect(point, *right))
            })
            .unwrap_or(canvas);
    }
    if let Some(selected_id) = selected_id
        && let Some(bounds) = visible_images()
            .find(|image| image.base.id == selected_id)
            .map(image_bounds)
    {
        return bounds;
    }
    visible_images().next_back().map_or(canvas, image_bounds)
}

fn distance_to_rect(point: Point, rect: Rect) -> f64 {
    let delta_x = if point.x < rect.x {
        rect.x - point.x
    } else if point.x > rect.x + rect.width {
        point.x - (rect.x + rect.width)
    } else {
        0.
    };
    let delta_y = if point.y < rect.y {
        rect.y - point.y
    } else if point.y > rect.y + rect.height {
        point.y - (rect.y + rect.height)
    } else {
        0.
    };
    delta_x.hypot(delta_y)
}

fn placement_at_point(point: Point, target: Rect) -> ImportPlacement {
    let relative_x = point.x - target.x;
    let relative_y = point.y - target.y;
    let inside = relative_x >= 0.
        && relative_y >= 0.
        && relative_x <= target.width
        && relative_y <= target.height;
    if inside && target.width > 0. && target.height > 0. {
        let edge_band_x = target.width * 0.22;
        let edge_band_y = target.height * 0.22;
        if edge_band_x * 2. < target.width
            && edge_band_y * 2. < target.height
            && relative_x >= edge_band_x
            && relative_x <= target.width - edge_band_x
            && relative_y >= edge_band_y
            && relative_y <= target.height - edge_band_y
        {
            return ImportPlacement::Stack;
        }
    }
    [
        (ImportPlacement::Top, (point.y - target.y).abs()),
        (
            ImportPlacement::Right,
            (point.x - (target.x + target.width)).abs(),
        ),
        (
            ImportPlacement::Bottom,
            (point.y - (target.y + target.height)).abs(),
        ),
        (ImportPlacement::Left, (point.x - target.x).abs()),
    ]
    .into_iter()
    .min_by(|left, right| left.1.total_cmp(&right.1))
    .expect("four image edges")
    .0
}

fn position_imported_image(
    natural_width: u32,
    natural_height: u32,
    document_width: f64,
    document_height: f64,
    target: Rect,
    placement: ImportPlacement,
    point: Option<Point>,
) -> Rect {
    let natural_width = f64::from(natural_width).max(1.);
    let natural_height = f64::from(natural_height).max(1.);
    let scale = if placement == ImportPlacement::Stack {
        ((document_width * 0.65).max(160.) / natural_width)
            .min((document_height * 0.65).max(120.) / natural_height)
            .min(1.)
    } else {
        1.
    };
    let width = js_round(natural_width * scale).max(1.);
    let height = js_round(natural_height * scale).max(1.);
    let center = point.unwrap_or(Point {
        x: target.x + target.width / 2.,
        y: target.y + target.height / 2.,
    });
    let mut rect = Rect {
        x: js_round(center.x - width / 2.).max(0.),
        y: js_round(center.y - height / 2.).max(0.),
        width,
        height,
    };
    match placement {
        ImportPlacement::Stack => {
            rect.x = js_round(center.x - width / 2.);
            rect.y = js_round(center.y - height / 2.);
        }
        ImportPlacement::Top => {
            rect.x = js_round(target.x + (target.width - width) / 2.);
            rect.y = js_round(target.y - height);
        }
        ImportPlacement::Right => {
            rect.x = js_round(target.x + target.width);
            rect.y = js_round(target.y + (target.height - height) / 2.);
        }
        ImportPlacement::Left => {
            rect.x = js_round(target.x - width);
            rect.y = js_round(target.y + (target.height - height) / 2.);
        }
        ImportPlacement::Bottom => {
            rect.x = js_round(target.x + (target.width - width) / 2.);
            rect.y = js_round(target.y + target.height);
        }
    }
    rect
}

fn js_round(value: f64) -> f64 {
    let lower = value.floor();
    if value - lower < 0.5 {
        lower
    } else {
        lower + 1.
    }
}

fn fully_outside_canvas(bounds: Rect, width: f64, height: f64) -> bool {
    const EPSILON: f64 = 0.5;
    !(bounds.x + bounds.width > EPSILON
        && width > bounds.x + EPSILON
        && bounds.y + bounds.height > EPSILON
        && height > bounds.y + EPSILON)
}

fn expand_document_for_element(document: &mut Document, mut element: Element, padding: f64) {
    let Element::Image(image) = &element else {
        unreachable!("image import creates an image element")
    };
    let bounds = image_bounds(image);
    let shift_x = (-bounds.x).ceil().max(0.);
    let shift_y = (-bounds.y).ceil().max(0.);
    element.translate(shift_x, shift_y);
    let Element::Image(shifted) = &element else {
        unreachable!("translated import remains an image")
    };
    let shifted_bounds = image_bounds(shifted);
    document.width =
        (document.width + shift_x).max((shifted_bounds.x + shifted_bounds.width + padding).ceil());
    document.height = (document.height + shift_y)
        .max((shifted_bounds.y + shifted_bounds.height + padding).ceil());
    document.translate(shift_x, shift_y);
    document.elements.push(element);
}

fn original_document(
    path: &Path,
    artifact_id: &str,
    assets: &mut BTreeMap<String, Arc<RgbaImage>>,
) -> Result<Document, String> {
    let mut remaining_pixels = MAX_RENDER_PIXELS;
    let image = decode_png(path, &mut remaining_pixels)?;
    let source = format!("{ASSET_PREFIX}{}", uuid::Uuid::new_v4());
    let document = Document::new_capture(
        &source,
        f64::from(image.width()),
        f64::from(image.height()),
        Some(artifact_id.into()),
    );
    assets.insert(source, Arc::new(image));
    Ok(document)
}

fn sources(document: &Document) -> BTreeSet<&str> {
    let mut sources = BTreeSet::new();
    for element in &document.elements {
        if let Element::Image(image) = element {
            sources.insert(image.src.as_str());
            if let OptionalNullable::Value(source) = &image.original_src {
                sources.insert(source.as_str());
            }
        }
    }
    sources
}

fn asset_id(source: &str) -> Result<&str, String> {
    source
        .strip_prefix(ASSET_PREFIX)
        .filter(|id| {
            !id.is_empty()
                && id.len() <= 80
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
        })
        .ok_or_else(|| "Draft images must refer to owned draft assets.".into())
}

fn read_bounded(path: &Path, maximum: u64) -> Result<Vec<u8>, String> {
    let file = File::open(path).map_err(|error| error.to_string())?;
    if file.metadata().map_err(|error| error.to_string())?.len() > maximum {
        return Err("Editor input exceeds its size limit.".into());
    }
    let mut bytes = Vec::new();
    file.take(maximum + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > maximum {
        return Err("Editor input exceeds its size limit.".into());
    }
    Ok(bytes)
}

fn decode_png(path: &Path, remaining_pixels: &mut u64) -> Result<RgbaImage, String> {
    let bytes = read_bounded(path, editor_draft::MAX_IMAGE_BYTES as u64)?;
    let reader = || ImageReader::with_format(Cursor::new(&bytes), ImageFormat::Png);
    let (width, height) = reader()
        .into_dimensions()
        .map_err(|error| error.to_string())?;
    let pixels = u64::from(width) * u64::from(height);
    if width == 0
        || height == 0
        || width > MAX_RENDER_DIMENSION
        || height > MAX_RENDER_DIMENSION
        || pixels > *remaining_pixels
    {
        return Err("Editor images exceed the dimension or total decoded-pixel limit.".into());
    }
    let image = reader()
        .decode()
        .map_err(|error| error.to_string())?
        .into_rgba8();
    *remaining_pixels -= pixels;
    Ok(image)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct ImportFixture {
        name: String,
        document: Document,
        selected_id: Option<String>,
        point: Option<Point>,
        natural: FixtureSize,
        expected: FixtureExpected,
    }

    #[derive(Deserialize)]
    struct FixtureSize {
        width: u32,
        height: u32,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct FixtureExpected {
        target: Rect,
        placement: String,
        position: Rect,
        fully_outside: bool,
        output: FixtureOutput,
    }

    #[derive(Deserialize)]
    struct FixtureOutput {
        width: f64,
        height: f64,
        elements: Vec<FixtureElement>,
    }

    #[derive(Debug, Deserialize, PartialEq)]
    struct FixtureElement {
        id: String,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    }

    fn fixture_element(element: &Element) -> FixtureElement {
        let Element::Image(image) = element else {
            panic!("import fixture contains only image layers")
        };
        FixtureElement {
            id: image.base.id.clone(),
            x: image.base.x,
            y: image.base.y,
            width: image.width,
            height: image.height,
        }
    }

    fn assert_rect_close(actual: Rect, expected: Rect, context: &str) {
        for (actual, expected) in [
            (actual.x, expected.x),
            (actual.y, expected.y),
            (actual.width, expected.width),
            (actual.height, expected.height),
        ] {
            assert!(
                (actual - expected).abs() <= 1e-12,
                "{context}: expected {expected}, got {actual}"
            );
        }
    }

    #[test]
    fn placement_and_expansion_match_shipping_typescript_fixture() {
        let fixture: Vec<ImportFixture> =
            serde_json::from_str(include_str!("../tests/editor-import-golden.json")).unwrap();
        for case in fixture {
            let (target, placement, point) =
                import_placement(&case.document, case.selected_id.as_deref(), case.point);
            assert_rect_close(
                target,
                case.expected.target,
                &format!("{} target", case.name),
            );
            assert_eq!(
                placement,
                match case.expected.placement.as_str() {
                    "top" => ImportPlacement::Top,
                    "right" => ImportPlacement::Right,
                    "bottom" => ImportPlacement::Bottom,
                    "left" => ImportPlacement::Left,
                    "stack" => ImportPlacement::Stack,
                    other => panic!("unknown fixture placement {other}"),
                },
                "{} placement",
                case.name
            );
            let bounds = position_imported_image(
                case.natural.width,
                case.natural.height,
                case.document.width,
                case.document.height,
                target,
                placement,
                point,
            );
            assert_eq!(bounds, case.expected.position, "{} position", case.name);
            assert_eq!(
                fully_outside_canvas(bounds, case.document.width, case.document.height),
                case.expected.fully_outside,
                "{} outside",
                case.name
            );
            let mut output = case.document;
            let imported = Element::Image(ImageElement {
                base: ElementBase {
                    id: "new-import".into(),
                    x: bounds.x,
                    y: bounds.y,
                    rotation: None,
                    locked: false,
                    visible: true,
                    opacity: 100.,
                    blend_mode: "source-over".into(),
                },
                source: "imported".into(),
                src: "fixture:new-import".into(),
                original_src: OptionalNullable::Null,
                name: "new-import.png".into(),
                source_artifact_id: None,
                width: bounds.width,
                height: bounds.height,
                natural_width: f64::from(case.natural.width),
                natural_height: f64::from(case.natural.height),
                orientation: None,
                extra: Default::default(),
            });
            if case.expected.fully_outside {
                expand_document_for_element(
                    &mut output,
                    imported,
                    if placement == ImportPlacement::Stack {
                        24.
                    } else {
                        0.
                    },
                );
            } else {
                output.elements.push(imported);
            }
            assert_eq!(
                output.width, case.expected.output.width,
                "{} width",
                case.name
            );
            assert_eq!(
                output.height, case.expected.output.height,
                "{} height",
                case.name
            );
            assert_eq!(
                output
                    .elements
                    .iter()
                    .map(fixture_element)
                    .collect::<Vec<_>>(),
                case.expected.output.elements,
                "{} elements",
                case.name
            );
        }
    }

    #[test]
    fn retained_asset_budget_checks_the_boundary_without_allocating() {
        assert!(validate_import_dimensions(7, 3, MAX_RENDER_PIXELS - 21).is_ok());
        assert!(validate_import_dimensions(7, 3, MAX_RENDER_PIXELS - 20).is_err());
        assert!(validate_import_dimensions(0, 3, 0).is_err());
        assert!(validate_import_dimensions(MAX_RENDER_DIMENSION + 1, 1, 0).is_err());
    }

    #[test]
    fn decode_budget_counts_distinct_assets_and_only_debits_successful_reads() {
        let data = tempfile::tempdir().unwrap();
        let path = data.path().join("image.png");
        std::fs::write(
            &path,
            captures_history::encode_png(&RgbaImage::new(7, 3)).unwrap(),
        )
        .unwrap();
        let mut budget = 42;
        assert_eq!(decode_png(&path, &mut budget).unwrap().dimensions(), (7, 3));
        assert_eq!(budget, 21);
        assert!(decode_png(&path, &mut budget).is_ok());
        assert_eq!(budget, 0);
        assert!(decode_png(&path, &mut budget).is_err());
        assert_eq!(budget, 0);
        let mut budget = 20;
        assert!(decode_png(&path, &mut budget).is_err());
        assert_eq!(budget, 20);
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(read_bounded(&path, bytes.len() as u64).unwrap(), bytes);
        assert!(read_bounded(&path, bytes.len() as u64 - 1).is_err());
    }
}
