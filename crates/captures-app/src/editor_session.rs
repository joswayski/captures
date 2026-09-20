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
use image::{ImageFormat, ImageReader, RgbaImage};
use serde::{Deserialize, Serialize};

use crate::{
    editor::{
        Document, DocumentHistory, Element, ElementBase, ImageElement, LayerEdit, OptionalNullable,
        Point, Rect, image_bounds,
    },
    editor_render::{MAX_RENDER_DIMENSION, MAX_RENDER_PIXELS, render},
};

const ASSET_PREFIX: &str = "draft-asset:";
const MAX_METADATA_BYTES: u64 = 8 * 1024 * 1024;

pub use captures_image::{ExportFormat, ExportOptions, ExportQuality, PngOptions};

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
    Layer {
        id: String,
        edit: LayerEdit,
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

#[derive(Debug, Serialize)]
pub struct Snapshot<'a> {
    pub artifact_id: &'a str,
    pub document: &'a Document,
    pub can_undo: bool,
    pub can_redo: bool,
    /// Changes since the last successful draft save (or open), not since capture.
    pub unsaved_changes: bool,
    pub has_draft: bool,
}

pub struct EditorSession {
    artifact_id: String,
    drafts_root: PathBuf,
    history: DocumentHistory,
    original_path: PathBuf,
    persisted: Document,
    assets: BTreeMap<String, Arc<RgbaImage>>,
    pixels: Arc<RgbaImage>,
    has_draft: bool,
}

impl EditorSession {
    pub fn open(request: OpenRequest) -> Result<Self, String> {
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
        let pixels = Arc::new(render(&document, &assets)?);
        Ok(Self {
            artifact_id: request.artifact_id,
            drafts_root: request.drafts_root,
            history: DocumentHistory::new(document.clone()),
            original_path,
            persisted: document,
            assets,
            pixels,
            has_draft,
        })
    }

    #[must_use]
    pub fn snapshot(&self) -> Snapshot<'_> {
        Snapshot {
            artifact_id: &self.artifact_id,
            document: self.history.current(),
            can_undo: self.history.undo_len() > 0,
            can_redo: self.history.redo_len() > 0,
            unsaved_changes: self.history.current() != &self.persisted,
            has_draft: self.has_draft,
        }
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
        captures_image::encode_export(&self.pixels, options)
    }

    /// Import one decoded image as a single undoable edit. Asset ownership,
    /// document history, and rendered pixels are published atomically.
    pub fn import_image(&mut self, request: ImportImage) -> Result<String, String> {
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
        let pixels = render(history.current(), &assets)?;

        self.history = history;
        self.assets = assets;
        self.pixels = Arc::new(pixels);
        Ok(layer_id)
    }

    pub fn execute(&mut self, request: Request) -> Result<(), String> {
        let request = match request {
            Request::Snapshot => return Ok(()),
            Request::SaveDraft { updated_at_ms } => return self.save_draft(updated_at_ms),
            Request::DiscardDraft => return self.discard_draft(),
            edit => edit,
        };
        let mut next = self.history.clone();
        match request {
            Request::Snapshot | Request::SaveDraft { .. } | Request::DiscardDraft => unreachable!(),
            Request::Undo => {
                next.undo();
            }
            Request::Redo => {
                next.redo();
            }
            Request::Commit { document } => {
                next.commit(document);
            }
            Request::Layer { id, edit } => {
                let mut document = next.current().clone();
                document.edit_layer(&id, edit)?;
                next.commit(document);
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
        let pixels = render(next.current(), &self.assets)?;
        // Rendering/validation failure leaves both the undo stacks and frame
        // unchanged. Hosts never receive a half-applied edit.
        self.history = next;
        self.pixels = Arc::new(pixels);
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
        editor_draft::save(
            &self.drafts_root,
            editor_draft::SaveRequest {
                artifact_id: self.artifact_id.clone(),
                document: serde_json::to_value(document).map_err(|error| error.to_string())?,
                assets,
                updated_at_ms,
            },
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
        let pixels = render(&original, &assets)?;
        editor_draft::discard(&self.drafts_root, &self.artifact_id)
            .map_err(|error| error.to_string())?;
        self.history = DocumentHistory::new(original.clone());
        self.persisted = original;
        self.assets = assets;
        self.pixels = Arc::new(pixels);
        self.has_draft = false;
        Ok(())
    }
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
